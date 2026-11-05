//! Sorts files by what they hold, so like sits next to like in a block and each block
//! gets the filter that suits it.

use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// Native x86 or x64 code.
    Exe,
    Text,
    /// PCM audio. The value is the size of one sample frame in bytes.
    Audio(u8),
    /// Everything else, opened up by `unwrap` where it can be.
    Data,
    /// Media that is already compressed, left as it is.
    Packed,
    Jpeg,
}

impl Class {
    pub fn label(self) -> &'static str {
        match self {
            Class::Exe => "Programs",
            Class::Text => "Text and scripts",
            Class::Audio(_) => "Raw audio",
            Class::Data => "Game data",
            Class::Packed => "Compressed media",
            Class::Jpeg => "JPEG images",
        }
    }
}

const TEXT: &[&str] = &[
    "txt", "json", "xml", "ini", "cfg", "conf", "config", "csv", "tsv", "yaml", "yml", "toml", "md", "html", "htm",
    "css", "js", "ts", "lua", "py", "rb", "cs", "c", "h", "cpp", "hpp", "hlsl", "glsl", "fx", "shader", "vert", "frag",
    "vdf", "vmt", "res", "acf", "log", "po", "pot", "strings", "loc", "lang", "sql", "bat", "cmd", "ps1", "sh", "svg",
];

const PACKED: &[&str] = &[
    "ogg", "oga", "opus", "mp3", "m4a", "aac", "wma", "flac", "mp4", "m4v", "webm", "mkv", "avi", "mov", "wmv", "bik",
    "bk2", "usm", "ogv", "mpg", "mpeg", "7z", "rar", "xz", "bz2", "zst", "lz4", "cab", "webp", "avif", "heic", "jxl",
];

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// x86 or x64 native code. .NET assemblies carry bytecode, which the filter would only scramble.
fn is_native_pe(head: &[u8]) -> bool {
    if head.len() < 0x40 || &head[..2] != b"MZ" {
        return false;
    }
    let pe = u32::from_le_bytes(head[0x3c..0x40].try_into().unwrap()) as usize;
    let Some(header) = head.get(pe..pe + 24) else { return false };
    if &header[..4] != b"PE\0\0" {
        return false;
    }
    let machine = u16::from_le_bytes([header[4], header[5]]);
    if machine != 0x14c && machine != 0x8664 {
        return false;
    }
    let optional = pe + 24;
    let Some(magic) = head.get(optional..optional + 2) else { return false };
    let directories = optional + if magic == [0x0b, 0x02] { 112 } else { 96 };
    let clr = directories + 14 * 8;
    match head.get(clr..clr + 4) {
        Some(rva) => rva == [0, 0, 0, 0],
        None => true,
    }
}

/// The size of one sample frame, for uncompressed PCM.
fn wav_frame(head: &[u8]) -> Option<u8> {
    if head.len() < 44 || &head[..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return None;
    }
    let mut at = 12usize;
    while at + 8 <= head.len() {
        let id = &head[at..at + 4];
        let len = u32::from_le_bytes(head[at + 4..at + 8].try_into().ok()?) as usize;
        if id == b"fmt " {
            let format = u16::from_le_bytes(head.get(at + 8..at + 10)?.try_into().ok()?);
            let align = u16::from_le_bytes(head.get(at + 20..at + 22)?.try_into().ok()?);
            return (matches!(format, 1 | 0xfffe) && (1..=16).contains(&align)).then_some(align as u8);
        }
        at = at.checked_add(8 + len + (len & 1))?;
    }
    None
}

fn looks_like_text(sample: &[u8]) -> bool {
    if sample.is_empty() {
        return false;
    }
    let odd = sample.iter().filter(|&&b| b == 0 || (b < 9) || (b > 13 && b < 32)).count();
    odd * 100 < sample.len()
}

/// Decides a file's class from its name and its first bytes.
pub fn classify(path: &Path, size: u64) -> Class {
    let ext = extension(path);
    let Ok(mut file) = File::open(crate::thaw::long_path(path)) else { return Class::Data };
    let mut head = vec![0u8; 4096];
    let n = file.read(&mut head).unwrap_or(0);
    head.truncate(n);

    if head.starts_with(&[0xff, 0xd8, 0xff]) {
        return Class::Jpeg;
    }
    if head.starts_with(b"MZ") {
        return if is_native_pe(&head) { Class::Exe } else { Class::Data };
    }
    if let Some(frame) = wav_frame(&head) {
        return Class::Audio(frame);
    }
    if head.starts_with(b"OggS") || PACKED.contains(&ext.as_str()) {
        return Class::Packed;
    }
    if TEXT.contains(&ext.as_str()) && looks_like_text(&head) {
        return Class::Text;
    }
    if size >= 4096 && ext.is_empty() && looks_like_text(&head) {
        return Class::Text;
    }
    Class::Data
}
