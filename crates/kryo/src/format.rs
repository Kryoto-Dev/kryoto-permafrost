//! What a `.kryo` holds on disk. `docs/format.md` describes the same layout in prose.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const VOLUME_MAGIC: &[u8; 4] = b"KRYO";
pub const TRAILER_MAGIC: &[u8; 8] = b"KRYOTAIL";
pub const FORMAT_VERSION: u16 = 1;
pub const VOLUME_HEADER_LEN: u64 = 32;
pub const TRAILER_LEN: u64 = 8 * 4 + 16 + 8;

/// How one block was compressed.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Store,
    Zstd { window_log: u8 },
    Lzma2 { dict: u32, lc: u8, lp: u8, pb: u8, filter: Filter },
    Ppmd { order: u8, mem: u32 },
}

/// A reversible pass run in front of LZMA2.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    None,
    /// Rewrites relative x86 calls and jumps as absolute, so repeated calls match.
    X86,
    /// Stores each byte as the difference from the byte `n` back. For PCM audio.
    Delta(u8),
}

impl Codec {
    /// Memory needed to decode a block of this codec.
    pub fn decode_memory(&self) -> u64 {
        const SLACK: u64 = 8 << 20;
        match *self {
            Codec::Store => SLACK,
            Codec::Zstd { window_log } => (1u64 << window_log) + SLACK,
            Codec::Lzma2 { dict, .. } => dict as u64 + SLACK,
            Codec::Ppmd { mem, .. } => mem as u64 + SLACK,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Codec::Store => "store",
            Codec::Zstd { .. } => "zstd",
            Codec::Lzma2 { filter: Filter::X86, .. } => "x86+lzma2",
            Codec::Lzma2 { filter: Filter::Delta(_), .. } => "delta+lzma2",
            Codec::Lzma2 { .. } => "lzma2",
            Codec::Ppmd { .. } => "ppmd",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// The stored bytes are the file's bytes.
    Raw,
    /// The stored bytes are items from `unwrap`, which rebuild into the file's bytes.
    Wrapped,
}

/// One run of a file inside a block.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Entry {
    pub file: u32,
    /// Where the run lands in the file.
    pub offset: u64,
    /// Length in the block's decoded stream.
    pub stored: u64,
    /// Length once written to the file.
    pub out: u64,
    pub kind: EntryKind,
}

/// A compressed block. Blocks decode on their own, so several can run at once.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Block {
    pub component: u32,
    /// Position in the component's data.
    pub offset: u64,
    pub packed: u64,
    /// Length of the decoded stream.
    pub stored: u64,
    pub codec: Codec,
    /// BLAKE3 of the packed bytes.
    pub hash: [u8; 32],
    pub entries: Vec<Entry>,
}

/// A range of a file that repeats a range of another (or the same) file.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Reference {
    pub offset: u64,
    pub source: u32,
    pub source_offset: u64,
    pub len: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileEntry {
    /// Relative to the install folder, with `/` separators.
    pub path: String,
    pub size: u64,
    /// Milliseconds since the Unix epoch.
    pub modified: u64,
    /// Windows file attributes that are worth keeping.
    pub attributes: u32,
    pub hash: [u8; 32],
    pub component: u32,
    /// Set when this file is a byte-for-byte copy of another.
    pub copy_of: Option<u32>,
    pub references: Vec<Reference>,
}

/// Where a run of a component's data lives.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Volume {
    /// File name beside the setup. Empty means the setup itself.
    pub name: String,
    /// Where the data starts in that file.
    pub offset: u64,
    pub len: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Component {
    pub id: String,
    pub name: String,
    pub note: String,
    /// The player cannot untick it.
    pub required: bool,
    /// Ticked when the setup opens.
    pub selected: bool,
    /// Bytes on disk once installed.
    pub size: u64,
    /// Bytes of data in the repack.
    pub packed: u64,
    pub volumes: Vec<Volume>,
}

/// An installer the setup can run after the files are in place.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Redist {
    pub name: String,
    /// Relative to the install folder.
    pub path: String,
    pub args: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Game {
    pub title: String,
    pub version: String,
    /// Who made this repack. Shown as "Repacked by".
    pub repacker: String,
    pub website: String,
    pub notes: String,
    /// The program to start, relative to the install folder.
    pub exe: String,
    pub args: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Stats {
    pub original: u64,
    pub packed: u64,
    pub files: u64,
    pub duplicate_bytes: u64,
    pub unwrapped_streams: u64,
    pub unwrapped_bytes: u64,
    pub jpegs: u64,
    pub jpeg_saved: u64,
    /// How each kind of file fared.
    pub classes: Vec<ClassStats>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ClassStats {
    pub label: String,
    pub files: u64,
    /// Bytes of these files in the game folder.
    pub original: u64,
    /// What they came to. Duplicates cost nothing, so this can be far below the rest.
    pub packed: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Manifest {
    pub format: u16,
    pub set_id: [u8; 16],
    pub game: Game,
    pub components: Vec<Component>,
    pub blocks: Vec<Block>,
    pub files: Vec<FileEntry>,
    /// Folders with nothing in them.
    pub empty_dirs: Vec<String>,
    pub redists: Vec<Redist>,
    pub stats: Stats,
}

impl Manifest {
    /// The most memory a single block needs to decode.
    pub fn peak_block_memory(&self) -> u64 {
        self.blocks.iter().map(|b| b.codec.decode_memory()).max().unwrap_or(0)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let raw = postcard::to_stdvec(self).context("encoding the manifest")?;
        Ok(zstd::bulk::compress(&raw, 19)?)
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let mut raw = Vec::new();
        zstd::stream::read::Decoder::new(data)?.read_to_end(&mut raw).context("reading the manifest")?;
        let manifest: Manifest = postcard::from_bytes(&raw).context("reading the manifest")?;
        if manifest.format > FORMAT_VERSION {
            bail!("This repack needs a newer Permafrost (format {}).", manifest.format);
        }
        Ok(manifest)
    }
}

/// What the setup shows: art, colours, music. Never read by anything but the setup.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Assets {
    /// RGBA, already scaled to the setup's art panel.
    pub cover: Option<Image>,
    /// 16-bit stereo PCM at 44.1 kHz, looped behind the setup.
    pub music: Option<Vec<i16>>,
    /// Picks the setup's accent. One of the site's palettes.
    pub accent: [u8; 3],
    /// The setup plays its own tune when no music was added. False silences it.
    pub tune: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Assets {
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let raw = postcard::to_stdvec(self).context("encoding the setup's art")?;
        Ok(zstd::bulk::compress(&raw, 15)?)
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let mut raw = Vec::new();
        zstd::stream::read::Decoder::new(data)?.read_to_end(&mut raw)?;
        Ok(postcard::from_bytes(&raw)?)
    }
}

/// Written at the very end of a setup, or of the last `.kryo` of the main set.
#[derive(Clone, Copy, Debug)]
pub struct Trailer {
    pub assets_offset: u64,
    pub assets_len: u64,
    pub manifest_offset: u64,
    pub manifest_len: u64,
    pub manifest_hash: [u8; 16],
}

impl Trailer {
    pub fn write_to(&self, out: &mut impl Write) -> Result<()> {
        out.write_all(&self.assets_offset.to_le_bytes())?;
        out.write_all(&self.assets_len.to_le_bytes())?;
        out.write_all(&self.manifest_offset.to_le_bytes())?;
        out.write_all(&self.manifest_len.to_le_bytes())?;
        out.write_all(&self.manifest_hash)?;
        out.write_all(TRAILER_MAGIC)?;
        Ok(())
    }

    pub fn read_from(file: &mut File) -> Result<Option<Self>> {
        let len = file.seek(SeekFrom::End(0))?;
        if len < TRAILER_LEN {
            return Ok(None);
        }
        file.seek(SeekFrom::Start(len - TRAILER_LEN))?;
        let mut raw = [0u8; TRAILER_LEN as usize];
        file.read_exact(&mut raw)?;
        if &raw[48..56] != TRAILER_MAGIC {
            return Ok(None);
        }
        let u = |i: usize| u64::from_le_bytes(raw[i..i + 8].try_into().unwrap());
        Ok(Some(Trailer {
            assets_offset: u(0),
            assets_len: u(8),
            manifest_offset: u(16),
            manifest_len: u(24),
            manifest_hash: raw[32..48].try_into().unwrap(),
        }))
    }
}

/// Reads the manifest and art out of a setup or a `.kryo`.
pub fn open_repack(path: &Path) -> Result<(Manifest, Assets)> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let Some(trailer) = Trailer::read_from(&mut file)? else {
        bail!("{} holds no repack.", path.display());
    };
    let mut raw = vec![0u8; trailer.manifest_len as usize];
    file.seek(SeekFrom::Start(trailer.manifest_offset))?;
    file.read_exact(&mut raw).context("reading the manifest")?;
    if blake3::hash(&raw).as_bytes()[..16] != trailer.manifest_hash {
        bail!("{} is damaged: its manifest does not match. Download it again.", path.display());
    }
    let manifest = Manifest::from_bytes(&raw)?;
    let mut assets = Assets::default();
    if trailer.assets_len > 0 {
        let mut raw = vec![0u8; trailer.assets_len as usize];
        file.seek(SeekFrom::Start(trailer.assets_offset))?;
        file.read_exact(&mut raw)?;
        assets = Assets::from_bytes(&raw)?;
    }
    Ok((manifest, assets))
}

/// The 32 bytes that open every `.kryo`, so a part from the wrong repack is caught.
pub fn volume_header(set_id: &[u8; 16], number: u32) -> [u8; VOLUME_HEADER_LEN as usize] {
    let mut h = [0u8; VOLUME_HEADER_LEN as usize];
    h[..4].copy_from_slice(VOLUME_MAGIC);
    h[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    h[8..24].copy_from_slice(set_id);
    h[24..28].copy_from_slice(&number.to_le_bytes());
    h
}
