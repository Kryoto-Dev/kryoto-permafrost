//! Turning a game folder into a repack.
//!
//! 1. Read every file once: whole-file hash, plus content-defined chunks for the dedupe.
//! 2. Drop duplicate files, and repeated ranges no matter how far apart they are. Those
//!    come back at install time by copying from what is already on the disk.
//! 3. Plan solid blocks of like with like.
//! 4. Compress the blocks side by side. A finished block is kept on disk, so a stopped
//!    freeze picks up where it left off.
//! 5. Write the setup and the `.kryo` files.
//! 6. Decode the finished repack and compare every byte with the game folder.

use crate::classify::{classify, Class};
use crate::codec::{trial, Effort, Encoder};
use crate::format::*;
use crate::progress::{Progress, Stage};
use crate::scan::{glob_set, walk, ComponentRule, Found};
use crate::thaw::{long_path, test_against, Source};
use crate::unwrap::{wrap, wrap_jpeg, WrapStats};
use crate::{parallel, MemoryGate};
use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::sync_channel;
use std::sync::Mutex;
use std::time::Instant;

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

/// Windows will not start an .exe of 4 GiB or more, so past this the data goes beside it.
pub const EMBED_LIMIT: u64 = 3900 * MIB;

/// The most of one file handled at a time. Also the largest piece kept in memory.
const SLICE: u64 = 128 * MIB;
/// How much of a block is tried against each setting before settling on one.
const SAMPLE: usize = 8 << 20;

const CHUNK_MIN: u32 = 8 << 10;
const CHUNK_AVG: u32 = 32 << 10;
const CHUNK_MAX: u32 = 128 << 10;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    /// Quick. For testing a repack before the long run.
    Frost,
    #[default]
    DeepFreeze,
    /// Smallest result, however long it takes.
    AbsoluteZero,
}

struct Tune {
    use_zstd: bool,
    zstd_level: i32,
    dict: u32,
    effort: Effort,
    block: u64,
    /// Try settings on a sample of every block.
    probe: bool,
    unwrap: bool,
}

impl Level {
    fn tune(self) -> Tune {
        match self {
            Level::Frost => Tune {
                use_zstd: true,
                zstd_level: 17,
                dict: 1 << 27,
                effort: Effort { nice_len: 64, depth: 0 },
                block: 512 * MIB,
                probe: false,
                unwrap: false,
            },
            Level::DeepFreeze => Tune {
                use_zstd: false,
                zstd_level: 0,
                dict: 1 << 27,
                effort: Effort { nice_len: 128, depth: 0 },
                block: GIB,
                probe: false,
                unwrap: true,
            },
            Level::AbsoluteZero => Tune {
                use_zstd: false,
                zstd_level: 0,
                dict: 1 << 30,
                effort: Effort { nice_len: 273, depth: 1024 },
                block: 4 * GIB,
                probe: true,
                unwrap: true,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Embed {
    /// Inside the setup when it fits, beside it when not.
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub source: PathBuf,
    pub out_dir: PathBuf,
    /// File name without extension.
    pub name: String,
    pub game: Game,
    pub level: Level,
    /// Memory the compressors may use at once.
    pub memory: u64,
    pub threads: usize,
    /// Largest `.kryo` part. `None` keeps each set in one file.
    pub split: Option<u64>,
    pub embed: Embed,
    pub exclude: Vec<String>,
    pub components: Vec<ComponentRule>,
    pub redists: Vec<Redist>,
    pub assets: Assets,
    /// The setup program. `None` writes only `.kryo` files.
    pub stub: Option<Vec<u8>>,
    /// Decode the result and compare it with the source.
    pub test: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub setup: Option<PathBuf>,
    pub outputs: Vec<PathBuf>,
    pub original: u64,
    /// The game's data alone.
    pub packed: u64,
    /// Everything written, with the setup program and its art.
    pub total_output: u64,
    pub seconds: f64,
    pub tested: bool,
    pub duplicate_bytes: u64,
    pub unwrapped_streams: u64,
    pub unwrapped_bytes: u64,
    pub jpegs: u64,
    pub jpeg_saved: u64,
    pub files: u64,
    pub memory_to_install: u64,
    pub codecs: Vec<CodecShare>,
    pub classes: Vec<ClassStats>,
    /// Plain-text summary for posting with the repack.
    pub info: PathBuf,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CodecShare {
    pub codec: String,
    pub blocks: u32,
    pub stored: u64,
    pub packed: u64,
}

/// One file as read in step 1.
struct Read1 {
    found: Found,
    class: Class,
    component: u32,
    hash: [u8; 32],
    chunks: Vec<([u8; 16], u32)>,
}

#[derive(Clone)]
struct Slice {
    file: u32,
    offset: u64,
    len: u64,
}

struct BlockPlan {
    component: u32,
    class: Class,
    slices: Vec<Slice>,
    raw: u64,
}

/// A finished block, as kept beside its packed bytes for resuming.
#[derive(Serialize, Deserialize, Clone)]
struct BlockDone {
    index: u32,
    packed: u64,
    stored: u64,
    codec: Codec,
    hash: [u8; 32],
    entries: Vec<Entry>,
    streams: u64,
    stream_bytes: u64,
    jpegs: u64,
    jpeg_saved: u64,
}

fn check_options(options: &Options) -> Result<()> {
    if !options.source.is_dir() {
        bail!("{} is not a folder.", options.source.display());
    }
    if options.name.trim().is_empty() {
        bail!("The repack needs a name.");
    }
    let out = std::path::absolute(&options.out_dir)?;
    let source = std::path::absolute(&options.source)?;
    if out.starts_with(&source) {
        bail!("Pick an output folder outside the game folder.");
    }
    Ok(())
}

/// Reads a file through once. Hard disks hate being read in several places at a time,
/// so the caller keeps this to a few threads.
fn read_file(found: Found, class: Class, component: u32, chunking: bool, progress: &Progress) -> Result<Read1> {
    let file = File::open(long_path(&found.path)).with_context(|| format!("reading {}", found.relative))?;
    let mut hasher = blake3::Hasher::new();
    let mut chunks = Vec::new();
    if chunking && found.size >= CHUNK_MIN as u64 {
        let reader = HashingReader { inner: io::BufReader::with_capacity(4 << 20, file), hasher: &mut hasher };
        for chunk in fastcdc::v2020::StreamCDC::new(reader, CHUNK_MIN, CHUNK_AVG, CHUNK_MAX) {
            let chunk = chunk.map_err(|e| anyhow::anyhow!("reading {}: {e}", found.relative))?;
            let digest = blake3::hash(&chunk.data);
            let mut key = [0u8; 16];
            key.copy_from_slice(&digest.as_bytes()[..16]);
            chunks.push((key, chunk.length as u32));
            progress.advance(chunk.length as u64);
            progress.checkpoint()?;
        }
    } else {
        let mut file = file;
        let mut buf = vec![0u8; (4 << 20).min(found.size.max(1) as usize)];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            progress.advance(n as u64);
            progress.checkpoint()?;
        }
    }
    Ok(Read1 { hash: *hasher.finalize().as_bytes(), found, class, component, chunks })
}

struct HashingReader<'a, R> {
    inner: R,
    hasher: &'a mut blake3::Hasher,
}

impl<R: Read> Read for HashingReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
}

/// What the dedupe decided for each file.
struct Deduped {
    entries: Vec<FileEntry>,
    /// The ranges of each file that still have to be stored.
    literals: Vec<Vec<(u64, u64)>>,
    duplicate_bytes: u64,
}

/// A file's bytes may only come from files the player is sure to have: its own part,
/// or the main one.
fn dedupe(files: &[Read1], enabled: bool) -> Deduped {
    let mut whole: HashMap<([u8; 32], u32), u32> = HashMap::new();
    let mut chunks: HashMap<([u8; 16], u32), (u32, u64)> = HashMap::new();
    let mut entries = Vec::with_capacity(files.len());
    let mut literals = Vec::with_capacity(files.len());
    let mut duplicate_bytes = 0u64;

    for (index, file) in files.iter().enumerate() {
        let index = index as u32;
        let mut entry = FileEntry {
            path: file.found.relative.clone(),
            size: file.found.size,
            modified: file.found.modified,
            attributes: file.found.read_only as u32,
            hash: file.hash,
            component: file.component,
            copy_of: None,
            references: Vec::new(),
        };
        if file.found.size == 0 {
            entries.push(entry);
            literals.push(Vec::new());
            continue;
        }
        let same = whole.get(&(file.hash, 0)).or_else(|| whole.get(&(file.hash, file.component)));
        if let Some(&original) = same.filter(|_| enabled) {
            entry.copy_of = Some(original);
            duplicate_bytes += file.found.size;
            entries.push(entry);
            literals.push(Vec::new());
            continue;
        }
        whole.insert((file.hash, file.component), index);

        let mut stored: Vec<(u64, u64)> = Vec::new();
        if file.chunks.is_empty() || !enabled {
            stored.push((0, file.found.size));
        } else {
            let mut at = 0u64;
            for (key, len) in &file.chunks {
                let len = *len as u64;
                let hit = chunks.get(&(*key, 0)).or_else(|| chunks.get(&(*key, file.component)));
                if let Some(&(source, source_offset)) = hit {
                    duplicate_bytes += len;
                    match entry.references.last_mut() {
                        Some(last)
                            if last.source == source
                                && last.offset + last.len == at
                                && last.source_offset + last.len == source_offset =>
                        {
                            last.len += len;
                        }
                        _ => entry.references.push(Reference { offset: at, source, source_offset, len }),
                    }
                } else {
                    chunks.insert((*key, file.component), (index, at));
                    match stored.last_mut() {
                        Some(last) if last.0 + last.1 == at => last.1 += len,
                        _ => stored.push((at, len)),
                    }
                }
                at += len;
            }
        }
        entries.push(entry);
        literals.push(stored);
    }
    Deduped { entries, literals, duplicate_bytes }
}

fn plan_blocks(files: &[Read1], deduped: &Deduped, tune: &Tune) -> Vec<BlockPlan> {
    let mut groups: HashMap<(u32, Class), Vec<u32>> = HashMap::new();
    for (index, file) in files.iter().enumerate() {
        if !deduped.literals[index].is_empty() {
            groups.entry((file.component, file.class)).or_default().push(index as u32);
        }
    }
    let mut keys: Vec<_> = groups.keys().copied().collect();
    keys.sort();
    let extension = |f: &Read1| {
        let name = f.found.relative.rsplit('/').next().unwrap_or_default();
        name.rsplit('.').next().unwrap_or_default().to_ascii_lowercase()
    };
    let mut plans = Vec::new();
    for key in keys {
        let mut members = groups.remove(&key).unwrap();
        members.sort_by_cached_key(|&i| (extension(&files[i as usize]), files[i as usize].found.relative.clone()));
        let mut current = BlockPlan { component: key.0, class: key.1, slices: Vec::new(), raw: 0 };
        for file in members {
            for &(start, len) in &deduped.literals[file as usize] {
                let mut at = start;
                let end = start + len;
                while at < end {
                    let take = (end - at).min(SLICE);
                    current.slices.push(Slice { file, offset: at, len: take });
                    current.raw += take;
                    at += take;
                    if current.raw >= tune.block {
                        let full = std::mem::replace(
                            &mut current,
                            BlockPlan { component: key.0, class: key.1, slices: Vec::new(), raw: 0 },
                        );
                        plans.push(full);
                    }
                }
            }
        }
        if !current.slices.is_empty() {
            plans.push(current);
        }
    }
    plans
}

/// A dictionary bigger than the data only wastes memory, on both ends.
fn dict_for(raw: u64, class: Class, tune: &Tune, memory: u64) -> u32 {
    let grown = if matches!(class, Class::Data) { raw.saturating_mul(3) } else { raw };
    let mut dict = grown.max(MIB).next_power_of_two().min(tune.dict as u64);
    while dict > 16 * MIB && encoder_memory(dict) > memory {
        dict /= 2;
    }
    dict as u32
}

fn encoder_memory(dict: u64) -> u64 {
    dict * 12 + 192 * MIB
}

/// Picks the block's codec, trying the candidates on a sample when the level asks for it.
fn choose_codec(plan: &BlockPlan, tune: &Tune, memory: u64, sample: &[u8]) -> Result<Codec> {
    if plan.class == Class::Jpeg {
        return Ok(Codec::Store);
    }
    if tune.use_zstd {
        let window_log = (plan.raw.max(MIB).next_power_of_two().ilog2() as u8).clamp(20, 27);
        return Ok(Codec::Zstd { window_log });
    }
    let dict = dict_for(plan.raw, plan.class, tune, memory);
    let lzma = |lc, lp, pb, filter| Codec::Lzma2 { dict, lc, lp, pb, filter };

    if plan.class == Class::Packed {
        // Already compressed. Do not spend hours finding that out block by block.
        let squeezed = zstd::bulk::compress(sample, 3)?.len();
        if squeezed * 100 >= sample.len() * 99 {
            return Ok(Codec::Store);
        }
        return Ok(lzma(3, 0, 2, Filter::None));
    }

    let filter = match plan.class {
        Class::Exe => Filter::X86,
        Class::Audio(frame) => Filter::Delta(frame),
        _ => Filter::None,
    };
    if !tune.probe || sample.len() < (1 << 20) {
        return Ok(match plan.class {
            Class::Text => lzma(4, 0, 0, filter),
            _ => lzma(3, 0, 2, filter),
        });
    }

    let small = dict.min(1 << 24);
    let try_lzma = |lc, lp, pb, filter| Codec::Lzma2 { dict: small, lc, lp, pb, filter };
    let mut candidates = vec![
        try_lzma(3, 0, 2, filter),
        try_lzma(4, 0, 2, filter),
        try_lzma(3, 0, 0, filter),
        try_lzma(4, 0, 0, filter),
        try_lzma(2, 2, 2, filter),
        try_lzma(0, 2, 2, filter),
    ];
    if filter != Filter::None {
        candidates.push(try_lzma(3, 0, 2, Filter::None));
    }
    if plan.class == Class::Text {
        candidates.push(Codec::Ppmd { order: 16, mem: ppmd_memory(plan.raw) });
    }
    let results: Vec<(Codec, usize)> = candidates
        .into_par_iter()
        .map(|c| trial(c, sample).map(|size| (c, size)))
        .collect::<Result<_>>()?;
    let (best, _) = results.into_iter().min_by_key(|(_, size)| *size).unwrap();
    Ok(match best {
        Codec::Lzma2 { lc, lp, pb, filter, .. } => lzma(lc, lp, pb, filter),
        other => other,
    })
}

fn ppmd_memory(raw: u64) -> u32 {
    (raw.saturating_mul(4).max(16 * MIB).next_power_of_two().min(512 * MIB)) as u32
}

struct Prepared {
    file: u32,
    offset: u64,
    out: u64,
    kind: EntryKind,
    bytes: Vec<u8>,
}

fn read_range(path: &Path, offset: u64, len: u64) -> Result<Vec<u8>> {
    let mut file = File::open(long_path(path))?;
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0u8; len as usize];
    file.read_exact(&mut bytes).with_context(|| format!("{} changed while freezing", path.display()))?;
    Ok(bytes)
}

fn prepare(slice: &Slice, files: &[Read1], unwrapping: bool, stats: &mut WrapStats) -> Result<Prepared> {
    let source = &files[slice.file as usize];
    let raw = read_range(&source.found.path, slice.offset, slice.len)?;
    let (kind, bytes) = match source.class {
        Class::Jpeg if unwrapping && slice.offset == 0 && slice.len == source.found.size => match wrap_jpeg(&raw, stats) {
            Some(wrapped) => (EntryKind::Wrapped, wrapped),
            None => (EntryKind::Raw, raw),
        },
        Class::Data if unwrapping => {
            let before = stats.streams;
            let wrapped = wrap(&raw, stats);
            if stats.streams > before {
                (EntryKind::Wrapped, wrapped)
            } else {
                (EntryKind::Raw, raw)
            }
        }
        _ => (EntryKind::Raw, raw),
    };
    Ok(Prepared { file: slice.file, offset: slice.offset, out: slice.len, kind, bytes })
}

struct Spool {
    out: BufWriter<File>,
    hasher: blake3::Hasher,
    written: u64,
}

impl Write for Spool {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.out.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

struct Run<'a> {
    files: &'a [Read1],
    tune: &'a Tune,
    work: &'a Path,
    memory: u64,
    pool: &'a rayon::ThreadPool,
    progress: &'a Progress,
}

fn compress_block(run: &Run, index: u32, plan: &BlockPlan) -> Result<BlockDone> {
    let unwrapping = run.tune.unwrap && matches!(plan.class, Class::Data | Class::Jpeg);
    let (sender, receiver) = sync_channel::<Result<(Vec<Prepared>, WrapStats)>>(1);

    std::thread::scope(|scope| -> Result<BlockDone> {
        // Opening up streams is the slow part, so it runs ahead of the compressor on the pool.
        scope.spawn(|| {
            // Taken by value so the compressor sees the channel close when this thread ends.
            let sender = sender;
            let mut at = 0;
            while at < plan.slices.len() {
                let mut end = at;
                let mut bytes = 0;
                while end < plan.slices.len() && (bytes < SLICE || end == at) && end - at < 512 {
                    bytes += plan.slices[end].len;
                    end += 1;
                }
                let batch = &plan.slices[at..end];
                at = end;
                let result = run.pool.install(|| {
                    batch
                        .par_iter()
                        .map(|slice| {
                            let mut stats = WrapStats::default();
                            prepare(slice, run.files, unwrapping, &mut stats).map(|p| (p, stats))
                        })
                        .collect::<Result<Vec<_>>>()
                });
                let message = result.map(|items| {
                    let mut total = WrapStats::default();
                    let mut prepared = Vec::with_capacity(items.len());
                    for (item, stats) in items {
                        total.streams += stats.streams;
                        total.stream_bytes += stats.stream_bytes;
                        total.jpegs += stats.jpegs;
                        total.jpeg_saved += stats.jpeg_saved;
                        prepared.push(item);
                    }
                    (prepared, total)
                });
                let failed = message.is_err();
                if sender.send(message).is_err() || failed {
                    return;
                }
            }
        });

        let path = run.work.join(format!("{index}.pack.tmp"));
        let spool = Spool {
            out: BufWriter::with_capacity(1 << 20, File::create(&path)?),
            hasher: blake3::Hasher::new(),
            written: 0,
        };
        let mut sample = Vec::new();
        let mut spool_slot = Some(spool);
        let mut encoder: Option<Encoder<Spool>> = None;
        let mut codec = Codec::Store;
        let mut entries = Vec::new();
        let mut stats = WrapStats::default();
        let mut stored = 0u64;
        let mut last_packed = 0u64;

        let start = |sample: &mut Vec<u8>, slot: &mut Option<Spool>, codec: &mut Codec| -> Result<Encoder<Spool>> {
            *codec = run.pool.install(|| choose_codec(plan, run.tune, run.memory, sample))?;
            let mut e = Encoder::new(*codec, run.tune.effort, run.tune.zstd_level, slot.take().unwrap())?;
            e.write_all(sample)?;
            sample.clear();
            sample.shrink_to_fit();
            Ok(e)
        };

        for message in receiver.iter() {
            let (batch, batch_stats) = message?;
            stats.streams += batch_stats.streams;
            stats.stream_bytes += batch_stats.stream_bytes;
            stats.jpegs += batch_stats.jpegs;
            stats.jpeg_saved += batch_stats.jpeg_saved;
            for item in batch {
                run.progress.checkpoint()?;
                run.progress.set_detail(&run.files[item.file as usize].found.relative);
                entries.push(Entry {
                    file: item.file,
                    offset: item.offset,
                    stored: item.bytes.len() as u64,
                    out: item.out,
                    kind: item.kind,
                });
                stored += item.bytes.len() as u64;
                let mut rest = item.bytes.as_slice();
                if encoder.is_none() {
                    let take = (SAMPLE - sample.len()).min(rest.len());
                    sample.extend_from_slice(&rest[..take]);
                    rest = &rest[take..];
                    if sample.len() >= SAMPLE {
                        encoder = Some(start(&mut sample, &mut spool_slot, &mut codec)?);
                    }
                }
                if let Some(e) = encoder.as_mut() {
                    // Small writes keep the counters moving during a long file.
                    for piece in rest.chunks(4 << 20) {
                        e.write_all(piece)?;
                        run.progress.checkpoint()?;
                    }
                    let packed = match e {
                        Encoder::Store(s) => s.written,
                        Encoder::Zstd(s) => s.get_ref().written,
                        Encoder::Lzma(s) => s.get_ref().written,
                        Encoder::Ppmd(s) => s.get_ref().written,
                    };
                    run.progress.packed.fetch_add(packed - last_packed, Ordering::Relaxed);
                    last_packed = packed;
                }
                run.progress.advance(item.out);
            }
        }
        let encoder = match encoder {
            Some(e) => e,
            None => start(&mut sample, &mut spool_slot, &mut codec)?,
        };
        let mut spool = encoder.finish()?;
        spool.flush()?;
        run.progress.packed.fetch_add(spool.written - last_packed, Ordering::Relaxed);
        let packed = spool.written;
        let hash = *spool.hasher.finalize().as_bytes();
        drop(spool.out);
        fs::rename(&path, run.work.join(format!("{index}.pack")))?;
        let done = BlockDone {
            index,
            packed,
            stored,
            codec,
            hash,
            entries,
            streams: stats.streams,
            stream_bytes: stats.stream_bytes,
            jpegs: stats.jpegs,
            jpeg_saved: stats.jpeg_saved,
        };
        fs::write(run.work.join(format!("{index}.meta")), postcard::to_stdvec(&done)?)?;
        Ok(done)
    })
}

fn load_done(work: &Path, index: u32) -> Option<BlockDone> {
    let meta = fs::read(work.join(format!("{index}.meta"))).ok()?;
    let done: BlockDone = postcard::from_bytes(&meta).ok()?;
    let len = fs::metadata(work.join(format!("{index}.pack"))).ok()?.len();
    (len == done.packed && done.index == index).then_some(done)
}

/// Writes one component's data as numbered `.kryo` parts.
struct PartWriter<'a> {
    dir: &'a Path,
    stem: String,
    set_id: [u8; 16],
    split: Option<u64>,
    numbered: bool,
    volumes: Vec<Volume>,
    paths: Vec<PathBuf>,
    current: Option<BufWriter<File>>,
    room: u64,
}

impl<'a> PartWriter<'a> {
    fn new(dir: &'a Path, stem: String, set_id: [u8; 16], split: Option<u64>, total: u64) -> Self {
        let numbered = split.is_some_and(|s| total > s);
        PartWriter { dir, stem, set_id, split, numbered, volumes: Vec::new(), paths: Vec::new(), current: None, room: 0 }
    }

    fn next_part(&mut self) -> Result<()> {
        self.close()?;
        let number = self.volumes.len() as u32 + 1;
        let name = if self.numbered {
            format!("{}.{number:03}.kryo", self.stem)
        } else {
            format!("{}.kryo", self.stem)
        };
        let path = self.dir.join(&name);
        let mut file = BufWriter::with_capacity(1 << 20, File::create(&path).with_context(|| format!("writing {name}"))?);
        file.write_all(&volume_header(&self.set_id, number))?;
        self.volumes.push(Volume { name, offset: VOLUME_HEADER_LEN, len: 0 });
        self.paths.push(path);
        self.current = Some(file);
        self.room = self.split.unwrap_or(u64::MAX).saturating_sub(VOLUME_HEADER_LEN);
        Ok(())
    }

    fn write_from(&mut self, reader: &mut impl Read, mut left: u64, progress: &Progress) -> Result<()> {
        let mut buf = vec![0u8; 1 << 20];
        while left > 0 {
            if self.current.is_none() || self.room == 0 {
                self.next_part()?;
            }
            let n = left.min(self.room).min(buf.len() as u64) as usize;
            reader.read_exact(&mut buf[..n])?;
            self.current.as_mut().unwrap().write_all(&buf[..n])?;
            self.volumes.last_mut().unwrap().len += n as u64;
            self.room -= n as u64;
            left -= n as u64;
            progress.advance(n as u64);
        }
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        if let Some(mut file) = self.current.take() {
            file.flush()?;
        }
        Ok(())
    }
}

/// `Some: Game?` becomes a name Windows accepts.
pub fn safe_file_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { ' ' } else { c })
        .collect();
    clean.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches('.').to_string()
}

fn append_tail(file: &mut (impl Write + Seek), assets: Option<&[u8]>, manifest: &[u8]) -> Result<()> {
    let assets_offset = file.stream_position()?;
    let assets = assets.unwrap_or_default();
    file.write_all(assets)?;
    let manifest_offset = file.stream_position()?;
    file.write_all(manifest)?;
    let mut hash = [0u8; 16];
    hash.copy_from_slice(&blake3::hash(manifest).as_bytes()[..16]);
    Trailer {
        assets_offset,
        assets_len: assets.len() as u64,
        manifest_offset,
        manifest_len: manifest.len() as u64,
        manifest_hash: hash,
    }
    .write_to(file)
}

fn plan_hash(files: &[Read1], options: &Options) -> blake3::Hash {
    let mut h = blake3::Hasher::new();
    h.update(format!("{}:{:?}:{:?}", FORMAT_VERSION, options.level, options.memory).as_bytes());
    for rule in &options.components {
        h.update(rule.id.as_bytes());
        h.update(rule.patterns.join("\n").as_bytes());
    }
    for file in files {
        h.update(file.found.relative.as_bytes());
        h.update(&file.hash);
    }
    h.finalize()
}

/// Makes the repack.
pub fn freeze(options: &Options, progress: &Progress) -> Result<Report> {
    let started = Instant::now();
    check_options(options)?;
    fs::create_dir_all(&options.out_dir).context("creating the output folder")?;
    let tune = options.level.tune();
    let threads = options.threads.max(1);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build()?;

    // Step 1: read everything.
    progress.start(Stage::Scanning, 0);
    let (found, empty_dirs) = walk(&options.source, &options.exclude)?;
    if found.is_empty() {
        bail!("There is nothing to freeze in that folder.");
    }
    let rules: Vec<_> = options
        .components
        .iter()
        .map(|rule| glob_set(&rule.patterns))
        .collect::<Result<_>>()?;
    let component_of = |relative: &str| {
        rules.iter().position(|set| set.is_match(relative)).map(|i| i as u32 + 1).unwrap_or(0)
    };

    let original: u64 = found.iter().map(|f| f.size).sum();
    progress.start(Stage::Reading, original);
    let slots: Vec<Mutex<Option<Read1>>> = found.iter().map(|_| Mutex::new(None)).collect();
    let found_ref = &found;
    parallel(threads.min(4), found.len(), |i| {
        let file = found_ref[i].clone();
        progress.set_detail(&file.relative);
        let class = classify(&file.path, file.size);
        let chunking = !matches!(class, Class::Jpeg);
        let component = component_of(&file.relative);
        let read = read_file(file, class, component, chunking, progress)?;
        *slots[i].lock().unwrap() = Some(read);
        Ok(())
    })?;
    let mut files: Vec<Read1> = slots.into_iter().map(|s| s.into_inner().unwrap().unwrap()).collect();
    // Main part first, so optional parts can lean on it.
    files.sort_by(|a, b| (a.component, &a.found.relative).cmp(&(b.component, &b.found.relative)));

    // Steps 2 and 3.
    let deduped = dedupe(&files, true);
    let plans = plan_blocks(&files, &deduped, &tune);
    let stored_total: u64 = plans.iter().map(|p| p.raw).sum();
    progress.original.store(original, Ordering::SeqCst);
    progress.repeated.store(deduped.duplicate_bytes, Ordering::SeqCst);
    progress.blocks_total.store(plans.len() as u64, Ordering::SeqCst);
    progress.blocks_done.store(0, Ordering::SeqCst);

    let plan_id = plan_hash(&files, options);
    let work = options.out_dir.join(format!(".permafrost-{}", &plan_id.to_hex()[..12]));
    fs::create_dir_all(&work)?;

    // Step 4.
    progress.start(Stage::Compressing, stored_total);
    progress.packed.store(0, Ordering::SeqCst);
    let mut done: Vec<Option<BlockDone>> = vec![None; plans.len()];
    for (index, slot) in done.iter_mut().enumerate() {
        *slot = load_done(&work, index as u32);
        if let Some(block) = slot {
            progress.advance(plans[index].raw);
            progress.packed.fetch_add(block.packed, Ordering::Relaxed);
            progress.blocks_done.fetch_add(1, Ordering::Relaxed);
        }
    }
    let mut todo: Vec<usize> = (0..plans.len()).filter(|&i| done[i].is_none()).collect();
    todo.sort_by_key(|&i| std::cmp::Reverse(plans[i].raw));
    let run = Run { files: &files, tune: &tune, work: &work, memory: options.memory, pool: &pool, progress };
    let gate = MemoryGate::new(options.memory);
    let results = Mutex::new(Vec::new());
    parallel(threads, todo.len(), |i| {
        let index = todo[i];
        let plan = &plans[index];
        let needs = if tune.use_zstd {
            512 * MIB
        } else {
            encoder_memory(dict_for(plan.raw, plan.class, &tune, options.memory) as u64)
        };
        let _held = gate.hold(needs);
        let block = compress_block(&run, index as u32, plan)?;
        progress.blocks_done.fetch_add(1, Ordering::Relaxed);
        results.lock().unwrap().push(block);
        Ok(())
    })?;
    for block in results.into_inner().unwrap() {
        let index = block.index as usize;
        done[index] = Some(block);
    }
    let done: Vec<BlockDone> = done.into_iter().map(Option::unwrap).collect();

    // Step 5.
    let packed_total: u64 = done.iter().map(|b| b.packed).sum();
    progress.start(Stage::Writing, packed_total);

    let mut components = vec![Component {
        id: "game".into(),
        name: "Game".into(),
        note: String::new(),
        required: true,
        selected: true,
        ..Default::default()
    }];
    for rule in &options.components {
        components.push(Component {
            id: rule.id.clone(),
            name: rule.name.clone(),
            note: rule.note.clone(),
            required: false,
            selected: rule.selected,
            ..Default::default()
        });
    }
    for file in &files {
        components[file.component as usize].size += file.found.size;
    }

    let set_id: [u8; 16] = {
        let stamp = format!("{plan_id}{:?}", std::time::SystemTime::now());
        blake3::hash(stamp.as_bytes()).as_bytes()[..16].try_into().unwrap()
    };
    let name = safe_file_name(&options.name);
    let embed = match (&options.stub, options.embed) {
        (None, _) | (_, Embed::Never) => false,
        (Some(_), Embed::Always) => true,
        (Some(stub), Embed::Auto) => options.split.is_none() && packed_total + stub.len() as u64 <= EMBED_LIMIT,
    };
    if embed && packed_total + options.stub.as_ref().map_or(0, |s| s.len() as u64) > EMBED_LIMIT {
        bail!("This repack is too big to fit inside one .exe. Keep the data beside the setup instead.");
    }

    // Not `- kryoto`: that ending is how players tell Kryoto's own downloads apart, and
    // anyone can make a repack. Kryoto's uploads get it when they are uploaded.
    let setup_name = format!("{name} Setup.exe");
    let setup_path = options.out_dir.join(&setup_name);
    let mut outputs = Vec::new();
    let mut blocks: Vec<Block> = Vec::with_capacity(done.len());
    let mut setup_file: Option<BufWriter<File>> = None;
    if let Some(stub) = &options.stub {
        let mut file = BufWriter::with_capacity(1 << 20, File::create(&setup_path).context("writing the setup")?);
        file.write_all(stub)?;
        setup_file = Some(file);
    }

    let mut main_part: Option<PathBuf> = None;
    for component in 0..components.len() as u32 {
        let members: Vec<usize> = (0..plans.len()).filter(|&i| plans[i].component == component).collect();
        let total: u64 = members.iter().map(|&i| done[i].packed).sum();
        let stem = if component == 0 {
            name.clone()
        } else {
            format!("{name}.{}", components[component as usize].id)
        };
        let mut parts = PartWriter::new(&options.out_dir, stem, set_id, options.split, total);
        let start = match (&mut setup_file, embed) {
            (Some(file), true) => file.stream_position()?,
            _ => 0,
        };
        let mut offset = 0u64;
        for &index in &members {
            let block = &done[index];
            let pack = work.join(format!("{index}.pack"));
            let mut reader = File::open(&pack)?;
            if embed {
                let mut limited = (&mut reader).take(block.packed);
                let file = setup_file.as_mut().unwrap();
                let copied = io::copy(&mut limited, file)?;
                progress.advance(copied);
            } else {
                parts.write_from(&mut reader, block.packed, progress)?;
            }
            blocks.push(Block {
                component,
                offset,
                packed: block.packed,
                stored: block.stored,
                codec: block.codec,
                hash: block.hash,
                entries: block.entries.clone(),
            });
            offset += block.packed;
        }
        parts.close()?;
        let target = &mut components[component as usize];
        target.packed = total;
        target.volumes = if embed {
            vec![Volume { name: String::new(), offset: start, len: total }]
        } else {
            parts.volumes.clone()
        };
        if component == 0 {
            main_part = parts.paths.last().cloned();
        }
        outputs.extend(parts.paths);
    }

    let mut classes: HashMap<&str, ClassStats> = HashMap::new();
    for file in &files {
        let label = file.class.label();
        let row = classes.entry(label).or_insert_with(|| ClassStats { label: label.into(), ..Default::default() });
        row.files += 1;
        row.original += file.found.size;
    }
    for (plan, block) in plans.iter().zip(&done) {
        if let Some(row) = classes.get_mut(plan.class.label()) {
            row.packed += block.packed;
        }
    }
    let mut classes: Vec<ClassStats> = classes.into_values().collect();
    classes.sort_by_key(|c| std::cmp::Reverse(c.original));

    let stats = Stats {
        original,
        packed: packed_total,
        files: files.len() as u64,
        duplicate_bytes: deduped.duplicate_bytes,
        unwrapped_streams: done.iter().map(|b| b.streams).sum(),
        unwrapped_bytes: done.iter().map(|b| b.stream_bytes).sum(),
        jpegs: done.iter().map(|b| b.jpegs).sum(),
        jpeg_saved: done.iter().map(|b| b.jpeg_saved).sum(),
        classes: classes.clone(),
    };
    let redists: Vec<Redist> = options
        .redists
        .iter()
        .filter(|r| deduped.entries.iter().any(|f| f.path == r.path))
        .cloned()
        .collect();
    let manifest = Manifest {
        format: FORMAT_VERSION,
        set_id,
        game: options.game.clone(),
        components,
        blocks,
        files: deduped.entries,
        empty_dirs,
        redists,
        stats: stats.clone(),
    };
    let manifest_bytes = manifest.to_bytes()?;
    if let Some(mut file) = setup_file.take() {
        let assets = options.assets.to_bytes()?;
        append_tail(&mut file, Some(&assets), &manifest_bytes)?;
        file.flush()?;
        outputs.insert(0, setup_path.clone());
    }
    if let Some(part) = &main_part {
        let mut file = fs::OpenOptions::new().append(true).open(part)?;
        let mut file_at_end = BufWriter::new(&mut file);
        file_at_end.seek(SeekFrom::End(0))?;
        append_tail(&mut file_at_end, None, &manifest_bytes)?;
        file_at_end.flush()?;
    }

    // Step 6.
    if options.test {
        let container = if options.stub.is_some() {
            setup_path.clone()
        } else {
            main_part.clone().context("the repack came out empty")?
        };
        test_against(&manifest, &Source::new(&container), &options.source, threads, options.memory, progress)?;
    }

    let _ = fs::remove_dir_all(&work);
    let total_output = outputs.iter().filter_map(|p| fs::metadata(p).ok()).map(|m| m.len()).sum();
    let mut codecs: HashMap<&str, CodecShare> = HashMap::new();
    for block in &manifest.blocks {
        let label = block.codec.label();
        let share = codecs.entry(label).or_insert_with(|| CodecShare {
            codec: label.into(),
            blocks: 0,
            stored: 0,
            packed: 0,
        });
        share.blocks += 1;
        share.stored += block.stored;
        share.packed += block.packed;
    }
    let mut codecs: Vec<_> = codecs.into_values().collect();
    codecs.sort_by_key(|c| std::cmp::Reverse(c.packed));

    let info = options.out_dir.join(format!("{name} - repack info.txt"));
    fs::write(&info, repack_info(&manifest, total_output, &outputs)).context("writing the repack info")?;

    Ok(Report {
        setup: options.stub.as_ref().map(|_| setup_path.clone()),
        outputs,
        original,
        packed: packed_total,
        total_output,
        seconds: started.elapsed().as_secs_f64(),
        tested: options.test,
        duplicate_bytes: stats.duplicate_bytes,
        unwrapped_streams: stats.unwrapped_streams,
        unwrapped_bytes: stats.unwrapped_bytes,
        jpegs: stats.jpegs,
        jpeg_saved: stats.jpeg_saved,
        files: stats.files,
        memory_to_install: manifest.peak_block_memory(),
        codecs,
        classes,
        info,
    })
}

fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else {
        format!("{:.1} MB", b / 1e6)
    }
}

/// What a repacker usually writes by hand: sizes, parts, and what a player needs to know.
fn repack_info(manifest: &Manifest, total_output: u64, outputs: &[PathBuf]) -> String {
    let game = &manifest.game;
    let stats = &manifest.stats;
    let mut text = game.title.clone();
    if !game.version.is_empty() {
        text.push_str(&format!(" {}", game.version));
    }
    text.push('\n');
    if !game.repacker.is_empty() {
        text.push_str(&format!("Repacked by {}\n", game.repacker));
    }
    text.push_str(&format!(
        "\nOriginal size      {}\nRepack size        {}\nRatio              {:.1}%\nFiles              {}\nMemory to install  {}\n",
        size_text(stats.original),
        size_text(total_output),
        total_output as f64 * 100.0 / stats.original.max(1) as f64,
        stats.files,
        size_text(manifest.peak_block_memory()),
    ));
    let optional: Vec<_> = manifest.components.iter().filter(|c| !c.required).collect();
    if !optional.is_empty() {
        text.push_str("\nOptional parts\n");
        for part in optional {
            text.push_str(&format!("  {}  {}\n", part.name, size_text(part.packed)));
        }
    }
    text.push_str("\nFiles\n");
    for path in outputs {
        if let (Some(name), Ok(meta)) = (path.file_name(), fs::metadata(path)) {
            text.push_str(&format!("  {}  {}\n", name.to_string_lossy(), size_text(meta.len())));
        }
    }
    text.push_str("\nKeep every file in one folder and run the setup. It checks the data as it installs.\n");
    text
}

/// A rough guess at the final size, from compressing a sample of each kind of file.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    pub original: u64,
    pub estimated: u64,
    pub classes: Vec<ClassStats>,
}

/// Compresses a sample of every class, spread across its files, with the level's real
/// pipeline. The dedupe is left out, so a game with lots of repeats beats this.
pub fn estimate(root: &Path, exclude: &[String], level: Level, progress: &Progress) -> Result<Estimate> {
    const SAMPLE_PER_CLASS: u64 = 48 * MIB;
    const PIECE: u64 = 4 * MIB;
    let tune = level.tune();
    let (found, _) = walk(root, exclude)?;
    let mut groups: HashMap<Class, Vec<Found>> = HashMap::new();
    for file in found {
        groups.entry(classify(&file.path, file.size)).or_default().push(file);
    }
    let total: u64 = groups.values().flatten().map(|f| f.size).sum();
    progress.start(Stage::Reading, groups.len() as u64);

    let mut classes: HashMap<&str, ClassStats> = HashMap::new();
    let mut estimated = 0u64;
    for (class, mut members) in groups {
        progress.checkpoint()?;
        progress.set_detail(class.label());
        members.sort_by_key(|f| std::cmp::Reverse(f.size));
        let bytes: u64 = members.iter().map(|f| f.size).sum();
        // Take a piece every so often, so one giant file does not stand in for the rest.
        let every = (bytes / SAMPLE_PER_CLASS.min(bytes.max(1))).max(1) * PIECE;
        let mut sample = Vec::new();
        let mut raw = 0u64;
        let mut stats = WrapStats::default();
        let mut skip = 0u64;
        for file in &members {
            let mut at = skip.min(file.size);
            while at < file.size && raw < SAMPLE_PER_CLASS {
                let take = PIECE.min(file.size - at);
                let bytes = read_range(&file.path, at, take)?;
                let whole = at == 0 && take == file.size;
                let piece = match class {
                    Class::Jpeg if tune.unwrap && whole => wrap_jpeg(&bytes, &mut stats).unwrap_or(bytes),
                    Class::Data if tune.unwrap => wrap(&bytes, &mut stats),
                    _ => bytes,
                };
                raw += take;
                sample.extend_from_slice(&piece);
                at += every;
            }
            skip = at.saturating_sub(file.size);
            if raw >= SAMPLE_PER_CLASS {
                break;
            }
        }
        let plan = BlockPlan { component: 0, class, slices: Vec::new(), raw: bytes };
        let codec = choose_codec(&plan, &tune, 2 * GIB, &sample[..sample.len().min(SAMPLE)])?;
        let packed_sample = trial(codec, &sample)? as f64;
        let share = packed_sample / raw.max(1) as f64;
        let packed = (bytes as f64 * share.min(1.0)) as u64;
        estimated += packed;
        let row = classes.entry(class.label()).or_insert_with(|| ClassStats { label: class.label().into(), ..Default::default() });
        row.files += members.len() as u64;
        row.original += bytes;
        row.packed += packed;
        progress.advance(1);
    }
    let mut classes: Vec<ClassStats> = classes.into_values().collect();
    classes.sort_by_key(|c| std::cmp::Reverse(c.original));
    Ok(Estimate { original: total, estimated, classes })
}
