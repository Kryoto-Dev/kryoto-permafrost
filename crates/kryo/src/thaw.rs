//! Installing a repack: read the blocks, rebuild the files, check them.
//!
//! The same decoder also runs when a freeze finishes, with `Target::Compare`: every byte
//! comes out of the finished repack and is compared with the game folder it was made from.

use crate::codec;
use crate::format::{Block, EntryKind, FileEntry, Manifest, Volume};
use crate::progress::{Progress, Stage};
use crate::unwrap::unwrap;
use crate::{parallel, MemoryGate};
use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component as PathPart, Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

const RESUME_FILE: &str = ".kryo-thaw";

/// Where the data files are.
#[derive(Clone, Debug)]
pub struct Source {
    /// The setup or main `.kryo` the manifest came from.
    pub container: PathBuf,
    pub dir: PathBuf,
}

impl Source {
    pub fn new(container: &Path) -> Self {
        let dir = container.parent().map(Path::to_path_buf).unwrap_or_default();
        Source { container: container.to_path_buf(), dir }
    }

    fn volume_path(&self, volume: &Volume) -> PathBuf {
        if volume.name.is_empty() {
            self.container.clone()
        } else {
            self.dir.join(&volume.name)
        }
    }
}

#[derive(Clone, Debug)]
pub struct ThawOptions {
    pub dest: PathBuf,
    pub source: Source,
    /// One flag per component.
    pub selected: Vec<bool>,
    pub threads: usize,
    /// Memory the decoders may use at once.
    pub memory: u64,
    /// Read every file back and compare its hash.
    pub verify: bool,
}

/// Reads a block's packed bytes across as many volumes as it spans, hashing as it goes.
struct BlockReader<'a> {
    volumes: &'a [Volume],
    source: &'a Source,
    position: u64,
    left: u64,
    current: Option<(usize, File)>,
    hasher: blake3::Hasher,
}

impl<'a> BlockReader<'a> {
    fn new(volumes: &'a [Volume], source: &'a Source, block: &Block) -> Self {
        BlockReader {
            volumes,
            source,
            position: block.offset,
            left: block.packed,
            current: None,
            hasher: blake3::Hasher::new(),
        }
    }

    fn drain(&mut self) -> io::Result<()> {
        io::copy(self, &mut io::sink()).map(|_| ())
    }
}

impl Read for BlockReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 || buf.is_empty() {
            return Ok(0);
        }
        let mut start = 0u64;
        for (index, volume) in self.volumes.iter().enumerate() {
            if self.position >= start + volume.len {
                start += volume.len;
                continue;
            }
            let inside = self.position - start;
            if !matches!(&self.current, Some((i, _)) if *i == index) {
                let file = File::open(self.source.volume_path(volume))?;
                self.current = Some((index, file));
            }
            let file = &mut self.current.as_mut().unwrap().1;
            file.seek(SeekFrom::Start(volume.offset + inside))?;
            let take = (volume.len - inside).min(self.left).min(buf.len() as u64) as usize;
            let n = file.read(&mut buf[..take])?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "a repack file is cut short"));
            }
            self.hasher.update(&buf[..n]);
            self.position += n as u64;
            self.left -= n as u64;
            return Ok(n);
        }
        Err(io::Error::new(io::ErrorKind::UnexpectedEof, "a repack file is missing data"))
    }
}

#[derive(Clone, Copy)]
pub enum Target<'a> {
    /// Write the files under this folder.
    Disk(&'a Path),
    /// Compare with the files under this folder.
    Compare(&'a Path),
}

/// Turns a stored path into a real one, refusing anything that climbs out of the folder.
pub fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut out = root.to_path_buf();
    for part in Path::new(relative).components() {
        match part {
            PathPart::Normal(name) => out.push(name),
            PathPart::CurDir => {}
            _ => bail!("This repack holds an unsafe path: {relative}"),
        }
    }
    if relative.contains(':') {
        bail!("This repack holds an unsafe path: {relative}");
    }
    Ok(out)
}

/// Windows stops at 260 characters unless the path is written this way.
pub fn long_path(path: &Path) -> PathBuf {
    if !cfg!(windows) {
        return path.to_path_buf();
    }
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let text = absolute.to_string_lossy().replace('/', "\\");
    if text.starts_with(r"\\?\") {
        return PathBuf::from(text);
    }
    if let Some(share) = text.strip_prefix(r"\\") {
        return PathBuf::from(format!(r"\\?\UNC\{share}"));
    }
    PathBuf::from(format!(r"\\?\{text}"))
}

#[cfg(windows)]
fn write_at(file: &File, offset: u64, data: &[u8]) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut done = 0;
    while done < data.len() {
        done += file.seek_write(&data[done..], offset + done as u64)?;
    }
    Ok(())
}

#[cfg(windows)]
fn read_at(file: &File, offset: u64, data: &mut [u8]) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut done = 0;
    while done < data.len() {
        let n = file.seek_read(&mut data[done..], offset + done as u64)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        done += n;
    }
    Ok(())
}

#[cfg(unix)]
fn write_at(file: &File, offset: u64, data: &[u8]) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, data, offset)
}

#[cfg(unix)]
fn read_at(file: &File, offset: u64, data: &mut [u8]) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, data, offset)
}

/// Keeps the file a worker is on open between runs.
struct Output<'a> {
    target: Target<'a>,
    files: &'a [FileEntry],
    open: Option<(u32, File)>,
    scratch: Vec<u8>,
}

impl<'a> Output<'a> {
    fn new(target: Target<'a>, files: &'a [FileEntry]) -> Self {
        Output { target, files, open: None, scratch: Vec::new() }
    }

    fn put(&mut self, file: u32, offset: u64, data: &[u8]) -> Result<()> {
        if !matches!(&self.open, Some((f, _)) if *f == file) {
            let entry = &self.files[file as usize];
            let handle = match self.target {
                Target::Disk(root) => OpenOptions::new()
                    .write(true)
                    .open(long_path(&safe_join(root, &entry.path)?))
                    .with_context(|| format!("writing {}", entry.path))?,
                Target::Compare(root) => File::open(long_path(&safe_join(root, &entry.path)?))
                    .with_context(|| format!("reading {}", entry.path))?,
            };
            self.open = Some((file, handle));
        }
        let handle = &self.open.as_ref().unwrap().1;
        match self.target {
            Target::Disk(_) => write_at(handle, offset, data)
                .with_context(|| format!("writing {}", self.files[file as usize].path))?,
            Target::Compare(_) => {
                self.scratch.resize(data.len(), 0);
                read_at(handle, offset, &mut self.scratch)?;
                if self.scratch != data {
                    bail!("{} did not come back the same at byte {offset}", self.files[file as usize].path);
                }
            }
        }
        Ok(())
    }
}

/// Decodes one block into `target`.
pub fn decode_block(
    manifest: &Manifest,
    block: &Block,
    source: &Source,
    target: Target,
    progress: &Progress,
) -> Result<()> {
    let volumes = &manifest.components[block.component as usize].volumes;
    let mut reader = BlockReader::new(volumes, source, block);
    let mut output = Output::new(target, &manifest.files);
    let result = (|| -> Result<()> {
        let mut decoder = codec::decoder(block.codec, &mut reader)?;
        let mut buf = vec![0u8; 1 << 20];
        for entry in &block.entries {
            progress.checkpoint()?;
            match entry.kind {
                EntryKind::Raw => {
                    let mut left = entry.stored;
                    let mut at = entry.offset;
                    while left > 0 {
                        let n = left.min(buf.len() as u64) as usize;
                        decoder.read_exact(&mut buf[..n])?;
                        output.put(entry.file, at, &buf[..n])?;
                        progress.advance(n as u64);
                        at += n as u64;
                        left -= n as u64;
                    }
                }
                EntryKind::Wrapped => {
                    let mut at = entry.offset;
                    unwrap(&mut decoder, entry.stored, &mut |bytes| {
                        output.put(entry.file, at, bytes)?;
                        progress.advance(bytes.len() as u64);
                        at += bytes.len() as u64;
                        Ok(())
                    })?;
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        // Damaged data makes decoders fail in odd ways. If the hash says that is what
        // happened, say so: it tells the player what to do.
        if error.downcast_ref::<crate::progress::Cancelled>().is_none()
            && reader.drain().is_ok()
            && reader.hasher.finalize().as_bytes() != &block.hash
        {
            return Err(damaged(manifest, block));
        }
        return Err(error);
    }
    reader.drain().map_err(|e| anyhow!(e)).context("reading the repack")?;
    if reader.hasher.finalize().as_bytes() != &block.hash {
        return Err(damaged(manifest, block));
    }
    Ok(())
}

fn damaged(manifest: &Manifest, block: &Block) -> anyhow::Error {
    let volumes = &manifest.components[block.component as usize].volumes;
    let mut start = 0u64;
    let mut name = String::new();
    for volume in volumes {
        if block.offset < start + volume.len {
            name = volume.name.clone();
            break;
        }
        start += volume.len;
    }
    if name.is_empty() {
        anyhow!("The setup file is damaged. Download it again.")
    } else {
        anyhow!("{name} is damaged. Download that file again.")
    }
}

/// Which files and blocks a selection needs.
pub fn selection(manifest: &Manifest, selected: &[bool]) -> (Vec<usize>, Vec<usize>) {
    let on = |component: u32| {
        let c = component as usize;
        manifest.components[c].required || selected.get(c).copied().unwrap_or(false)
    };
    let files = (0..manifest.files.len()).filter(|&i| on(manifest.files[i].component)).collect();
    let blocks = (0..manifest.blocks.len()).filter(|&i| on(manifest.blocks[i].component)).collect();
    (files, blocks)
}

/// Bytes the selection takes on disk.
pub fn selection_size(manifest: &Manifest, selected: &[bool]) -> u64 {
    let (files, _) = selection(manifest, selected);
    files.iter().map(|&i| manifest.files[i].size).sum()
}

/// The data files a selection needs that are missing or too short, by name.
pub fn missing_volumes(manifest: &Manifest, source: &Source, selected: &[bool]) -> Vec<String> {
    let mut missing = Vec::new();
    for (i, component) in manifest.components.iter().enumerate() {
        if !(component.required || selected.get(i).copied().unwrap_or(false)) {
            continue;
        }
        for volume in &component.volumes {
            let len = fs::metadata(source.volume_path(volume)).map(|m| m.len()).unwrap_or(0);
            if len < volume.offset + volume.len {
                missing.push(if volume.name.is_empty() {
                    "the setup file".to_string()
                } else {
                    volume.name.clone()
                });
            }
        }
    }
    missing
}

/// Reads the selected data and compares every block's hash. Writes nothing.
pub fn check_data(manifest: &Manifest, source: &Source, selected: &[bool], progress: &Progress) -> Result<()> {
    let (_, blocks) = selection(manifest, selected);
    let total = blocks.iter().map(|&b| manifest.blocks[b].packed).sum();
    progress.start(Stage::Checking, total);
    for &index in &blocks {
        progress.checkpoint()?;
        let block = &manifest.blocks[index];
        let volumes = &manifest.components[block.component as usize].volumes;
        let mut reader = BlockReader::new(volumes, source, block);
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            progress.advance(n as u64);
        }
        if reader.hasher.finalize().as_bytes() != &block.hash {
            return Err(damaged(manifest, block));
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn resume_state(dest: &Path, manifest: &Manifest) -> HashSet<usize> {
    let Ok(text) = fs::read_to_string(dest.join(RESUME_FILE)) else {
        return HashSet::new();
    };
    let mut lines = text.lines();
    if lines.next() != Some(hex(&manifest.set_id).as_str()) {
        return HashSet::new();
    }
    lines.filter_map(|l| l.parse().ok()).collect()
}

/// True when an earlier install of this repack in `dest` stopped partway.
pub fn can_resume(dest: &Path, manifest: &Manifest) -> bool {
    !resume_state(dest, manifest).is_empty()
}

fn prepare_files(manifest: &Manifest, files: &[usize], dest: &Path, progress: &Progress) -> Result<()> {
    let mut made = HashSet::new();
    for dir in &manifest.empty_dirs {
        fs::create_dir_all(long_path(&safe_join(dest, dir)?))?;
    }
    for &index in files {
        progress.checkpoint()?;
        let entry = &manifest.files[index];
        let path = long_path(&safe_join(dest, &entry.path)?);
        if let Some(parent) = path.parent() {
            if made.insert(parent.to_path_buf()) {
                fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
            }
        }
        if let Ok(meta) = fs::metadata(&path) {
            if meta.permissions().readonly() {
                let mut permissions = meta.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                permissions.set_readonly(false);
                fs::set_permissions(&path, permissions)?;
            }
        }
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("creating {}", entry.path))?;
        file.set_len(entry.size).with_context(|| format!("making room for {}", entry.path))?;
    }
    Ok(())
}

fn copy_range(from: &File, from_offset: u64, to: &File, to_offset: u64, len: u64, buf: &mut [u8]) -> Result<()> {
    let mut done = 0u64;
    while done < len {
        let n = (len - done).min(buf.len() as u64) as usize;
        read_at(from, from_offset + done, &mut buf[..n])?;
        write_at(to, to_offset + done, &buf[..n])?;
        done += n as u64;
    }
    Ok(())
}

/// Fills in every repeated range and duplicate file from the copy already on disk.
/// Sources are always stored ranges, so the order does not matter.
fn link_files(manifest: &Manifest, files: &[usize], dest: &Path, progress: &Progress) -> Result<()> {
    let open_read = |index: usize| -> Result<File> {
        let path = long_path(&safe_join(dest, &manifest.files[index].path)?);
        File::open(path).with_context(|| format!("reading {}", manifest.files[index].path))
    };
    let mut buf = vec![0u8; 4 << 20];
    for &index in files {
        let entry = &manifest.files[index];
        if entry.copy_of.is_none() && entry.references.is_empty() {
            continue;
        }
        progress.checkpoint()?;
        let to = OpenOptions::new().write(true).open(long_path(&safe_join(dest, &entry.path)?))?;
        if let Some(original) = entry.copy_of {
            let from = open_read(original as usize)?;
            let source = &manifest.files[original as usize];
            copy_range(&from, 0, &to, 0, entry.size, &mut buf)?;
            copy_references(manifest, source, &to, &mut buf, dest)?;
            progress.advance(entry.size);
            continue;
        }
        copy_references(manifest, entry, &to, &mut buf, dest)?;
    }
    Ok(())
}

/// Applies `entry`'s references onto `to`. A duplicate of a file with references gets them
/// here too, since its original may not be finished when it is copied.
fn copy_references(manifest: &Manifest, entry: &FileEntry, to: &File, buf: &mut [u8], dest: &Path) -> Result<()> {
    for reference in &entry.references {
        let source = &manifest.files[reference.source as usize];
        let from = File::open(long_path(&safe_join(dest, &source.path)?))
            .with_context(|| format!("reading {}", source.path))?;
        copy_range(&from, reference.source_offset, to, reference.offset, reference.len, buf)?;
    }
    Ok(())
}

fn hash_file(path: &Path, progress: &Progress) -> Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        progress.checkpoint()?;
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(*hasher.finalize().as_bytes());
        }
        hasher.update(&buf[..n]);
        progress.advance(n as u64);
    }
}

fn finish_file(entry: &FileEntry, path: &Path) -> Result<()> {
    let file = OpenOptions::new().write(true).open(path)?;
    if entry.modified > 0 {
        file.set_modified(UNIX_EPOCH + Duration::from_millis(entry.modified))?;
    }
    drop(file);
    if entry.attributes & 1 != 0 {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

/// Installs the selection. If a run of this repack stopped partway, finished blocks are kept.
pub fn thaw(manifest: &Manifest, options: &ThawOptions, progress: &Progress) -> Result<()> {
    let (files, blocks) = selection(manifest, &options.selected);
    let dest = options.dest.as_path();
    let missing = missing_volumes(manifest, &options.source, &options.selected);
    if !missing.is_empty() {
        bail!("Missing or incomplete: {}. Put them next to the setup.", missing.join(", "));
    }

    progress.start(Stage::Unpacking, 0);
    fs::create_dir_all(long_path(dest)).with_context(|| format!("creating {}", dest.display()))?;
    let done = resume_state(dest, manifest);
    let resume_path = dest.join(RESUME_FILE);
    if done.is_empty() {
        let mut state = File::create(&resume_path).context("writing to the install folder")?;
        writeln!(state, "{}", hex(&manifest.set_id))?;
    }
    prepare_files(manifest, &files, dest, progress)?;

    let mut work: Vec<usize> = blocks.iter().copied().filter(|b| !done.contains(b)).collect();
    work.sort_by_key(|&b| std::cmp::Reverse(manifest.blocks[b].stored));
    let output_of = |b: usize| manifest.blocks[b].entries.iter().map(|e| e.out).sum::<u64>();
    let total: u64 = blocks.iter().map(|&b| output_of(b)).sum();
    let already: u64 = blocks.iter().filter(|b| done.contains(b)).map(|&b| output_of(b)).sum();
    progress.start(Stage::Unpacking, total);
    progress.advance(already);

    let gate = MemoryGate::new(options.memory.max(manifest.peak_block_memory()));
    let state = std::sync::Mutex::new(OpenOptions::new().append(true).open(&resume_path)?);
    parallel(options.threads.max(1), work.len(), |i| {
        let index = work[i];
        let block = &manifest.blocks[index];
        let _held = gate.hold(block.codec.decode_memory());
        decode_block(manifest, block, &options.source, Target::Disk(dest), progress)?;
        let mut state = state.lock().unwrap();
        writeln!(state, "{index}")?;
        state.flush()?;
        Ok(())
    })?;

    let linked: u64 = files
        .iter()
        .map(|&i| &manifest.files[i])
        .filter(|f| f.copy_of.is_some())
        .map(|f| f.size)
        .sum();
    progress.start(Stage::Linking, linked);
    link_files(manifest, &files, dest, progress)?;

    if options.verify {
        let total = files.iter().map(|&i| manifest.files[i].size).sum();
        progress.start(Stage::Verifying, total);
        // Two readers: more only makes a hard disk seek.
        parallel(2, files.len(), |i| {
            let entry = &manifest.files[files[i]];
            let path = long_path(&safe_join(dest, &entry.path)?);
            progress.set_detail(&entry.path);
            let hash = hash_file(&path, progress).with_context(|| format!("checking {}", entry.path))?;
            if hash != entry.hash {
                bail!("{} did not install correctly. Check the disk, then install again.", entry.path);
            }
            Ok(())
        })?;
    }

    progress.start(Stage::Finishing, files.len() as u64);
    for &index in &files {
        let entry = &manifest.files[index];
        finish_file(entry, &long_path(&safe_join(dest, &entry.path)?))?;
        progress.advance(1);
    }
    drop(state);
    let _ = fs::remove_file(&resume_path);
    Ok(())
}

/// Decodes the whole repack against the folder it was made from.
pub fn test_against(manifest: &Manifest, source: &Source, root: &Path, threads: usize, memory: u64, progress: &Progress) -> Result<()> {
    let total: u64 = manifest.blocks.iter().flat_map(|b| &b.entries).map(|e| e.out).sum();
    progress.start(Stage::Testing, total);
    let gate = MemoryGate::new(memory.max(manifest.peak_block_memory()));
    let mut order: Vec<usize> = (0..manifest.blocks.len()).collect();
    order.sort_by_key(|&b| std::cmp::Reverse(manifest.blocks[b].stored));
    parallel(threads.max(1), order.len(), |i| {
        let block = &manifest.blocks[order[i]];
        let _held = gate.hold(block.codec.decode_memory());
        decode_block(manifest, block, source, Target::Compare(root), progress)
    })?;
    // Repeated ranges were never stored, so check the plan itself against the source.
    let mut a = vec![0u8; 1 << 20];
    let mut b = vec![0u8; 1 << 20];
    for entry in &manifest.files {
        for reference in &entry.references {
            let source_entry = &manifest.files[reference.source as usize];
            let to = File::open(long_path(&safe_join(root, &entry.path)?))?;
            let from = File::open(long_path(&safe_join(root, &source_entry.path)?))?;
            let mut done = 0u64;
            while done < reference.len {
                let n = (reference.len - done).min(a.len() as u64) as usize;
                read_at(&to, reference.offset + done, &mut a[..n])?;
                read_at(&from, reference.source_offset + done, &mut b[..n])?;
                if a[..n] != b[..n] {
                    bail!("{} has a repeated range that does not match", entry.path);
                }
                done += n as u64;
            }
        }
    }
    Ok(())
}
