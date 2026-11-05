//! The pages of the setup, drawn fresh every frame.

use crate::art::{self, Frozen};
use crate::canvas::*;
use crate::system;
use crate::tune::Player;
use kryo::format::{Assets, Manifest};
use kryo::progress::{Cancelled, Progress, Stage};
use kryo::thaw::{self, Source, ThawOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use windows::Win32::Foundation::HWND;

pub const WIDTH: f32 = 960.0;
pub const HEIGHT: f32 = 600.0;
const ART_W: f32 = 372.0;
const PAD: f32 = 32.0;
const TOP: f32 = 44.0;

#[derive(Default, Clone)]
pub struct Input {
    pub x: f32,
    pub y: f32,
    pub down: bool,
    pub clicked: bool,
    pub released: bool,
    pub wheel: f32,
    pub tab: bool,
    pub back_tab: bool,
    pub enter: bool,
    pub escape: bool,
    pub typed: Vec<char>,
    pub backspace: bool,
    pub delete: bool,
    pub left: bool,
    pub right: bool,
    pub home: bool,
    pub end: bool,
    pub paste: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Welcome,
    Installing,
    Done,
    Failed,
    Uninstall,
    Removing,
    Removed,
}

#[derive(Default)]
pub struct Signals {
    pub close: bool,
    pub minimise: bool,
    pub drag: bool,
}

/// What the install thread reports back.
#[derive(Default)]
struct Outcome {
    finished: bool,
    error: Option<String>,
    cancelled: bool,
    elevate: bool,
    removed: f32,
}

pub struct Choices {
    pub dest: String,
    pub selected: Vec<bool>,
    pub desktop: bool,
    pub start_menu: bool,
    pub redists: bool,
    pub verify: bool,
    pub low_memory: bool,
    pub shutdown: bool,
}

pub struct App {
    pub manifest: Option<Arc<Manifest>>,
    pub source: Option<Source>,
    assets: Assets,
    pub page: Page,
    frozen: Frozen,
    pub choices: Choices,
    accent: Color,
    started: Instant,
    pub progress: Arc<Progress>,
    outcome: Arc<Mutex<Outcome>>,
    thaw_at: f32,
    show_thaw: f32,
    speed: f32,
    last_sample: (Instant, u64),
    begun: Option<Instant>,
    took: u64,
    error: String,
    confirm_close: bool,
    scroll: f32,
    player: Option<Player>,
    hwnd: HWND,
    // Which control the keyboard is on, and which the mouse pressed.
    focus: i32,
    pressed: i32,
    next_id: i32,
    controls: i32,
    caret: usize,
    missing: Vec<usize>,
    pub uninstall: Option<(PathBuf, system::InstallInfo)>,
}

fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e12 {
        format!("{:.2} TB", b / 1e12)
    } else if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.0} KB", b / 1e3)
    }
}

fn time_text(seconds: f32) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

impl App {
    pub fn new(manifest: Option<Manifest>, source: Option<Source>, assets: Assets, muted: bool) -> Self {
        let accent = match assets.accent {
            [0, 0, 0] => 0x9fd3ee,
            [r, g, b] => (r as u32) << 16 | (g as u32) << 8 | b as u32,
        };
        let frozen = Frozen::new(assets.cover.as_ref(), ART_W, HEIGHT);
        let (mut dest, mut selected) = (String::new(), Vec::new());
        let mut missing = Vec::new();
        let mut redists = false;
        if let Some(m) = &manifest {
            selected = m.components.iter().map(|c| c.selected || c.required).collect();
            let needed = thaw::selection_size(m, &selected);
            dest = system::default_dest(&m.game.title, needed).display().to_string();
            redists = !m.redists.is_empty();
            if let Some(source) = &source {
                // A part that was not downloaded simply cannot be ticked.
                for (i, component) in m.components.iter().enumerate() {
                    let alone: Vec<bool> = (0..m.components.len()).map(|j| j == i).collect();
                    if !component.required && !thaw::missing_volumes(m, source, &alone).is_empty() {
                        selected[i] = false;
                        missing.push(i);
                    }
                }
            }
        }
        let music = assets.music.clone();
        let player = (assets.tune || music.is_some()).then(|| Player::start(music, muted));
        App {
            manifest: manifest.map(Arc::new),
            source,
            assets,
            page: Page::Welcome,
            frozen,
            choices: Choices {
                dest,
                selected,
                desktop: true,
                start_menu: true,
                redists,
                verify: true,
                low_memory: false,
                shutdown: false,
            },
            accent,
            started: Instant::now(),
            progress: Arc::new(Progress::new()),
            outcome: Arc::new(Mutex::new(Outcome::default())),
            thaw_at: 0.0,
            show_thaw: 0.0,
            speed: 0.0,
            last_sample: (Instant::now(), 0),
            begun: None,
            took: 0,
            error: String::new(),
            confirm_close: false,
            scroll: 0.0,
            player,
            hwnd: HWND::default(),
            focus: -1,
            pressed: -1,
            next_id: 0,
            controls: 0,
            caret: usize::MAX,
            missing,
            uninstall: None,
        }
    }

    pub fn set_window(&mut self, hwnd: HWND) {
        self.hwnd = hwnd;
    }

    fn id(&mut self) -> i32 {
        self.next_id += 1;
        self.next_id
    }

    fn over(input: &Input, x: f32, y: f32, w: f32, h: f32) -> bool {
        input.x >= x && input.x < x + w && input.y >= y && input.y < y + h
    }

    /// Mouse and keyboard handling every control shares. True when it was activated.
    fn interact(&mut self, input: &Input, id: i32, x: f32, y: f32, w: f32, h: f32) -> bool {
        let hover = Self::over(input, x, y, w, h);
        if input.clicked && hover {
            self.pressed = id;
            self.focus = id;
        }
        let mut fired = input.released && self.pressed == id && hover;
        if self.focus == id && (input.enter || input.typed.contains(&' ')) {
            fired = true;
        }
        fired
    }

    fn focus_ring(&self, c: &mut Canvas, id: i32, x: f32, y: f32, w: f32, h: f32, radius: f32) {
        if self.focus == id {
            c.rounded(x - 3.0, y - 3.0, w + 6.0, h + 6.0, radius + 3.0, INK, 1.0, Some(1.5));
        }
    }

    fn button(&mut self, c: &mut Canvas, input: &Input, x: f32, y: f32, w: f32, label: &str, primary: bool, enabled: bool) -> bool {
        let id = self.id();
        let h = 40.0;
        let hover = enabled && Self::over(input, x, y, w, h);
        let fired = enabled && self.interact(input, id, x, y, w, h);
        let (fill, text) = match (primary, enabled) {
            (true, true) => (if hover { 0xffffff } else { INK }, BG),
            (true, false) => (0x2a2a2a, 0x6c6c6c),
            (false, _) => (if hover { 0x242424 } else { 0x181818 }, if enabled { INK } else { 0x6c6c6c }),
        };
        c.rounded(x, y, w, h, 20.0, fill, 1.0, None);
        if !primary {
            c.rounded(x, y, w, h, 20.0, LINE, 1.0, Some(1.0));
        }
        let tw = c.text_width(label, 13.0, Face::Bold);
        c.text(x + (w - tw) / 2.0, y + 13.0, label, 13.0, Face::Bold, text);
        self.focus_ring(c, id, x, y, w, h, 20.0);
        fired
    }

    fn checkbox(&mut self, c: &mut Canvas, input: &Input, x: f32, y: f32, w: f32, label: &str, note: &str, on: &mut bool, enabled: bool) {
        let id = self.id();
        let hover = enabled && Self::over(input, x, y - 2.0, w, 22.0);
        if enabled && self.interact(input, id, x, y - 2.0, w, 22.0) {
            *on = !*on;
        }
        c.rect(x, y, 16.0, 16.0, if hover { 0x262626 } else { 0x181818 }, 1.0);
        for (bx, by, bw, bh) in [(0.0, 0.0, 16.0, 1.0), (0.0, 15.0, 16.0, 1.0), (0.0, 0.0, 1.0, 16.0), (15.0, 0.0, 1.0, 16.0)] {
            c.rect(x + bx, y + by, bw, bh, if *on { self.accent } else { 0x4a4a4a }, 1.0);
        }
        if *on {
            c.rect(x + 4.0, y + 4.0, 8.0, 8.0, self.accent, 1.0);
        }
        let color = if enabled { INK } else { 0x6c6c6c };
        c.text(x + 26.0, y + 1.0, label, 13.0, Face::Regular, color);
        if !note.is_empty() {
            let at = c.text_width(label, 13.0, Face::Regular) + 40.0;
            let fitted = c.fit(note, 12.0, Face::Regular, w - at, false);
            c.text(x + at, y + 2.0, &fitted, 12.0, Face::Regular, MUTED);
        }
        self.focus_ring(c, id, x, y, 16.0, 16.0, 0.0);
    }

    fn path_field(&mut self, c: &mut Canvas, input: &Input, x: f32, y: f32, w: f32) {
        let id = self.id();
        let h = 40.0;
        if input.clicked && Self::over(input, x, y, w, h) {
            self.focus = id;
            self.caret = usize::MAX;
        }
        let focused = self.focus == id;
        let mut chars: Vec<char> = self.choices.dest.chars().collect();
        self.caret = self.caret.min(chars.len());
        if focused {
            for &ch in input.typed.iter().filter(|c| !c.is_control()) {
                chars.insert(self.caret, ch);
                self.caret += 1;
            }
            if let Some(text) = &input.paste {
                for ch in text.chars().filter(|c| !c.is_control()) {
                    chars.insert(self.caret, ch);
                    self.caret += 1;
                }
            }
            if input.backspace && self.caret > 0 {
                self.caret -= 1;
                chars.remove(self.caret);
            }
            if input.delete && self.caret < chars.len() {
                chars.remove(self.caret);
            }
            if input.left {
                self.caret = self.caret.saturating_sub(1);
            }
            if input.right {
                self.caret = (self.caret + 1).min(chars.len());
            }
            if input.home {
                self.caret = 0;
            }
            if input.end {
                self.caret = chars.len();
            }
            self.choices.dest = chars.iter().collect();
        }
        c.rounded(x, y, w, h, 12.0, 0x141414, 1.0, None);
        c.rounded(x, y, w, h, 12.0, if focused { MUTED } else { LINE }, 1.0, Some(1.0));
        let text = c.fit(&self.choices.dest, 13.0, Face::Regular, w - 28.0, true);
        c.text(x + 14.0, y + 13.0, &text, 13.0, Face::Regular, INK);
        if focused && (self.seconds() * 2.0) as i32 % 2 == 0 {
            let before: String = chars[..self.caret].iter().collect();
            let at = if text == self.choices.dest { c.text_width(&before, 13.0, Face::Regular) } else { w - 28.0 };
            c.rect(x + 14.0 + at, y + 11.0, 1.5, 18.0, self.accent, 1.0);
        }
    }

    fn icon_button(&mut self, c: &mut Canvas, input: &Input, x: f32, kind: u8) -> bool {
        let id = self.id();
        let (y, s) = (6.0, 30.0);
        let hover = Self::over(input, x, y, s, s);
        let fired = self.interact(input, id, x, y, s, s);
        if hover {
            c.rounded(x, y, s, s, 8.0, 0x262626, 1.0, None);
        }
        let ink = if hover { INK } else { MUTED };
        let (ox, oy) = (x + 9.0, y + 9.0);
        // Pixel icons on a 2-unit grid, like the rest of Kryoto's.
        let px: &[(i32, i32)] = match kind {
            0 => &[(0, 0), (1, 1), (2, 2), (3, 3), (4, 4), (5, 5), (6, 6), (6, 0), (5, 1), (4, 2), (2, 4), (1, 5), (0, 6)],
            1 => &[(0, 5), (1, 5), (2, 5), (3, 5), (4, 5), (5, 5), (6, 5)],
            2 => &[(0, 2), (0, 3), (0, 4), (1, 2), (1, 3), (1, 4), (2, 1), (2, 2), (2, 3), (2, 4), (2, 5), (3, 0), (3, 1), (3, 2), (3, 3), (3, 4), (3, 5), (3, 6), (5, 2), (5, 4), (6, 1), (6, 3), (6, 5)],
            _ => &[(0, 2), (0, 3), (0, 4), (1, 2), (1, 3), (1, 4), (2, 1), (2, 2), (2, 3), (2, 4), (2, 5), (3, 0), (3, 1), (3, 2), (3, 3), (3, 4), (3, 5), (3, 6), (5, 2), (6, 3), (5, 4), (6, 1), (6, 5)],
        };
        for &(gx, gy) in px {
            c.rect(ox + gx as f32 * 2.0, oy + gy as f32 * 2.0, 2.0, 2.0, ink, 1.0);
        }
        self.focus_ring(c, id, x, y, s, s, 8.0);
        fired
    }

    fn seconds(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    fn title_bar(&mut self, c: &mut Canvas, input: &Input, signals: &mut Signals) {
        let right = WIDTH - 8.0;
        if self.icon_button(c, input, right - 30.0, 0) {
            self.ask_close(signals);
        }
        if self.icon_button(c, input, right - 64.0, 1) {
            signals.minimise = true;
        }
        if let Some(player) = &self.player {
            let muted = player.muted.clone();
            let kind = if muted.load(Ordering::Relaxed) { 3 } else { 2 };
            if self.icon_button(c, input, right - 98.0, kind) {
                muted.store(!muted.load(Ordering::Relaxed), Ordering::Relaxed);
            }
        }
        if input.clicked && input.y < TOP - 2.0 && input.x < WIDTH - 110.0 {
            signals.drag = true;
        }
    }

    fn ask_close(&mut self, signals: &mut Signals) {
        if matches!(self.page, Page::Installing | Page::Removing) {
            self.confirm_close = true;
        } else {
            signals.close = true;
        }
    }

    fn start_install(&mut self) {
        let Some(manifest) = self.manifest.clone() else { return };
        let Some(source) = self.source.clone() else { return };
        let dest = PathBuf::from(self.choices.dest.trim());
        let selected = self.choices.selected.clone();
        let (_, available) = system::memory();
        let low = self.choices.low_memory;
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
        let options = ThawOptions {
            dest: dest.clone(),
            source,
            selected: selected.clone(),
            threads: if low { 1 } else { threads.saturating_sub(1).max(1) },
            memory: if low { 0 } else { available / 10 * 6 },
            verify: self.choices.verify,
        };
        let progress = self.progress.clone();
        let outcome = self.outcome.clone();
        let (desktop, start_menu, redists) = (self.choices.desktop, self.choices.start_menu, self.choices.redists);
        self.page = Page::Installing;
        self.begun = Some(Instant::now());
        self.last_sample = (Instant::now(), 0);
        std::thread::spawn(move || {
            let result = run_install(&manifest, &options, &progress, desktop, start_menu, redists);
            let mut done = outcome.lock().unwrap();
            match result {
                Ok(()) => done.finished = true,
                Err(e) if e.downcast_ref::<Cancelled>().is_some() => done.cancelled = true,
                Err(e) => done.error = Some(format!("{e:#}")),
            }
        });
    }

    /// The share of the whole job done, for the bar and the melt.
    fn overall(&self) -> f32 {
        let done = self.progress.done.load(Ordering::Relaxed) as f32;
        let total = self.progress.total.load(Ordering::Relaxed).max(1) as f32;
        let part = (done / total).clamp(0.0, 1.0);
        let verify = self.choices.verify;
        let (from, span) = match self.progress.stage() {
            Stage::Unpacking => (0.0, if verify { 0.82 } else { 0.96 }),
            Stage::Linking => (if verify { 0.82 } else { 0.96 }, 0.02),
            Stage::Verifying => (0.84, 0.14),
            Stage::Finishing => (0.98, 0.02),
            _ => (0.0, 0.0),
        };
        from + span * part
    }

    fn welcome(&mut self, c: &mut Canvas, input: &Input) {
        let Some(manifest) = self.manifest.clone() else { return };
        let x = ART_W + PAD;
        let w = WIDTH - x - PAD;
        let mut y = 46.0;

        let title = c.fit(&manifest.game.title, 26.0, Face::Bold, w, false);
        c.text(x, y, &title, 26.0, Face::Bold, INK);
        y += 36.0;
        let mut by = String::new();
        if !manifest.game.version.is_empty() {
            by.push_str(&manifest.game.version);
            by.push_str("  ·  ");
        }
        if manifest.game.repacker.is_empty() {
            by.push_str("Kryoto repack");
        } else {
            by.push_str("Repacked by ");
            by.push_str(&manifest.game.repacker);
        }
        c.text(x, y, &by, 13.0, Face::Regular, MUTED);
        y += 30.0;

        let needed = thaw::selection_size(&manifest, &self.choices.selected);
        let packed = manifest.stats.packed.max(1);
        let original = manifest.stats.original.max(1) as f32;
        let stats = [
            ("INSTALLED", size_text(needed)),
            ("DOWNLOAD", size_text(packed)),
            ("RATIO", format!("{:.1}%", packed as f32 * 100.0 / original)),
            ("SAVED", size_text(manifest.stats.original.saturating_sub(packed))),
        ];
        for (i, (label, value)) in stats.iter().enumerate() {
            let sx = x + i as f32 * (w / 4.0);
            c.text(sx, y, label, 10.0, Face::Regular, MUTED);
            c.text(sx, y + 15.0, value, 16.0, Face::Bold, INK);
        }
        y += 54.0;

        c.text(x, y, "Install to", 12.0, Face::Regular, MUTED);
        y += 20.0;
        self.path_field(c, input, x, y, w - 104.0);
        if self.button(c, input, x + w - 92.0, y, 92.0, "Browse", false, true) {
            let start = PathBuf::from(&self.choices.dest);
            if let Some(mut picked) = system::pick_folder(self.hwnd, &start) {
                let folder = system::safe_name(&manifest.game.title);
                if !picked.ends_with(&folder) {
                    picked.push(folder);
                }
                self.choices.dest = picked.display().to_string();
                self.caret = usize::MAX;
            }
        }
        y += 48.0;
        let dest = PathBuf::from(self.choices.dest.trim());
        let free = system::free_space(&dest);
        let enough = free.is_none_or(|f| f >= needed);
        let valid = dest.is_absolute() && self.choices.dest.trim().len() > 3;
        let space = match free {
            Some(f) if !enough => format!("Not enough room: {} free, {} needed.", size_text(f), size_text(needed)),
            Some(f) => format!("{} free on this drive", size_text(f)),
            None => "Pick a drive that exists.".to_string(),
        };
        c.text(x, y, &space, 12.0, Face::Regular, if enough && free.is_some() { MUTED } else { DANGER });
        y += 28.0;

        // Optional parts scroll if there are many of them.
        let optional: Vec<usize> = (0..manifest.components.len())
            .filter(|&i| !manifest.components[i].required && manifest.components[i].size > 0)
            .collect();
        let list_bottom = HEIGHT - 142.0;
        if !optional.is_empty() {
            c.text(x, y, "Parts", 12.0, Face::Regular, MUTED);
            y += 20.0;
            let height = (list_bottom - y).max(24.0);
            let content = optional.len() as f32 * 24.0;
            if Self::over(input, x, y, w, height) {
                self.scroll = (self.scroll - input.wheel * 24.0).clamp(0.0, (content - height).max(0.0));
            }
            c.clip(x - 4.0, y - 3.0, w + 8.0, height + 3.0);
            let mut row = y - self.scroll;
            for &index in &optional {
                let component = manifest.components[index].clone();
                let unavailable = self.missing.contains(&index);
                let note = if unavailable {
                    "not downloaded".to_string()
                } else {
                    format!("{}  {}", size_text(component.size), component.note)
                };
                let mut on = self.choices.selected[index];
                self.checkbox(c, input, x, row + 2.0, w, &component.name, &note, &mut on, !unavailable);
                self.choices.selected[index] = on;
                row += 24.0;
            }
            c.unclip();
            y += height + 6.0;
        }

        let column = w / 2.0;
        let mut y2 = HEIGHT - 134.0;
        let has_redists = !manifest.redists.is_empty();
        let (mut desktop, mut start_menu) = (self.choices.desktop, self.choices.start_menu);
        let (mut verify, mut low) = (self.choices.verify, self.choices.low_memory);
        self.checkbox(c, input, x, y2, column, "Desktop shortcut", "", &mut desktop, true);
        self.checkbox(c, input, x + column, y2, column, "Check files after", "", &mut verify, true);
        y2 += 24.0;
        self.checkbox(c, input, x, y2, column, "Start menu shortcut", "", &mut start_menu, true);
        self.checkbox(c, input, x + column, y2, column, "Use less memory", "", &mut low, true);
        self.choices.desktop = desktop;
        self.choices.start_menu = start_menu;
        self.choices.verify = verify;
        self.choices.low_memory = low;
        let _ = (y, has_redists);

        let resumable = thaw::can_resume(&dest, &manifest);
        let label = if resumable { "Continue install" } else { "Install" };
        let ready = enough && valid && free.is_some();
        let by = HEIGHT - 66.0;
        if self.button(c, input, x, by, w, label, true, ready) {
            if system::can_write(&dest) {
                self.start_install();
            } else {
                let arg = |on: bool, flag: &str| if on { format!(" {flag}") } else { String::new() };
                let parts: Vec<String> = self.choices.selected.iter().map(|&b| (b as u8).to_string()).collect();
                let args = format!(
                    "--auto \"--dir={}\" --parts={}{}{}{}{}",
                    self.choices.dest.trim(),
                    parts.join(""),
                    arg(self.choices.desktop, "--desktop"),
                    arg(self.choices.start_menu, "--start-menu"),
                    arg(self.choices.verify, "--verify"),
                    arg(self.choices.low_memory, "--low-memory"),
                );
                match system::run_elevated(&args, false) {
                    Ok(_) => self.outcome.lock().unwrap().elevate = true,
                    Err(e) => {
                        self.error = format!("{e:#}");
                        self.page = Page::Failed;
                    }
                }
            }
        }
    }

    fn installing(&mut self, c: &mut Canvas, input: &Input) {
        let x = ART_W + PAD;
        let w = WIDTH - x - PAD;
        let stage = self.progress.stage();
        let paused = self.progress.is_paused();
        let label = match stage {
            _ if paused => "Paused",
            Stage::Linking => "Rebuilding repeated data",
            Stage::Verifying => "Checking every file",
            Stage::Finishing => "Finishing up",
            _ => "Thawing",
        };
        let manifest_title = self.manifest.as_ref().map(|m| m.game.title.clone()).unwrap_or_default();
        let title = c.fit(&manifest_title, 16.0, Face::Regular, w, false);
        c.text(x, 76.0, &title, 16.0, Face::Regular, MUTED);
        c.text(x, 104.0, label, 26.0, Face::Bold, INK);

        let share = self.show_thaw;
        let percent = format!("{:.1}%", share * 100.0);
        c.text(x, 184.0, &percent, 54.0, Face::Bold, INK);
        art::bar(c, x, 258.0, w, share, self.seconds(), self.accent);

        // Speed from a smoothed sample of bytes written.
        let done = self.progress.done.load(Ordering::Relaxed);
        let total = self.progress.total.load(Ordering::Relaxed);
        let (at, before) = self.last_sample;
        let dt = at.elapsed().as_secs_f32();
        if dt >= 0.5 {
            let rate = done.saturating_sub(before) as f32 / dt;
            self.speed = if self.speed == 0.0 { rate } else { self.speed * 0.8 + rate * 0.2 };
            self.last_sample = (Instant::now(), done);
        }
        let left = if self.speed > 1.0 { total.saturating_sub(done) as f32 / self.speed } else { 0.0 };
        let elapsed = self.begun.map(|b| b.elapsed().as_secs_f32()).unwrap_or(0.0);
        let columns = [
            ("SPEED", format!("{}/s", size_text(self.speed as u64))),
            ("TIME LEFT", if left > 0.0 && !paused { time_text(left) } else { "...".into() }),
            ("ELAPSED", time_text(elapsed)),
        ];
        for (i, (name, value)) in columns.iter().enumerate() {
            let sx = x + i as f32 * (w / 3.0);
            c.text(sx, 288.0, name, 10.0, Face::Regular, MUTED);
            c.text(sx, 303.0, value, 15.0, Face::Bold, INK);
        }
        let detail = c.fit(&self.progress.detail(), 12.0, Face::Regular, w, true);
        c.text(x, 342.0, &detail, 12.0, Face::Regular, MUTED);

        let mut shutdown = self.choices.shutdown;
        self.checkbox(c, input, x, HEIGHT - 110.0, w, "Shut down this PC when it finishes", "", &mut shutdown, true);
        self.choices.shutdown = shutdown;

        let half = (w - 12.0) / 2.0;
        let by = HEIGHT - 66.0;
        if self.button(c, input, x, by, half, if paused { "Resume" } else { "Pause" }, false, true) {
            self.progress.set_paused(!paused);
        }
        if self.button(c, input, x + half + 12.0, by, half, "Stop", false, true) {
            self.confirm_close = true;
        }
    }

    fn done(&mut self, c: &mut Canvas, input: &Input, signals: &mut Signals) {
        let Some(manifest) = self.manifest.clone() else { return };
        let x = ART_W + PAD;
        let w = WIDTH - x - PAD;
        let title = c_title(c, &manifest.game.title, w);
        c.text(x, 76.0, &title, 16.0, Face::Regular, MUTED);
        c.text(x, 104.0, "Ready to play", 26.0, Face::Bold, INK);
        let stats = &manifest.stats;
        let ratio = stats.packed as f32 * 100.0 / stats.original.max(1) as f32;
        let line = format!("{} from {} ({ratio:.1}%) in {}.", size_text(stats.original), size_text(stats.packed), time_text(self.took as f32));
        c.text(x, 150.0, &line, 13.0, Face::Regular, MUTED);
        if self.choices.verify {
            c.text(x, 172.0, "Every file was checked and matches the original.", 13.0, Face::Regular, MUTED);
        }
        if self.choices.shutdown {
            c.text(x, 194.0, "This PC shuts down in a minute. Run shutdown /a to stop it.", 13.0, Face::Regular, DANGER);
        }
        let notes = manifest.game.notes.clone();
        if !notes.is_empty() {
            let lines = c.wrap(&notes, 13.0, Face::Regular, w);
            for (i, line) in lines.iter().take(8).enumerate() {
                c.text(x, 226.0 + i as f32 * 20.0, line, 13.0, Face::Regular, INK);
            }
        }
        let by = HEIGHT - 66.0;
        let has_exe = !manifest.game.exe.is_empty();
        if has_exe && self.button(c, input, x, by - 52.0, w, "Play", true, true) {
            system::launch_game(Path::new(self.choices.dest.trim()), &manifest);
            signals.close = true;
        }
        let half = (w - 12.0) / 2.0;
        if self.button(c, input, x, by, half, "Open folder", false, true) {
            system::open_folder(Path::new(self.choices.dest.trim()));
        }
        if self.button(c, input, x + half + 12.0, by, half, "Close", false, true) {
            signals.close = true;
        }
    }

    fn failed(&mut self, c: &mut Canvas, input: &Input, signals: &mut Signals) {
        let x = ART_W + PAD;
        let w = WIDTH - x - PAD;
        c.text(x, 104.0, "That didn't work", 26.0, Face::Bold, INK);
        let error = self.error.clone();
        let lines = c.wrap(&error, 13.0, Face::Regular, w);
        for (i, line) in lines.iter().take(12).enumerate() {
            c.text(x, 150.0 + i as f32 * 20.0, line, 13.0, Face::Regular, DANGER);
        }
        let by = HEIGHT - 66.0;
        let half = (w - 12.0) / 2.0;
        let can_retry = self.manifest.is_some();
        if can_retry && self.button(c, input, x, by, half, "Back", false, true) {
            self.reset_after_stop();
        }
        if self.button(c, input, x + half + 12.0, by, half, "Close", false, true) {
            signals.close = true;
        }
    }

    fn reset_after_stop(&mut self) {
        self.progress = Arc::new(Progress::new());
        *self.outcome.lock().unwrap() = Outcome::default();
        self.thaw_at = 0.0;
        self.speed = 0.0;
        self.page = Page::Welcome;
    }

    fn uninstall_page(&mut self, c: &mut Canvas, input: &Input, signals: &mut Signals) {
        let x = ART_W + PAD;
        let w = WIDTH - x - PAD;
        let Some((dir, info)) = &self.uninstall else { return };
        let (dir, title, count) = (dir.clone(), info.title.clone(), info.files.len());
        let shown_title = c_title(c, &title, w);
        c.text(x, 76.0, &shown_title, 16.0, Face::Regular, MUTED);
        c.text(x, 104.0, "Remove this game?", 26.0, Face::Bold, INK);
        c.text(x, 150.0, &format!("{count} files in"), 13.0, Face::Regular, MUTED);
        let shown = c.fit(&dir.display().to_string(), 13.0, Face::Regular, w, true);
        c.text(x, 172.0, &shown, 13.0, Face::Regular, INK);
        c.text(x, 204.0, "Saves and anything else the game wrote stay.", 13.0, Face::Regular, MUTED);
        let by = HEIGHT - 66.0;
        let half = (w - 12.0) / 2.0;
        if self.button(c, input, x, by, half, "Keep it", false, true) {
            signals.close = true;
        }
        if self.button(c, input, x + half + 12.0, by, half, "Remove", true, true) {
            self.page = Page::Removing;
            let (dir, info) = self.uninstall.take().unwrap();
            let outcome = self.outcome.clone();
            std::thread::spawn(move || {
                let progress = outcome.clone();
                let result = system::remove_install(&dir, &info, |share| {
                    progress.lock().unwrap().removed = share;
                });
                let cleanup = system::hand_off_cleanup(&dir);
                let mut done = outcome.lock().unwrap();
                match result.and(cleanup) {
                    Ok(()) => done.finished = true,
                    Err(e) => done.error = Some(format!("{e:#}")),
                }
            });
        }
    }

    fn removing(&mut self, c: &mut Canvas) {
        let x = ART_W + PAD;
        let w = WIDTH - x - PAD;
        c.text(x, 104.0, "Removing", 26.0, Face::Bold, INK);
        let share = self.outcome.lock().unwrap().removed;
        art::bar(c, x, 170.0, w, share, self.seconds(), self.accent);
    }

    fn removed(&mut self, c: &mut Canvas, input: &Input, signals: &mut Signals) {
        let x = ART_W + PAD;
        let w = WIDTH - x - PAD;
        c.text(x, 104.0, "Removed", 26.0, Face::Bold, INK);
        if self.button(c, input, x, HEIGHT - 66.0, w, "Close", true, true) {
            signals.close = true;
        }
    }

    fn dialog(&mut self, c: &mut Canvas, input: &Input, signals: &mut Signals) {
        c.rect(0.0, 0.0, WIDTH, HEIGHT, 0x000000, 0.72);
        let (w, h) = (420.0, 176.0);
        let (x, y) = ((WIDTH - w) / 2.0, (HEIGHT - h) / 2.0);
        c.rounded(x, y, w, h, 16.0, PANEL, 1.0, None);
        c.rounded(x, y, w, h, 16.0, LINE, 1.0, Some(1.0));
        c.text(x + 24.0, y + 22.0, "Stop installing?", 17.0, Face::Bold, INK);
        c.text(x + 24.0, y + 54.0, "Finished parts are kept. Run the setup again", 13.0, Face::Regular, MUTED);
        c.text(x + 24.0, y + 74.0, "and it carries on from there.", 13.0, Face::Regular, MUTED);
        let half = (w - 60.0) / 2.0;
        if self.button(c, input, x + 24.0, y + h - 60.0, half, "Keep going", true, true) || input.escape {
            self.confirm_close = false;
        }
        if self.button(c, input, x + 36.0 + half, y + h - 60.0, half, "Stop", false, true) {
            self.confirm_close = false;
            self.progress.cancel();
            signals.close = false;
        }
    }

    fn poll(&mut self, signals: &mut Signals) {
        let mut outcome = self.outcome.lock().unwrap();
        if outcome.elevate {
            signals.close = true;
        }
        if outcome.finished {
            outcome.finished = false;
            drop(outcome);
            match self.page {
                Page::Removing => self.page = Page::Removed,
                _ => {
                    self.page = Page::Done;
                    self.thaw_at = 1.0;
                    self.took = self.begun.map(|b| b.elapsed().as_secs()).unwrap_or(0);
                    if self.choices.shutdown {
                        system::shut_down_soon();
                    }
                }
            }
            return;
        }
        if let Some(error) = outcome.error.take() {
            drop(outcome);
            self.error = error;
            self.page = Page::Failed;
            return;
        }
        if outcome.cancelled {
            drop(outcome);
            self.reset_after_stop();
        }
    }

    /// Starts straight away, for the elevated second run and silent installs.
    pub fn auto_start(&mut self) {
        self.start_install();
    }

    /// One frame.
    pub fn frame(&mut self, c: &mut Canvas, input: &Input, signals: &mut Signals) {
        self.poll(signals);
        self.next_id = 0;
        if input.tab && self.controls > 0 {
            self.focus = self.focus % self.controls + 1;
        }
        if input.back_tab && self.controls > 0 {
            self.focus = if self.focus <= 1 { self.controls } else { self.focus - 1 };
        }
        if input.clicked {
            self.focus = -1;
        }

        if self.page == Page::Installing {
            self.thaw_at = self.overall();
        }
        self.show_thaw += (self.thaw_at - self.show_thaw) * 0.12;

        c.px.fill(BG);
        let time = self.seconds();
        let accent = self.accent;
        art::panel(c, &self.frozen, self.assets.cover.as_ref(), 0.0, 0.0, ART_W, HEIGHT, self.show_thaw, time, accent);
        c.rect(ART_W, 0.0, 1.0, HEIGHT, LINE, 1.0);

        art::mark(c, ART_W + PAD, 15.0, 37.0, 1.0, |_| INK);
        c.text(ART_W + PAD + 48.0, 15.0, "SETUP", 11.0, Face::Regular, MUTED);

        let modal = self.confirm_close;
        // A dialog takes the mouse; the page below still draws.
        let idle = Input { x: -1.0, y: -1.0, ..Default::default() };
        let page_input = if modal { idle } else { input.clone() };
        match self.page {
            Page::Welcome => self.welcome(c, &page_input),
            Page::Installing => self.installing(c, &page_input),
            Page::Done => self.done(c, &page_input, signals),
            Page::Failed => self.failed(c, &page_input, signals),
            Page::Uninstall => self.uninstall_page(c, &page_input, signals),
            Page::Removing => self.removing(c),
            Page::Removed => self.removed(c, &page_input, signals),
        }

        let site = self.manifest.as_ref().map(|m| m.game.website.clone()).unwrap_or_default();
        let footer_y = HEIGHT - 20.0;
        let link = if site.is_empty() { "kryo.to".to_string() } else { site.trim_start_matches("https://").trim_end_matches('/').to_string() };
        let tw = c.text_width(&link, 11.0, Face::Regular);
        let fx = WIDTH - PAD - tw;
        let id = self.id();
        let hover = !modal && Self::over(&page_input, fx, footer_y - 2.0, tw, 16.0);
        if !modal && self.interact(&page_input, id, fx, footer_y - 2.0, tw, 16.0) {
            system::open_site(if site.is_empty() { "https://kryo.to" } else { &site });
        }
        c.text(fx, footer_y, &link, 11.0, Face::Regular, if hover { INK } else { MUTED });
        c.text(ART_W + PAD, footer_y, "Permafrost", 11.0, Face::Regular, 0x5c5c5c);

        self.title_bar(c, &page_input, signals);
        if modal {
            self.dialog(c, input, signals);
        }
        if input.escape && !modal && matches!(self.page, Page::Installing) {
            self.confirm_close = true;
        }
        self.controls = self.next_id;
        if input.released {
            self.pressed = -1;
        }
    }
}

fn c_title(c: &mut Canvas, title: &str, width: f32) -> String {
    c.fit(title, 16.0, Face::Regular, width, false)
}

fn run_install(
    manifest: &Manifest,
    options: &ThawOptions,
    progress: &Progress,
    desktop: bool,
    start_menu: bool,
    redists: bool,
) -> anyhow::Result<()> {
    thaw::thaw(manifest, options, progress)?;
    let (files, _) = thaw::selection(manifest, &options.selected);
    let paths: Vec<String> = files.iter().map(|&i| manifest.files[i].path.clone()).collect();
    system::finish_install(system::Finish {
        manifest,
        dest: &options.dest,
        files: paths,
        desktop,
        start_menu,
    })?;
    let runtimes = manifest.components.iter().position(|c| c.id == "redist");
    let wanted = runtimes.is_some_and(|i| options.selected.get(i).copied().unwrap_or(false));
    if redists && wanted && !manifest.redists.is_empty() {
        progress.set_detail("Installing system runtimes");
        // Runtime installers want administrator rights. One prompt covers all of them.
        let args = format!("--redists \"--dir={}\"", options.dest.display());
        let _ = system::run_elevated(&args, true);
    }
    Ok(())
}

/// Testing and screenshots: put the setup on a page with made-up progress.
#[cfg(feature = "shots")]
impl App {
    pub fn pose(&mut self, page: Page, share: f32) {
        self.page = page;
        self.thaw_at = share;
        self.show_thaw = share;
        self.took = 754;
        self.begun = Some(Instant::now());
        self.speed = 214e6;
        if page == Page::Installing {
            self.progress.start(Stage::Unpacking, 40_000_000_000);
            self.progress.advance((share as f64 * 40e9) as u64);
            self.progress.set_detail("Content/Paks/pakchunk0-WindowsNoEditor.pak");
        }
        if page == Page::Failed {
            self.error = "Game.002.kryo is damaged. Download that file again.".into();
        }
    }
}
