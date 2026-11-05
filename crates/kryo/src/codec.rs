//! The block codecs. Decoding lives here for the setup; encoding only with `freeze`.

use crate::format::{Codec, Filter};
use anyhow::{Context, Result};
use liblzma::stream::{Filters, LzmaOptions, Stream};
use std::io::Read;

fn lzma_options(dict: u32, lc: u8, lp: u8, pb: u8) -> Result<LzmaOptions> {
    let mut opts = LzmaOptions::new_preset(6)?;
    opts.dict_size(dict)
        .literal_context_bits(lc as u32)
        .literal_position_bits(lp as u32)
        .position_bits(pb as u32);
    Ok(opts)
}

fn filter_chain(filter: Filter, opts: &LzmaOptions) -> Result<Filters> {
    let mut chain = Filters::new();
    match filter {
        Filter::None => {}
        Filter::X86 => {
            chain.x86();
        }
        Filter::Delta(distance) => {
            chain.delta_properties(&[distance.saturating_sub(1)])?;
        }
    }
    chain.lzma2(opts);
    Ok(chain)
}

/// Wraps the packed bytes of a block in the reader that decodes them.
pub fn decoder<'a, R: Read + 'a>(codec: Codec, packed: R) -> Result<Box<dyn Read + 'a>> {
    Ok(match codec {
        Codec::Store => Box::new(packed),
        Codec::Zstd { window_log } => {
            let mut d = zstd::stream::read::Decoder::new(packed)?;
            d.window_log_max(window_log.max(10) as u32)?;
            Box::new(d)
        }
        Codec::Lzma2 { dict, lc, lp, pb, filter } => {
            let opts = lzma_options(dict, lc, lp, pb)?;
            let chain = filter_chain(filter, &opts)?;
            let stream = Stream::new_raw_decoder(&chain).context("starting LZMA2")?;
            Box::new(liblzma::read::XzDecoder::new_stream(packed, stream))
        }
        Codec::Ppmd { order, mem } => {
            let d = ppmd_rust::Ppmd7Decoder::new(packed, order as u32, mem)
                .map_err(|e| anyhow::anyhow!("starting PPMd: {e:?}"))?;
            Box::new(d)
        }
    })
}

#[cfg(feature = "freeze")]
pub use encode::*;

#[cfg(feature = "freeze")]
mod encode {
    use super::*;
    use liblzma::stream::{MatchFinder, Mode};
    use std::io::Write;

    /// How hard the LZMA2 match search works.
    #[derive(Clone, Copy, Debug)]
    pub struct Effort {
        pub nice_len: u32,
        pub depth: u32,
    }

    /// A running encoder. `finish` flushes it and hands back the writer.
    pub enum Encoder<W: Write> {
        Store(W),
        Zstd(zstd::stream::write::Encoder<'static, W>),
        Lzma(liblzma::write::XzEncoder<W>),
        Ppmd(Box<ppmd_rust::Ppmd7Encoder<W>>),
    }

    impl<W: Write> Encoder<W> {
        pub fn new(codec: Codec, effort: Effort, zstd_level: i32, out: W) -> Result<Self> {
            Ok(match codec {
                Codec::Store => Encoder::Store(out),
                Codec::Zstd { window_log } => {
                    let mut e = zstd::stream::write::Encoder::new(out, zstd_level)?;
                    e.window_log(window_log as u32)?;
                    e.long_distance_matching(true)?;
                    Encoder::Zstd(e)
                }
                Codec::Lzma2 { dict, lc, lp, pb, filter } => {
                    let mut opts = lzma_options(dict, lc, lp, pb)?;
                    opts.mode(Mode::Normal)
                        .match_finder(MatchFinder::BinaryTree4)
                        .nice_len(effort.nice_len)
                        .depth(effort.depth);
                    let chain = filter_chain(filter, &opts)?;
                    let stream = Stream::new_raw_encoder(&chain).context("starting LZMA2")?;
                    Encoder::Lzma(liblzma::write::XzEncoder::new_stream(out, stream))
                }
                Codec::Ppmd { order, mem } => Encoder::Ppmd(Box::new(
                    ppmd_rust::Ppmd7Encoder::new(out, order as u32, mem)
                        .map_err(|e| anyhow::anyhow!("starting PPMd: {e:?}"))?,
                )),
            })
        }

        pub fn write_all(&mut self, data: &[u8]) -> Result<()> {
            match self {
                Encoder::Store(w) => w.write_all(data)?,
                Encoder::Zstd(w) => w.write_all(data)?,
                Encoder::Lzma(w) => w.write_all(data)?,
                Encoder::Ppmd(w) => w.write_all(data)?,
            }
            Ok(())
        }

        pub fn finish(self) -> Result<W> {
            Ok(match self {
                Encoder::Store(w) => w,
                Encoder::Zstd(w) => w.finish()?,
                Encoder::Lzma(w) => w.finish()?,
                Encoder::Ppmd(w) => w.finish(false)?,
            })
        }
    }

    /// Bytes this codec turns `sample` into. Used to pick settings for a block.
    pub fn trial(codec: Codec, sample: &[u8]) -> Result<usize> {
        let quick = Effort { nice_len: 64, depth: 0 };
        let mut encoder = Encoder::new(codec, quick, 3, Counter(0))?;
        encoder.write_all(sample)?;
        Ok(encoder.finish()?.0)
    }

    struct Counter(usize);

    impl Write for Counter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
