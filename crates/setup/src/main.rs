//! The setup a player runs. One program, three jobs: it installs the repack appended to
//! it, installs quietly from the command line, and, copied into a game folder with no
//! repack attached, uninstalls that game.

#![cfg_attr(not(feature = "shots"), windows_subsystem = "windows")]
// Drawing calls take a position, a size and a look. Wrapping those in structs reads worse.
#![allow(clippy::too_many_arguments)]

mod app;
mod art;
mod canvas;
mod mark;
mod system;
mod tune;

use anyhow::{bail, Result};
use app::{App, Input, Page, Signals, HEIGHT, WIDTH};
use canvas::Canvas;
use kryo::format::{open_repack, Assets};
use kryo::progress::Progress;
use kryo::thaw::{self, Source, ThawOptions};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

struct Window {
    app: App,
    canvas: Canvas,
    input: Input,
}

thread_local! {
    static WINDOW: RefCell<Option<Window>> = const { RefCell::new(None) };
}

static CLOSING: AtomicBool = AtomicBool::new(false);

fn paste_text() -> Option<String> {
    // SAFETY: the clipboard is opened, read and closed in order; the lock is released.
    unsafe {
        OpenClipboard(None).ok()?;
        let mut text = None;
        if let Ok(handle) = GetClipboardData(13) {
            let global = HGLOBAL(handle.0);
            let data = GlobalLock(global) as *const u16;
            if !data.is_null() {
                let mut len = 0;
                while *data.add(len) != 0 {
                    len += 1;
                }
                text = Some(String::from_utf16_lossy(std::slice::from_raw_parts(data, len)));
                let _ = GlobalUnlock(global);
            }
        }
        let _ = CloseClipboard();
        text
    }
}

fn dpi_scale(hwnd: HWND) -> f32 {
    // SAFETY: a plain query on our own window.
    unsafe { GetDpiForWindow(hwnd) as f32 / 96.0 }
}

fn draw(hwnd: HWND, window: &mut Window) {
    let mut signals = Signals::default();
    let input = std::mem::take(&mut window.input);
    let held = Input { x: input.x, y: input.y, down: input.down, ..Default::default() };
    window.app.frame(&mut window.canvas, &input, &mut signals);
    window.input = held;

    let c = &window.canvas;
    let header = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: c.width,
        biHeight: -c.height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let info = BITMAPINFO { bmiHeader: header, ..Default::default() };
    // SAFETY: the pixels match the bitmap header, and the DC is released.
    unsafe {
        let dc = GetDC(Some(hwnd));
        SetDIBitsToDevice(dc, 0, 0, c.width as u32, c.height as u32, 0, 0, 0, c.height as u32, c.px.as_ptr() as *const _, &info, DIB_RGB_COLORS);
        ReleaseDC(Some(hwnd), dc);
    }

    if signals.drag {
        // SAFETY: standard way to let the shell move a borderless window.
        unsafe {
            let _ = ReleaseCapture();
            SendMessageW(hwnd, WM_NCLBUTTONDOWN, Some(WPARAM(HTCAPTION as usize)), Some(LPARAM(0)));
        }
    }
    if signals.minimise {
        // SAFETY: our own window.
        unsafe {
            let _ = ShowWindow(hwnd, SW_MINIMIZE);
        }
    }
    if signals.close {
        CLOSING.store(true, Ordering::SeqCst);
        // SAFETY: our own window.
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

fn with_window(f: impl FnOnce(&mut Window)) {
    WINDOW.with(|cell| {
        if let Ok(mut slot) = cell.try_borrow_mut() {
            if let Some(window) = slot.as_mut() {
                f(window);
            }
        }
    });
}

extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let position = |window: &mut Window| {
        let x = (lparam.0 & 0xffff) as i16 as f32;
        let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
        let s = window.canvas.scale;
        window.input.x = x / s;
        window.input.y = y / s;
    };
    match message {
        WM_TIMER | WM_PAINT => {
            if message == WM_PAINT {
                let mut paint = PAINTSTRUCT::default();
                // SAFETY: paired with EndPaint.
                unsafe {
                    BeginPaint(hwnd, &mut paint);
                    let _ = EndPaint(hwnd, &paint);
                }
            }
            with_window(|w| draw(hwnd, w));
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            with_window(position);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            with_window(|w| {
                position(w);
                w.input.down = true;
                w.input.clicked = true;
            });
            // SAFETY: released on button up.
            unsafe {
                SetCapture(hwnd);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            with_window(|w| {
                position(w);
                w.input.down = false;
                w.input.released = true;
            });
            // SAFETY: pairs with SetCapture.
            unsafe {
                let _ = ReleaseCapture();
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let turn = ((wparam.0 >> 16) & 0xffff) as i16 as f32 / 120.0;
            with_window(|w| w.input.wheel += turn);
            LRESULT(0)
        }
        WM_CHAR => {
            let code = wparam.0 as u32;
            if let Some(ch) = char::from_u32(code) {
                if !ch.is_control() {
                    with_window(|w| w.input.typed.push(ch));
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let key = VIRTUAL_KEY(wparam.0 as u16);
            // SAFETY: plain state query.
            let held = |k: VIRTUAL_KEY| unsafe { GetKeyState(k.0 as i32) } < 0;
            with_window(|w| match key {
                VK_TAB if held(VK_SHIFT) => w.input.back_tab = true,
                VK_TAB => w.input.tab = true,
                VK_RETURN => w.input.enter = true,
                VK_ESCAPE => w.input.escape = true,
                VK_BACK => w.input.backspace = true,
                VK_DELETE => w.input.delete = true,
                VK_LEFT => w.input.left = true,
                VK_RIGHT => w.input.right = true,
                VK_HOME => w.input.home = true,
                VK_END => w.input.end = true,
                VK_V if held(VK_CONTROL) => w.input.paste = paste_text(),
                _ => {}
            });
            LRESULT(0)
        }
        WM_DPICHANGED => {
            // SAFETY: the system passes the suggested rectangle in lparam.
            let rect = unsafe { *(lparam.0 as *const RECT) };
            with_window(|w| {
                let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
                let scale = (wparam.0 & 0xffff) as f32 / 96.0;
                w.canvas.resize(width, height, scale);
            });
            // SAFETY: our own window.
            unsafe {
                let _ = SetWindowPos(hwnd, None, rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top, SWP_NOZORDER | SWP_NOACTIVATE);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        // SAFETY: forwards what we do not handle.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn open_window(app: App, title: &str) -> Result<()> {
    // SAFETY: Win32 setup on this thread, all arguments valid for the calls.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: w!("KryotoSetup"),
            lpfnWndProc: Some(proc),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: LoadIconW(Some(instance.into()), windows::core::PCWSTR(1 as _)).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&class);
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let scale = GetDpiForSystem() as f32 / 96.0;
        let (w, h) = ((WIDTH * scale) as i32, (HEIGHT * scale) as i32);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("KryotoSetup"),
            &HSTRING::from(title),
            WS_POPUP | WS_MINIMIZEBOX | WS_SYSMENU,
            (screen_w - w) / 2,
            (screen_h - h) / 2,
            w,
            h,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let corners = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corners as *const _ as *const _, 4);
        let scale = dpi_scale(hwnd);
        let mut app = app;
        app.set_window(hwnd);
        let canvas = Canvas::new((WIDTH * scale) as i32, (HEIGHT * scale) as i32, scale);
        WINDOW.with(|cell| {
            *cell.borrow_mut() = Some(Window { app, canvas, input: Input::default() });
        });
        let _ = ShowWindow(hwnd, SW_SHOW);
        SetTimer(Some(hwnd), 1, 33, None);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

fn message_box(text: &str) {
    // SAFETY: strings outlive the call.
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), w!("Kryoto Setup"), MB_ICONERROR | MB_OK);
    }
}

#[derive(Default)]
struct Args {
    auto: bool,
    silent: bool,
    dir: Option<String>,
    parts: Option<String>,
    desktop: Option<bool>,
    start_menu: Option<bool>,
    verify: Option<bool>,
    low_memory: bool,
    mute: bool,
    redists: bool,
    cleanup: Option<PathBuf>,
    shot: Option<(String, String)>,
}

fn parse_args() -> Args {
    let mut args = Args::default();
    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        let lower = arg.to_ascii_lowercase();
        if let Some(dir) = arg.strip_prefix("--dir=").or(arg.strip_prefix("/DIR=")) {
            args.dir = Some(dir.to_string());
        } else if let Some(parts) = arg.strip_prefix("--parts=") {
            args.parts = Some(parts.to_string());
        } else {
            match lower.as_str() {
                "--auto" => args.auto = true,
                "/silent" | "/verysilent" | "--silent" => args.silent = true,
                "--desktop" => args.desktop = Some(true),
                "--start-menu" => args.start_menu = Some(true),
                "--verify" => args.verify = Some(true),
                "--low-memory" => args.low_memory = true,
                "/noicons" | "--no-shortcuts" => {
                    args.desktop = Some(false);
                    args.start_menu = Some(false);
                }
                "/mute" | "--mute" => args.mute = true,
                "--redists" => args.redists = true,
                "--cleanup" => args.cleanup = raw.next().map(PathBuf::from),
                "--shot" => {
                    if let (Some(page), Some(file)) = (raw.next(), raw.next()) {
                        args.shot = Some((page, file));
                    }
                }
                _ => {}
            }
        }
    }
    args
}

/// `/SILENT`: no window. The exit code says how it went.
fn silent_install(args: &Args, container: &std::path::Path) -> Result<()> {
    let (manifest, _) = open_repack(container)?;
    let selected: Vec<bool> = match &args.parts {
        Some(parts) => parts.chars().map(|c| c == '1').collect(),
        None => manifest.components.iter().map(|c| c.selected || c.required).collect(),
    };
    let dest = match &args.dir {
        Some(dir) => PathBuf::from(dir),
        None => system::default_dest(&manifest.game.title, thaw::selection_size(&manifest, &selected)),
    };
    let (_, available) = system::memory();
    let options = ThawOptions {
        dest: dest.clone(),
        source: Source::new(container),
        selected: selected.clone(),
        threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2),
        memory: available / 10 * 6,
        verify: args.verify.unwrap_or(true),
    };
    thaw::thaw(&manifest, &options, &Progress::new())?;
    let (files, _) = thaw::selection(&manifest, &selected);
    system::finish_install(system::Finish {
        manifest: &manifest,
        dest: &dest,
        files: files.iter().map(|&i| manifest.files[i].path.clone()).collect(),
        desktop: args.desktop.unwrap_or(true),
        start_menu: args.start_menu.unwrap_or(true),
    })
}

#[cfg(feature = "shots")]
fn write_shot(app: &mut App, page: &str, file: &str) -> Result<()> {
    let scale = 1.0;
    let mut canvas = Canvas::new((WIDTH * scale) as i32, (HEIGHT * scale) as i32, scale);
    let (posed, share) = match page {
        "welcome" => (Page::Welcome, 0.0),
        "early" => (Page::Installing, 0.18),
        "installing" => (Page::Installing, 0.57),
        "late" => (Page::Installing, 0.93),
        "done" => (Page::Done, 1.0),
        "failed" => (Page::Failed, 0.31),
        other => bail!("no page {other}"),
    };
    app.pose(posed, share);
    let mut signals = Signals::default();
    for _ in 0..3 {
        app.frame(&mut canvas, &Input::default(), &mut signals);
    }
    let mut bmp = Vec::with_capacity(canvas.px.len() * 4 + 54);
    let size = (canvas.px.len() * 4) as u32;
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(size + 54).to_le_bytes());
    bmp.extend_from_slice(&[0, 0, 0, 0, 54, 0, 0, 0, 40, 0, 0, 0]);
    bmp.extend_from_slice(&canvas.width.to_le_bytes());
    bmp.extend_from_slice(&(-canvas.height).to_le_bytes());
    bmp.extend_from_slice(&[1, 0, 32, 0, 0, 0, 0, 0]);
    bmp.extend_from_slice(&size.to_le_bytes());
    bmp.extend_from_slice(&[0; 16]);
    for pixel in &canvas.px {
        bmp.extend_from_slice(&(pixel | 0xff000000).to_le_bytes());
    }
    std::fs::write(file, bmp)?;
    Ok(())
}

fn run() -> Result<()> {
    let args = parse_args();
    if let Some(dir) = &args.cleanup {
        system::cleanup(dir);
        return Ok(());
    }
    let me = std::env::current_exe()?;

    // No repack on the end of this program: it is the uninstaller in a game folder.
    let repack = open_repack(&me);
    let Ok((manifest, assets)) = repack else {
        let dir = me.parent().map(PathBuf::from).unwrap_or_default();
        let Ok(info) = system::InstallInfo::load(&dir) else {
            if let Err(e) = repack {
                bail!("{e:#}");
            }
            return Ok(());
        };
        let mut app = App::new(None, None, Assets { tune: false, ..Default::default() }, true);
        app.page = Page::Uninstall;
        let title = format!("Remove {}", info.title);
        app.uninstall = Some((dir, info));
        return open_window(app, &title);
    };

    if args.redists {
        let dest = PathBuf::from(args.dir.clone().unwrap_or_default());
        system::run_redists(&manifest, &dest);
        return Ok(());
    }
    if args.silent {
        return silent_install(&args, &me);
    }

    let source = Source::new(&me);
    let required_missing: Vec<String> = {
        let required: Vec<bool> = manifest.components.iter().map(|c| c.required).collect();
        thaw::missing_volumes(&manifest, &source, &required)
    };
    let title = format!("{} Setup", manifest.game.title);
    let mut app = App::new(Some(manifest), Some(source), assets, args.mute);

    #[cfg(feature = "shots")]
    if let Some((page, file)) = &args.shot {
        return write_shot(&mut app, page, file);
    }

    if !required_missing.is_empty() {
        bail!("Missing or incomplete: {}.\nPut them in the same folder as this setup.", required_missing.join(", "));
    }
    if let Some(dir) = &args.dir {
        app.choices.dest = dir.clone();
    }
    if args.desktop == Some(false) {
        app.choices.desktop = false;
        app.choices.start_menu = false;
    }
    if args.auto {
        if let Some(parts) = &args.parts {
            app.choices.selected = parts.chars().map(|c| c == '1').collect();
        }
        app.choices.desktop = args.desktop.unwrap_or(false);
        app.choices.start_menu = args.start_menu.unwrap_or(false);
        app.choices.verify = args.verify.unwrap_or(false);
        app.choices.low_memory = args.low_memory;
        app.auto_start();
    }
    open_window(app, &title)
}

fn main() {
    if let Err(e) = run() {
        let silent = std::env::args().any(|a| a.eq_ignore_ascii_case("/silent") || a.eq_ignore_ascii_case("/verysilent"));
        if silent {
            eprintln!("{e:#}");
        } else {
            message_box(&format!("{e:#}"));
        }
        std::process::exit(1);
    }
}
