//! Compressed data inside game files stops LZMA2 cold: a PNG or a zipped asset looks random.
//! Here those streams are opened up, so the real content reaches the compressor, with just
//! enough extra data to rebuild the original stream bit for bit at install time.
//!
//! A wrapped run is a sequence of items:
//!
//! ```text
//! 0 raw      varint len, bytes
//! 1 deflate  varint corrections len, varint plain len, corrections, plain
//! 2 png      varint chunk count, chunk sizes, 2 header bytes, varint tail len, tail,
//!            then a deflate item's body
//! 3 jpeg     varint len, Lepton bytes
//! ```

use anyhow::{anyhow, bail, Context, Result};
use std::io::{Cursor, Read};

const RAW: u8 = 0;
const DEFLATE: u8 = 1;
const PNG: u8 = 2;
const JPEG: u8 = 3;

fn read_varint(r: &mut impl Read) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let mut b = [0u8];
        r.read_exact(&mut b)?;
        value |= ((b[0] & 0x7f) as u64) << shift;
        if b[0] & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("a length in this block is damaged")
}

fn read_vec(r: &mut impl Read, len: u64) -> Result<Vec<u8>> {
    // Grows as data arrives, so a damaged length cannot ask for terabytes up front.
    let mut out = Vec::new();
    r.take(len).read_to_end(&mut out)?;
    if out.len() as u64 != len {
        bail!("a block ended early");
    }
    Ok(out)
}

fn rebuild_deflate(plain: &[u8], corrections: &[u8]) -> Result<Vec<u8>> {
    preflate_rs::recreate_whole_deflate_stream(plain, corrections)
        .map_err(|e| anyhow!("rebuilding a compressed stream: {e:?}"))
}

fn png_chunks(stream: &[u8], sizes: &[u64]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(stream.len() + sizes.len() * 12);
    let mut at = 0usize;
    for &size in sizes {
        let end = at.checked_add(size as usize).filter(|&e| e <= stream.len());
        let end = end.context("a PNG in this block is damaged")?;
        out.extend_from_slice(&(size as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(b"IDAT");
        out.extend_from_slice(&stream[at..end]);
        let crc = crc32fast::hash(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
        at = end;
    }
    Ok(out)
}

fn decode_jpeg(lepton: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut reader = Cursor::new(lepton);
    lepton_jpeg::decode_lepton(
        &mut reader,
        &mut out,
        &lepton_jpeg::EnabledFeatures::compat_lepton_vector_read(),
        &lepton_jpeg::SingleThreadPool::default(),
    )
    .map_err(|e| anyhow!("rebuilding a JPEG: {e:?}"))?;
    Ok(out)
}

/// Turns `stored` bytes of items back into the original bytes, passing them to `out` in order.
pub fn unwrap(r: &mut impl Read, stored: u64, out: &mut dyn FnMut(&[u8]) -> Result<()>) -> Result<()> {
    let mut r = CountingReader { inner: r, count: 0 };
    while r.count < stored {
        let mut tag = [0u8];
        r.read_exact(&mut tag)?;
        match tag[0] {
            RAW => {
                let mut left = read_varint(&mut r)?;
                let mut buf = vec![0u8; left.min(1 << 20) as usize];
                while left > 0 {
                    let n = left.min(buf.len() as u64) as usize;
                    r.read_exact(&mut buf[..n])?;
                    out(&buf[..n])?;
                    left -= n as u64;
                }
            }
            DEFLATE => {
                let corrections_len = read_varint(&mut r)?;
                let plain_len = read_varint(&mut r)?;
                let corrections = read_vec(&mut r, corrections_len)?;
                let plain = read_vec(&mut r, plain_len)?;
                out(&rebuild_deflate(&plain, &corrections)?)?;
            }
            PNG => {
                let count = read_varint(&mut r)?;
                let mut sizes = Vec::new();
                for _ in 0..count {
                    sizes.push(read_varint(&mut r)?);
                }
                let header = read_vec(&mut r, 2)?;
                let tail_len = read_varint(&mut r)?;
                let tail = read_vec(&mut r, tail_len)?;
                let corrections_len = read_varint(&mut r)?;
                let plain_len = read_varint(&mut r)?;
                let corrections = read_vec(&mut r, corrections_len)?;
                let plain = read_vec(&mut r, plain_len)?;
                let mut stream = header;
                stream.extend_from_slice(&rebuild_deflate(&plain, &corrections)?);
                stream.extend_from_slice(&tail);
                out(&png_chunks(&stream, &sizes)?)?;
            }
            JPEG => {
                let len = read_varint(&mut r)?;
                let lepton = read_vec(&mut r, len)?;
                out(&decode_jpeg(&lepton)?)?;
            }
            other => bail!("unknown item {other} in a block"),
        }
    }
    Ok(())
}

struct CountingReader<'a, R> {
    inner: &'a mut R,
    count: u64,
}

impl<R: Read> Read for CountingReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count += n as u64;
        Ok(n)
    }
}

#[cfg(feature = "freeze")]
pub use wrap::*;

#[cfg(feature = "freeze")]
mod wrap {
    use super::*;
    use miniz_oxide::inflate::core::{decompress, inflate_flags, DecompressorOxide};
    use miniz_oxide::inflate::TINFLStatus;
    use preflate_rs::{PreflateConfig, PreflateStreamProcessor};
    use std::panic::{catch_unwind, AssertUnwindSafe};

    /// Below this a stream costs more to describe than it saves.
    const MIN_STREAM: usize = 96;

    #[derive(Default, Clone, Copy, Debug)]
    pub struct WrapStats {
        pub streams: u64,
        pub stream_bytes: u64,
        pub jpegs: u64,
        pub jpeg_saved: u64,
    }

    fn put_varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push(v as u8 | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    fn put_raw(out: &mut Vec<u8>, data: &[u8]) {
        if !data.is_empty() {
            out.push(RAW);
            put_varint(out, data.len() as u64);
            out.extend_from_slice(data);
        }
    }

    /// Cheap test before the real work: does a deflate stream really start here?
    fn plausible_deflate(data: &[u8]) -> bool {
        let mut state = DecompressorOxide::new();
        let mut out = vec![0u8; 1 << 15];
        let take = data.len().min(4096);
        let flags = inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF
            | if take < data.len() { inflate_flags::TINFL_FLAG_HAS_MORE_INPUT } else { 0 };
        let (status, _, written) = decompress(&mut state, &data[..take], &mut out, 0, flags);
        match status {
            TINFLStatus::Done => written >= MIN_STREAM / 2,
            TINFLStatus::NeedsMoreInput | TINFLStatus::HasMoreOutput => written >= 16,
            _ => false,
        }
    }

    struct Opened {
        /// Bytes of deflate data consumed.
        compressed: usize,
        corrections: Vec<u8>,
        plain: Vec<u8>,
    }

    /// `Ok(None)` when this is not a stream we can open; `Err(len)` means it was a real
    /// stream of `len` bytes but not worth opening, so scanning can skip past it.
    fn open_stream(data: &[u8]) -> std::result::Result<Option<Opened>, usize> {
        let attempt = catch_unwind(AssertUnwindSafe(|| {
            let config = PreflateConfig::default();
            let mut processor = PreflateStreamProcessor::new(&config);
            let result = processor.decompress(data).ok()?;
            if !processor.is_done() {
                return None;
            }
            let plain = processor.detach_plain_text();
            Some((result.compressed_size, result.corrections, plain.text().to_vec()))
        }));
        let Ok(Some((compressed, corrections, plain))) = attempt else {
            return Ok(None);
        };
        if compressed < MIN_STREAM {
            return Ok(None);
        }
        // Lots of corrections means an encoder preflate cannot model. The corrections
        // barely compress, so past this point opening the stream loses more than it wins.
        // Every stream pays about 60 bytes whatever its size, which small ones can afford.
        if corrections.len() > compressed / 5 + 128 {
            return Err(compressed);
        }
        Ok(Some(Opened { compressed, corrections, plain }))
    }

    fn zlib_header(a: u8, b: u8) -> bool {
        a == 0x78 && matches!(b, 0x01 | 0x5e | 0x9c | 0xda)
    }

    fn gzip_body(data: &[u8]) -> Option<usize> {
        if data.len() < 18 || data[..3] != [0x1f, 0x8b, 8] || data[3] & 0xe0 != 0 {
            return None;
        }
        let flags = data[3];
        let mut at = 10usize;
        if flags & 4 != 0 {
            at += 2 + u16::from_le_bytes([*data.get(at)?, *data.get(at + 1)?]) as usize;
        }
        for bit in [8u8, 16] {
            if flags & bit != 0 {
                at += data.get(at..)?.iter().position(|&c| c == 0)? + 1;
            }
        }
        if flags & 2 != 0 {
            at += 2;
        }
        (at < data.len()).then_some(at)
    }

    fn zip_body(data: &[u8]) -> Option<usize> {
        if data.len() < 30 || &data[..4] != b"PK\x03\x04" || u16::from_le_bytes([data[8], data[9]]) != 8 {
            return None;
        }
        let name = u16::from_le_bytes([data[26], data[27]]) as usize;
        let extra = u16::from_le_bytes([data[28], data[29]]) as usize;
        let at = 30 + name + extra;
        (at < data.len()).then_some(at)
    }

    /// The chunk layout of a PNG's image data.
    struct PngData {
        start: usize,
        end: usize,
        sizes: Vec<u64>,
        stream: Vec<u8>,
    }

    fn png_data(data: &[u8]) -> Option<PngData> {
        const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
        if !data.starts_with(SIGNATURE) {
            return None;
        }
        let mut at = SIGNATURE.len();
        let mut found: Option<PngData> = None;
        while at + 12 <= data.len() {
            let len = u32::from_be_bytes(data[at..at + 4].try_into().ok()?) as usize;
            let kind = &data[at + 4..at + 8];
            let next = at.checked_add(12)?.checked_add(len)?;
            if next > data.len() {
                return None;
            }
            if kind == b"IDAT" {
                let png = found.get_or_insert_with(|| PngData {
                    start: at,
                    end: at,
                    sizes: Vec::new(),
                    stream: Vec::new(),
                });
                png.sizes.push(len as u64);
                png.stream.extend_from_slice(&data[at + 8..at + 8 + len]);
                png.end = next;
            } else if found.is_some() {
                break;
            } else if kind == b"IEND" {
                return None;
            }
            at = next;
        }
        found.filter(|p| p.stream.len() > 6)
    }

    fn put_deflate_body(out: &mut Vec<u8>, stream: &Opened) {
        put_varint(out, stream.corrections.len() as u64);
        put_varint(out, stream.plain.len() as u64);
        out.extend_from_slice(&stream.corrections);
        out.extend_from_slice(&stream.plain);
    }

    /// Writes the PNG item if the image can be rebuilt exactly. Returns the end of the image data.
    fn wrap_png(data: &[u8], out: &mut Vec<u8>, raw_from: &mut usize, at: usize, stats: &mut WrapStats) -> Option<usize> {
        let png = png_data(&data[at..])?;
        if !zlib_like(&png.stream) {
            return None;
        }
        let opened = open_stream(&png.stream[2..]).ok().flatten()?;
        let tail = &png.stream[2 + opened.compressed..];
        // Rebuild now: a wrong chunk checksum in the original would not survive.
        let mut rebuilt = png.stream[..2].to_vec();
        rebuilt.extend_from_slice(&rebuild_deflate(&opened.plain, &opened.corrections).ok()?);
        rebuilt.extend_from_slice(tail);
        let (start, end) = (at + png.start, at + png.end);
        if png_chunks(&rebuilt, &png.sizes).ok()? != data[start..end] {
            return None;
        }
        put_raw(out, &data[*raw_from..start]);
        out.push(PNG);
        put_varint(out, png.sizes.len() as u64);
        for size in &png.sizes {
            put_varint(out, *size);
        }
        out.extend_from_slice(&png.stream[..2]);
        put_varint(out, tail.len() as u64);
        out.extend_from_slice(tail);
        put_deflate_body(out, &opened);
        stats.streams += 1;
        stats.stream_bytes += opened.compressed as u64;
        *raw_from = end;
        Some(end)
    }

    fn zlib_like(stream: &[u8]) -> bool {
        stream.len() > 2 && stream[0] & 0x0f == 8 && (stream[0] as u32 * 256 + stream[1] as u32).is_multiple_of(31)
    }

    /// Finds and opens every stream in `data`. The result is a wrapped run.
    pub fn wrap(data: &[u8], stats: &mut WrapStats) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + data.len() / 2);
        let mut raw_from = 0usize;
        let mut at = 0usize;
        while at + 8 <= data.len() {
            let body = match data[at] {
                0x78 if zlib_header(data[at], data[at + 1]) => Some(at + 2),
                0x1f => gzip_body(&data[at..]).map(|b| at + b),
                b'P' => zip_body(&data[at..]).map(|b| at + b),
                0x89 => {
                    if let Some(end) = wrap_png(data, &mut out, &mut raw_from, at, stats) {
                        at = end;
                        continue;
                    }
                    None
                }
                _ => None,
            };
            let Some(body) = body else {
                at += 1;
                continue;
            };
            if !plausible_deflate(&data[body..]) {
                at += 1;
                continue;
            }
            match open_stream(&data[body..]) {
                Ok(Some(opened)) => {
                    put_raw(&mut out, &data[raw_from..body]);
                    out.push(DEFLATE);
                    put_deflate_body(&mut out, &opened);
                    stats.streams += 1;
                    stats.stream_bytes += opened.compressed as u64;
                    at = body + opened.compressed;
                    raw_from = at;
                }
                Err(len) => at = body + len,
                Ok(None) => at += 1,
            }
        }
        put_raw(&mut out, &data[raw_from..]);
        out
    }

    /// Recompresses a whole JPEG with Lepton, about a fifth smaller, or `None` if it will
    /// not rebuild exactly.
    pub fn wrap_jpeg(data: &[u8], stats: &mut WrapStats) -> Option<Vec<u8>> {
        let lepton = catch_unwind(AssertUnwindSafe(|| {
            let mut reader = Cursor::new(data);
            let mut packed = Cursor::new(Vec::with_capacity(data.len()));
            lepton_jpeg::encode_lepton(
                &mut reader,
                &mut packed,
                &lepton_jpeg::EnabledFeatures::compat_lepton_vector_write(),
                &lepton_jpeg::SingleThreadPool::default(),
            )
            .ok()?;
            let packed = packed.into_inner();
            (decode_jpeg(&packed).ok()? == data).then_some(packed)
        }))
        .ok()??;
        if lepton.len() >= data.len() {
            return None;
        }
        let mut out = Vec::with_capacity(lepton.len() + 10);
        out.push(JPEG);
        put_varint(&mut out, lepton.len() as u64);
        out.extend_from_slice(&lepton);
        stats.jpegs += 1;
        stats.jpeg_saved += (data.len() - lepton.len()) as u64;
        Some(out)
    }
}
