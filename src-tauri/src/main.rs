#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod icon;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use kryo::format::{Assets, Game, Image, Redist};
use kryo::freeze::{self, Embed, Estimate, Level, Options, Report};
use kryo::progress::{Cancelled, Progress};
use kryo::scan::{self, ComponentRule, Inspection};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{Manager, State};

/// The setup, embedded so the app is one file.
const SETUP: &[u8] = include_bytes!("../stub/kryoto-setup.exe");

const COVER_W: u32 = 744;
const COVER_H: u32 = 1200;

#[derive(Default)]
struct Running {
    progress: Option<Arc<Progress>>,
    started: Option<Instant>,
    outcome: Option<Result<Report, String>>,
    cancelled: bool,
}

#[derive(Default)]
struct App {
    job: Arc<Mutex<Running>>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RedistJob {
    name: String,
    path: String,
    args: String,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Job {
    source: String,
    out_dir: String,
    title: String,
    version: String,
    repacker: String,
    website: String,
    notes: String,
    exe: String,
    args: String,
    level: Level,
    memory_gb: u64,
    threads: usize,
    split_mb: Option<u64>,
    embed: Embed,
    exclude: Vec<String>,
    components: Vec<ComponentRule>,
    redists: Vec<RedistJob>,
    /// A file on disk, or a `data:` URL from a kryo.to lookup.
    cover: Option<String>,
    /// `game`, `kryoto`, or the path of an .ico.
    icon: String,
    accent: [u8; 3],
    /// `tune`, `none`, or the path of a .wav.
    music: String,
    test: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    running: bool,
    stage: &'static str,
    done: u64,
    total: u64,
    packed: u64,
    original: u64,
    repeated: u64,
    blocks_done: u64,
    blocks_total: u64,
    detail: String,
    paused: bool,
    seconds: f64,
    cancelled: bool,
    report: Option<Report>,
    error: Option<String>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn decode_image(cover: &str) -> Result<image::DynamicImage> {
    let bytes = match cover.split_once(";base64,") {
        Some((_, data)) if cover.starts_with("data:") => base64::engine::general_purpose::STANDARD.decode(data)?,
        _ => std::fs::read(cover).context("reading the cover")?,
    };
    image::load_from_memory(&bytes).context("That image could not be read.")
}

/// Scales the cover to fill the setup's art panel at twice its size.
fn cover_for_setup(cover: &str) -> Result<Image> {
    let scaled = decode_image(cover)?.resize_to_fill(COVER_W, COVER_H, image::imageops::FilterType::Lanczos3);
    let rgba = scaled.to_rgba8();
    Ok(Image { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() })
}

/// 16-bit PCM at 44.1 kHz, the one format the setup plays without a decoder.
fn read_wav(path: &Path) -> Result<Vec<i16>> {
    let data = std::fs::read(path).context("reading the music")?;
    if data.len() < 44 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        bail!("Setup music must be a .wav file.");
    }
    let (mut channels, mut rate, mut bits) = (0u16, 0u32, 0u16);
    let mut at = 12usize;
    while at + 8 <= data.len() {
        let id = &data[at..at + 4];
        let len = u32::from_le_bytes(data[at + 4..at + 8].try_into()?) as usize;
        let body = at + 8;
        if id == b"fmt " {
            channels = u16::from_le_bytes(data[body + 2..body + 4].try_into()?);
            rate = u32::from_le_bytes(data[body + 4..body + 8].try_into()?);
            bits = u16::from_le_bytes(data[body + 14..body + 16].try_into()?);
        } else if id == b"data" {
            if rate != 44_100 || bits != 16 || !(1..=2).contains(&channels) {
                bail!("Setup music must be 16-bit, 44.1 kHz, mono or stereo.");
            }
            let end = (body + len).min(data.len());
            let mut samples = Vec::with_capacity((end - body) / 2 * (3 - channels as usize));
            for pair in data[body..end].as_chunks::<2>().0 {
                let value = i16::from_le_bytes([pair[0], pair[1]]);
                samples.push(value);
                if channels == 1 {
                    samples.push(value);
                }
            }
            return Ok(samples);
        }
        at = body + len + (len & 1);
    }
    bail!("That .wav has no audio in it.")
}

fn stub_for(job: &Job) -> Result<Vec<u8>> {
    let source = match job.icon.as_str() {
        "kryoto" => None,
        "game" if job.exe.is_empty() => None,
        "game" => Some(Path::new(&job.source).join(&job.exe)),
        path => Some(PathBuf::from(path)),
    };
    let Some(source) = source else { return Ok(SETUP.to_vec()) };
    std::fs::create_dir_all(&job.out_dir)?;
    let temp = Path::new(&job.out_dir).join(".permafrost-setup.tmp.exe");
    std::fs::write(&temp, SETUP)?;
    let applied = icon::apply(&temp, &source);
    let bytes = std::fs::read(&temp);
    let _ = std::fs::remove_file(&temp);
    match applied {
        Ok(()) => Ok(bytes?),
        // A game whose program carries no icon still gets a setup.
        Err(_) if job.icon == "game" => Ok(SETUP.to_vec()),
        Err(e) => Err(e),
    }
}

fn options_for(job: &Job) -> Result<Options> {
    let assets = Assets {
        cover: job.cover.as_deref().filter(|c| !c.is_empty()).map(cover_for_setup).transpose()?,
        music: match job.music.as_str() {
            "tune" | "none" => None,
            path => Some(read_wav(Path::new(path))?),
        },
        accent: job.accent,
        tune: job.music != "none",
    };
    Ok(Options {
        source: job.source.clone().into(),
        out_dir: job.out_dir.clone().into(),
        name: job.title.clone(),
        game: Game {
            title: job.title.clone(),
            version: job.version.clone(),
            repacker: job.repacker.clone(),
            website: job.website.clone(),
            notes: job.notes.clone(),
            exe: job.exe.clone(),
            args: job.args.clone(),
        },
        level: job.level,
        memory: job.memory_gb << 30,
        threads: job.threads,
        split: job.split_mb.map(|m| m << 20),
        embed: job.embed,
        exclude: job.exclude.clone(),
        components: job.components.clone(),
        redists: job
            .redists
            .iter()
            .map(|r| Redist { name: r.name.clone(), path: r.path.clone(), args: r.args.clone() })
            .collect(),
        assets,
        stub: Some(stub_for(job)?),
        test: job.test,
    })
}

#[tauri::command]
async fn pick_folder(title: String) -> Option<String> {
    rfd::AsyncFileDialog::new()
        .set_title(title)
        .pick_folder()
        .await
        .map(|f| f.path().display().to_string())
}

#[tauri::command]
async fn pick_file(title: String, kinds: Vec<String>) -> Option<String> {
    let kinds: Vec<&str> = kinds.iter().map(String::as_str).collect();
    rfd::AsyncFileDialog::new()
        .set_title(title)
        .add_filter("Files", &kinds)
        .pick_file()
        .await
        .map(|f| f.path().display().to_string())
}

#[tauri::command]
async fn inspect(path: String) -> Result<Inspection, String> {
    tauri::async_runtime::spawn_blocking(move || scan::inspect(Path::new(&path)))
        .await
        .map_err(err)?
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn estimate(path: String, exclude: Vec<String>, level: Level) -> Result<Estimate, String> {
    tauri::async_runtime::spawn_blocking(move || {
        freeze::estimate(Path::new(&path), &exclude, level, &Progress::new())
    })
    .await
    .map_err(err)?
    .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn start_freeze(job: Job, app: State<App>) -> Result<(), String> {
    let shared = app.job.clone();
    let mut slot = shared.lock().unwrap();
    if slot.progress.is_some() && slot.outcome.is_none() && !slot.cancelled {
        return Err("A freeze is already running.".into());
    }
    let progress = Arc::new(Progress::new());
    *slot = Running { progress: Some(progress.clone()), started: Some(Instant::now()), ..Default::default() };
    drop(slot);
    let result_slot = app.job.clone();
    std::thread::Builder::new()
        .name("freeze".into())
        .spawn(move || {
            let result = options_for(&job).and_then(|options| freeze::freeze(&options, &progress));
            let mut slot = result_slot.lock().unwrap();
            match result {
                Ok(report) => slot.outcome = Some(Ok(report)),
                Err(e) if e.downcast_ref::<Cancelled>().is_some() => slot.cancelled = true,
                Err(e) => slot.outcome = Some(Err(format!("{e:#}"))),
            }
        })
        .map_err(err)?;
    Ok(())
}

#[tauri::command]
fn freeze_status(app: State<App>) -> Status {
    let slot = app.job.lock().unwrap();
    let progress = slot.progress.as_deref();
    let read = |f: fn(&Progress) -> u64| progress.map(f).unwrap_or(0);
    Status {
        running: progress.is_some() && slot.outcome.is_none() && !slot.cancelled,
        stage: progress.map(|p| p.stage().id()).unwrap_or("idle"),
        done: read(|p| p.done.load(Ordering::Relaxed)),
        total: read(|p| p.total.load(Ordering::Relaxed)),
        packed: read(|p| p.packed.load(Ordering::Relaxed)),
        original: read(|p| p.original.load(Ordering::Relaxed)),
        repeated: read(|p| p.repeated.load(Ordering::Relaxed)),
        blocks_done: read(|p| p.blocks_done.load(Ordering::Relaxed)),
        blocks_total: read(|p| p.blocks_total.load(Ordering::Relaxed)),
        detail: progress.map(Progress::detail).unwrap_or_default(),
        paused: progress.is_some_and(Progress::is_paused),
        seconds: slot.started.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0),
        cancelled: slot.cancelled,
        report: slot.outcome.as_ref().and_then(|o| o.as_ref().ok().cloned()),
        error: slot.outcome.as_ref().and_then(|o| o.as_ref().err().cloned()),
    }
}

#[tauri::command]
fn freeze_pause(paused: bool, app: State<App>) {
    if let Some(progress) = &app.job.lock().unwrap().progress {
        progress.set_paused(paused);
    }
}

#[tauri::command]
fn freeze_stop(app: State<App>) {
    if let Some(progress) = &app.job.lock().unwrap().progress {
        progress.set_paused(false);
        progress.cancel();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Machine {
    memory_gb: u64,
    threads: usize,
    documents: String,
}

fn total_memory() -> u64 {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: `status` is sized as the call requires.
    match unsafe { GlobalMemoryStatusEx(&mut status) } {
        Ok(()) => status.ullTotalPhys,
        Err(_) => 8 << 30,
    }
}

#[tauri::command]
fn machine(app: tauri::AppHandle) -> Machine {
    Machine {
        memory_gb: (total_memory() + (1 << 29)) >> 30,
        threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
        documents: app.path().document_dir().map(|p| p.join("Permafrost").display().to_string()).unwrap_or_default(),
    }
}

fn state_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(err)?;
    std::fs::create_dir_all(&dir).map_err(err)?;
    Ok(dir.join("state.json"))
}

/// Settings and the log of past repacks.
#[tauri::command]
fn load_state(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    match std::fs::read(state_path(&app)?) {
        Ok(data) => serde_json::from_slice(&data).map_err(err),
        Err(_) => Ok(serde_json::json!({})),
    }
}

#[tauri::command]
fn save_state(state: serde_json::Value, app: tauri::AppHandle) -> Result<(), String> {
    std::fs::write(state_path(&app)?, serde_json::to_vec_pretty(&state).map_err(err)?).map_err(err)
}

#[tauri::command]
fn preview_image(path: String) -> Result<String, String> {
    let picture = decode_image(&path).map_err(|e| format!("{e:#}"))?.thumbnail(372, 600).to_rgb8();
    let mut out = std::io::Cursor::new(Vec::new());
    picture.write_to(&mut out, image::ImageFormat::Jpeg).map_err(err)?;
    let body = base64::engine::general_purpose::STANDARD.encode(out.into_inner());
    Ok(format!("data:image/jpeg;base64,{body}"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Found {
    slug: String,
    name: String,
    version: String,
    cover: String,
    exe: String,
    args: String,
    nsfw: bool,
    download_size: String,
}

fn field(value: &serde_json::Value, names: &[&str]) -> String {
    names.iter().find_map(|n| value.get(*n)?.as_str()).unwrap_or_default().to_string()
}

/// Looks a game up on kryo.to, to fill in its name and art.
#[tauri::command]
async fn find_on_kryoto(query: String) -> Result<Vec<Found>, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<Found>> {
        let response: serde_json::Value = ureq::get("https://kryo.to/api/games")
            .query("q", &query)
            .query("limit", "6")
            .set("User-Agent", "kryoto-permafrost")
            .call()
            .map_err(|e| anyhow!("kryo.to did not answer: {e}"))?
            .into_json()?;
        let games = response.get("games").and_then(|g| g.as_array()).cloned().unwrap_or_default();
        Ok(games
            .iter()
            .map(|g| Found {
                slug: field(g, &["slug"]),
                name: field(g, &["title"]),
                version: field(g, &["version"]),
                cover: [field(g, &["cover_vertical"]), field(g, &["cover"])].into_iter().find(|c| !c.is_empty()).unwrap_or_default(),
                exe: field(g, &["game_executable_path"]).replace('\\', "/"),
                args: field(g, &["game_executable_args"]),
                nsfw: g.get("nsfw").and_then(|n| n.as_bool()).unwrap_or(false),
                download_size: field(g, &["download_size"]),
            })
            .filter(|g| !g.name.is_empty())
            .collect())
    })
    .await
    .map_err(err)?
    .map_err(|e| format!("{e:#}"))
}

/// Downloads a cover found on kryo.to, as a `data:` URL the job can carry.
#[tauri::command]
async fn fetch_cover(url: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<String> {
        let parsed = url.strip_prefix("https://").context("Only https covers can be used.")?;
        let host = parsed.split('/').next().unwrap_or_default();
        if !(host == "kryo.to" || host.ends_with(".kryo.to") || host.ends_with("steamstatic.com") || host.ends_with("akamaihd.net")) {
            bail!("That cover is not on a known host.");
        }
        let mut body = Vec::new();
        let response = ureq::get(&url).set("User-Agent", "kryoto-permafrost").call().map_err(|e| anyhow!("{e}"))?;
        std::io::Read::read_to_end(&mut std::io::Read::take(response.into_reader(), 24 << 20), &mut body)?;
        let text = base64::engine::general_purpose::STANDARD.encode(body);
        Ok(format!("data:image/any;base64,{text}"))
    })
    .await
    .map_err(err)?
    .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn open_folder(path: String) -> Result<(), String> {
    std::process::Command::new("explorer.exe").arg(path).spawn().map_err(err)?;
    Ok(())
}

#[tauri::command]
fn run_setup(path: String) -> Result<(), String> {
    std::process::Command::new(&path)
        .current_dir(Path::new(&path).parent().unwrap_or(Path::new(".")))
        .spawn()
        .map_err(err)?;
    Ok(())
}

#[tauri::command]
fn read_text(path: String) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(err)
}

fn main() {
    tauri::Builder::default()
        .manage(App::default())
        .invoke_handler(tauri::generate_handler![
            pick_folder,
            pick_file,
            inspect,
            estimate,
            start_freeze,
            freeze_status,
            freeze_pause,
            freeze_stop,
            machine,
            load_state,
            save_state,
            preview_image,
            find_on_kryoto,
            fetch_cover,
            open_folder,
            run_setup,
            read_text,
        ])
        .run(tauri::generate_context!())
        .expect("starting the app");
}
