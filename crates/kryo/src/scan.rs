//! Looking at a game folder before freezing it: what is in there, what can be left out,
//! which program starts the game, and which parts could be optional.

use crate::classify::{classify, Class};
use crate::format::Redist;
use anyhow::{Context, Result};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Clone, Debug)]
pub struct Found {
    pub relative: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified: u64,
    pub read_only: bool,
}

pub fn glob_set(patterns: &[String]) -> Result<GlobSet> {
    let mut set = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern.trim())
            .case_insensitive(true)
            .literal_separator(false)
            .build()
            .with_context(|| format!("the pattern {pattern}"))?;
        set.add(glob);
    }
    Ok(set.build()?)
}

/// Lists a folder's files and empty folders, leaving out what matches `exclude`.
pub fn walk(root: &Path, exclude: &[String]) -> Result<(Vec<Found>, Vec<String>)> {
    let exclude = glob_set(exclude)?;
    let mut files = Vec::new();
    let mut empty = Vec::new();
    let mut walker = walkdir::WalkDir::new(root).follow_links(false).into_iter();
    while let Some(entry) = walker.next() {
        let entry = entry.with_context(|| format!("reading {}", root.display()))?;
        if entry.depth() == 0 {
            continue;
        }
        let relative = entry.path().strip_prefix(root)?.to_string_lossy().replace('\\', "/");
        if exclude.is_match(&relative) {
            if entry.file_type().is_dir() {
                walker.skip_current_dir();
            }
            continue;
        }
        if entry.file_type().is_dir() {
            let has_children = std::fs::read_dir(entry.path())?.next().is_some();
            if !has_children {
                empty.push(relative);
            }
        } else if entry.file_type().is_file() {
            let meta = entry.metadata()?;
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            files.push(Found {
                relative,
                path: entry.path().to_path_buf(),
                size: meta.len(),
                modified,
                read_only: meta.permissions().readonly(),
            });
        }
    }
    files.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok((files, empty))
}

/// A rule that puts files into an optional part of the repack.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ComponentRule {
    pub id: String,
    pub name: String,
    pub note: String,
    pub selected: bool,
    pub patterns: Vec<String>,
    /// Filled in by `inspect`: how much the rule covers.
    #[serde(default)]
    pub size: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Share {
    pub label: String,
    pub bytes: u64,
    pub files: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LeftOut {
    pub pattern: String,
    pub label: String,
    pub bytes: u64,
    pub files: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub title: String,
    pub size: u64,
    pub files: u64,
    pub shares: Vec<Share>,
    pub programs: Vec<String>,
    pub exe: String,
    pub components: Vec<ComponentRule>,
    pub redists: Vec<SuggestedRedist>,
    pub left_out: Vec<LeftOut>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedRedist {
    pub name: String,
    pub path: String,
    pub args: String,
}

impl From<SuggestedRedist> for Redist {
    fn from(r: SuggestedRedist) -> Self {
        Redist { name: r.name, path: r.path, args: r.args }
    }
}

const REDIST_FOLDERS: &[&str] = &["_commonredist", "commonredist", "_redist", "redist", "__installer", "installers", "prereq", "prerequisites"];

const LANGUAGES: &[(&str, &str)] = &[
    ("english", "English"), ("en", "English"), ("en-us", "English"), ("en_us", "English"),
    ("french", "French"), ("fr", "French"), ("fr-fr", "French"), ("fr_fr", "French"),
    ("german", "German"), ("de", "German"), ("de-de", "German"), ("de_de", "German"),
    ("spanish", "Spanish"), ("es", "Spanish"), ("es-es", "Spanish"), ("es_es", "Spanish"),
    ("italian", "Italian"), ("it", "Italian"), ("it-it", "Italian"), ("it_it", "Italian"),
    ("japanese", "Japanese"), ("ja", "Japanese"), ("ja-jp", "Japanese"), ("ja_jp", "Japanese"),
    ("korean", "Korean"), ("ko", "Korean"), ("ko-kr", "Korean"), ("ko_kr", "Korean"),
    ("russian", "Russian"), ("ru", "Russian"), ("ru-ru", "Russian"), ("ru_ru", "Russian"),
    ("polish", "Polish"), ("pl", "Polish"), ("pl-pl", "Polish"), ("pl_pl", "Polish"),
    ("portuguese", "Portuguese"), ("pt", "Portuguese"), ("pt-br", "Portuguese"), ("pt_br", "Portuguese"),
    ("chinese", "Chinese"), ("zh", "Chinese"), ("zh-cn", "Chinese"), ("zh_cn", "Chinese"), ("zh-tw", "Chinese"),
];

const BONUS_FOLDERS: &[(&str, &str)] = &[
    ("soundtrack", "Soundtrack"), ("ost", "Soundtrack"), ("artbook", "Artbook"), ("art book", "Artbook"),
    ("bonus", "Bonus content"), ("extras", "Bonus content"), ("wallpapers", "Bonus content"),
];

/// Below this an optional part is not worth a tick box.
const MIN_OPTIONAL: u64 = 32 << 20;

const LEAVE_OUT: &[(&str, &str)] = &[
    ("**/*.pdb", "Debug symbols"),
    ("**/*.dmp", "Crash dumps"),
    ("**/*.log", "Log files"),
    ("**/*.tmp", "Temporary files"),
    ("**/Thumbs.db", "Windows thumbnails"),
    ("**/desktop.ini", "Folder settings"),
    ("**/unins*.exe", "Old uninstallers"),
    ("**/unins*.dat", "Old uninstallers"),
    ("**/steam_autocloud.vdf", "Steam cloud marker"),
];

const NOT_THE_GAME: &[&str] = &[
    "unins", "setup", "install", "redist", "dxsetup", "vc_", "vcredist", "dotnet", "ndp", "crash", "report", "prereq",
    "launcherhelper", "cefsharp", "unitycrash", "ue4prereq", "ueprereq", "easyanticheat", "beservice", "steam", "7z",
    "update", "patch", "config", "benchmark", "editor", "server",
];

fn known_redist(name: &str) -> Option<(&'static str, &'static str)> {
    let n = name.to_ascii_lowercase();
    if !n.ends_with(".exe") {
        return None;
    }
    if n.starts_with("vc_redist") {
        return Some(("Visual C++ runtime", "/install /quiet /norestart"));
    }
    if n.starts_with("vcredist") {
        return Some(("Visual C++ runtime", "/q /norestart"));
    }
    if n == "dxsetup.exe" {
        return Some(("DirectX", "/silent"));
    }
    if n.starts_with("dotnetfx") || n.starts_with("ndp") {
        return Some((".NET Framework", "/q /norestart"));
    }
    if n.starts_with("ue4prereqsetup") || n.starts_with("ueprereqsetup") {
        return Some(("Unreal Engine prerequisites", "/quiet /norestart"));
    }
    if n == "oalinst.exe" {
        return Some(("OpenAL", "/silent"));
    }
    None
}

/// `Some Game v1.2 - kryoto` becomes `Some Game`.
pub fn guess_title(folder: &Path) -> String {
    let name = folder.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut title = name.replace(['_', '.'], " ");
    for tail in [" - kryoto", "-kryoto", "-AnkerGames", " repack"] {
        if let Some(i) = title.to_ascii_lowercase().find(&tail.to_ascii_lowercase()) {
            title.truncate(i);
        }
    }
    if let Some(i) = title.find(['[', '(']) {
        title.truncate(i);
    }
    title.trim().replace('-', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn program_score(relative: &str, size: u64, title: &str) -> i64 {
    let lower = relative.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    if NOT_THE_GAME.iter().any(|w| name.contains(w)) || lower.contains("redist") {
        return i64::MIN;
    }
    let depth = lower.matches('/').count() as i64;
    let squashed: String = title.to_ascii_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
    let stem: String = name.trim_end_matches(".exe").chars().filter(|c| c.is_alphanumeric()).collect();
    let mut score = (size >> 20) as i64 - depth * 40;
    if !squashed.is_empty() && (squashed.contains(&stem) || stem.contains(&squashed)) {
        score += 500;
    }
    if name.contains("shipping") {
        score += 200;
    }
    score
}

/// Everything the app shows about a folder before freezing it.
pub fn inspect(root: &Path) -> Result<Inspection> {
    let (files, _) = walk(root, &[])?;
    let title = guess_title(root);
    let size = files.iter().map(|f| f.size).sum();

    let mut shares: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    let mut dirs: BTreeMap<String, u64> = BTreeMap::new();
    let mut programs = Vec::new();
    let mut redists = Vec::new();
    for file in &files {
        let class = classify(&file.path, file.size);
        let label = if matches!(class, Class::Jpeg) { "Images" } else { class.label() };
        let share = shares.entry(label).or_default();
        share.0 += file.size;
        share.1 += 1;
        let mut at = 0;
        while let Some(i) = file.relative[at..].find('/') {
            at += i + 1;
            *dirs.entry(file.relative[..at - 1].to_string()).or_default() += file.size;
        }
        let name = file.relative.rsplit('/').next().unwrap_or(&file.relative);
        if let Some((label, args)) = known_redist(name) {
            redists.push(SuggestedRedist { name: label.into(), path: file.relative.clone(), args: args.into() });
        } else if name.to_ascii_lowercase().ends_with(".exe") {
            programs.push((file.relative.clone(), file.size));
        }
    }

    programs.sort_by_key(|(path, size)| std::cmp::Reverse(program_score(path, *size, &title)));
    let exe = programs
        .iter()
        .find(|(path, size)| program_score(path, *size, &title) != i64::MIN)
        .or(programs.first())
        .map(|(p, _)| p.clone())
        .unwrap_or_default();

    let mut components: BTreeMap<String, ComponentRule> = BTreeMap::new();
    let mut add = |id: String, name: &str, note: &str, pattern: String, bytes: u64| {
        let rule = components.entry(id.clone()).or_insert_with(|| ComponentRule {
            id,
            name: name.into(),
            note: note.into(),
            selected: true,
            patterns: Vec::new(),
            size: 0,
        });
        rule.patterns.push(pattern);
        rule.size += bytes;
    };
    for (dir, &bytes) in &dirs {
        let base = dir.rsplit('/').next().unwrap_or(dir).to_ascii_lowercase();
        let nested = |list: &[&str]| {
            dir.split('/').rev().skip(1).any(|p| list.contains(&p.to_ascii_lowercase().as_str()))
        };
        if nested(REDIST_FOLDERS) {
            continue;
        }
        if REDIST_FOLDERS.contains(&base.as_str()) {
            add("redist".into(), "System runtimes", "Installers the game needs on a fresh PC.", format!("{dir}/**"), bytes);
        } else if bytes >= MIN_OPTIONAL {
            if let Some((_, name)) = LANGUAGES.iter().find(|(key, _)| *key == base) {
                let id = format!("lang-{}", name.to_ascii_lowercase());
                add(id, &format!("{name} language"), "Voices and text in this language.", format!("{dir}/**"), bytes);
            } else if let Some((_, name)) = BONUS_FOLDERS.iter().find(|(key, _)| *key == base) {
                let id = name.to_ascii_lowercase().replace(' ', "-");
                add(id, name, "Not needed to play.", format!("{dir}/**"), bytes);
            }
        }
    }

    let mut left_out: BTreeMap<&str, LeftOut> = BTreeMap::new();
    for (pattern, label) in LEAVE_OUT {
        let set = glob_set(&[pattern.to_string()])?;
        for file in files.iter().filter(|f| set.is_match(&f.relative)) {
            let row = left_out.entry(label).or_insert_with(|| LeftOut {
                pattern: pattern.to_string(),
                label: label.to_string(),
                bytes: 0,
                files: 0,
            });
            row.bytes += file.size;
            row.files += 1;
        }
    }

    let mut shares: Vec<Share> = shares
        .into_iter()
        .map(|(label, (bytes, files))| Share { label: label.into(), bytes, files })
        .collect();
    shares.sort_by_key(|s| std::cmp::Reverse(s.bytes));

    Ok(Inspection {
        title,
        size,
        files: files.len() as u64,
        shares,
        programs: programs.into_iter().map(|(p, _)| p).take(24).collect(),
        exe,
        components: components.into_values().collect(),
        redists,
        left_out: left_out.into_values().collect(),
    })
}
