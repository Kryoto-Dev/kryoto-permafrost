//! Everything that talks to Windows: disks, shortcuts, the Programs list, elevation.

use anyhow::{Context, Result};
use kryo::format::Manifest;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use windows::core::{Interface, GUID, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Diagnostics::Debug::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::SystemInformation::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const UNINSTALLER: &str = "Permafrost Uninstall.exe";
pub const INSTALL_INFO: &str = ".kryo-install";

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

pub fn free_space(path: &Path) -> Option<u64> {
    let mut probe = path.to_path_buf();
    loop {
        let name = HSTRING::from(probe.as_os_str());
        let mut free = 0u64;
        // SAFETY: `free` outlives the call.
        if unsafe { GetDiskFreeSpaceExW(&name, Some(&mut free), None, None) }.is_ok() {
            return Some(free);
        }
        if !probe.pop() {
            return None;
        }
    }
}

fn fixed_drives() -> Vec<PathBuf> {
    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| PathBuf::from(format!("{}:\\", (b'A' + i) as char)))
        .filter(|p| {
            let name = HSTRING::from(p.as_os_str());
            // SAFETY: `name` outlives the call.
            unsafe { GetDriveTypeW(&name) == 3 }
        })
        .collect()
}

/// `C:\Games\<title>`, on a drive with room for it.
pub fn default_dest(title: &str, needed: u64) -> PathBuf {
    let drives = fixed_drives();
    let roomy = drives
        .iter()
        .find(|d| free_space(d).is_some_and(|f| f > needed + (needed / 10) + (2 << 30)))
        .or_else(|| drives.iter().max_by_key(|d| free_space(d).unwrap_or(0)));
    let root = roomy.cloned().unwrap_or_else(|| PathBuf::from("C:\\"));
    root.join("Games").join(safe_name(title))
}

pub fn safe_name(name: &str) -> String {
    let clean: String = name.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { ' ' } else { c }).collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean = clean.trim_end_matches('.').to_string();
    if clean.is_empty() { "Game".into() } else { clean }
}

pub fn memory() -> (u64, u64) {
    let mut status = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: `status` is sized as the call requires.
    if unsafe { GlobalMemoryStatusEx(&mut status) }.is_ok() {
        (status.ullTotalPhys, status.ullAvailPhys)
    } else {
        (8 << 30, 4 << 30)
    }
}

pub fn pick_folder(owner: HWND, start: &Path) -> Option<PathBuf> {
    // SAFETY: COM objects are released on drop; strings live across their calls.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let options = dialog.GetOptions().ok()?;
        dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST).ok()?;
        let _ = dialog.SetTitle(&HSTRING::from("Install to"));
        let mut existing = start.to_path_buf();
        while !existing.is_dir() && existing.pop() {}
        let folder = HSTRING::from(existing.as_os_str());
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&folder, None) {
            let _ = dialog.SetFolder(&item);
        }
        dialog.Show(Some(owner)).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        path.map(PathBuf::from)
    }
}

fn known_folder(id: &GUID) -> Result<PathBuf> {
    // SAFETY: the returned string is freed after it is copied.
    unsafe {
        let raw: PWSTR = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None)?;
        let text = raw.to_string();
        CoTaskMemFree(Some(raw.0 as *const _));
        Ok(PathBuf::from(text?))
    }
}

fn make_link(path: &Path, target: &Path, args: &str, dir: &Path, about: &str) -> Result<()> {
    // SAFETY: as above; COM is initialised on this thread first.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(target.as_os_str()))?;
        link.SetArguments(&HSTRING::from(args))?;
        link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        link.SetDescription(&HSTRING::from(about))?;
        let file: IPersistFile = link.cast()?;
        file.Save(&HSTRING::from(path.as_os_str()), true)?;
    }
    Ok(())
}

/// What the uninstaller needs. Kept in the game folder.
#[derive(Serialize, Deserialize, Default)]
pub struct InstallInfo {
    pub title: String,
    pub key: String,
    pub files: Vec<String>,
    pub empty_dirs: Vec<String>,
    pub links: Vec<PathBuf>,
}

impl InstallInfo {
    pub fn load(dir: &Path) -> Result<Self> {
        let raw = std::fs::read(dir.join(INSTALL_INFO)).context("reading the install record")?;
        Ok(postcard::from_bytes(&raw)?)
    }
}

pub struct Finish<'a> {
    pub manifest: &'a Manifest,
    pub dest: &'a Path,
    pub files: Vec<String>,
    pub desktop: bool,
    pub start_menu: bool,
}

/// Length of the program itself, without anything appended to it.
fn image_length(data: &[u8]) -> Result<usize> {
    let read32 = |at: usize| data.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
    let read16 = |at: usize| data.get(at..at + 2).map(|b| u16::from_le_bytes(b.try_into().unwrap()) as usize);
    let pe = read32(0x3c).context("reading the setup")?;
    let sections = read16(pe + 6).context("reading the setup")?;
    let optional = read16(pe + 20).context("reading the setup")?;
    let table = pe + 24 + optional;
    let mut end = 0;
    for i in 0..sections {
        let at = table + i * 40;
        let raw_size = read32(at + 16).context("reading the setup")?;
        let raw_start = read32(at + 20).context("reading the setup")?;
        end = end.max(raw_start + raw_size);
    }
    Ok(end)
}

fn write_uninstaller(dest: &Path) -> Result<PathBuf> {
    use std::io::Read;
    let me = std::env::current_exe()?;
    // Only the head: the setup can be gigabytes, the program is the first few megabytes.
    let mut head = vec![0u8; 32 << 20];
    let mut file = std::fs::File::open(&me)?;
    let mut filled = 0;
    while filled < head.len() {
        let n = file.read(&mut head[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    head.truncate(filled);
    let length = image_length(&head)?;
    let path = dest.join(UNINSTALLER);
    std::fs::write(&path, &head[..length.min(filled)]).context("writing the uninstaller")?;
    Ok(path)
}

fn set_string(key: HKEY, name: &str, value: &str) {
    let text: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the bytes are valid for the call and the key is open.
    let bytes = unsafe { std::slice::from_raw_parts(text.as_ptr() as *const u8, text.len() * 2) };
    let _ = unsafe { RegSetValueExW(key, &HSTRING::from(name), None, REG_SZ, Some(bytes)) };
}

fn set_number(key: HKEY, name: &str, value: u32) {
    let _ = unsafe { RegSetValueExW(key, &HSTRING::from(name), None, REG_DWORD, Some(&value.to_le_bytes())) };
}

fn uninstall_key(id: &str) -> String {
    format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{id}")
}

/// Shortcuts, the uninstaller, and the entry in Windows' list of programs.
pub fn finish_install(job: Finish) -> Result<()> {
    let game = &job.manifest.game;
    let title = safe_name(&game.title);
    let target = job.dest.join(game.exe.replace('/', "\\"));
    let dir = target.parent().unwrap_or(job.dest).to_path_buf();
    let mut links = Vec::new();
    if !game.exe.is_empty() {
        if job.desktop {
            let path = known_folder(&FOLDERID_Desktop)?.join(format!("{title}.lnk"));
            make_link(&path, &target, &game.args, &dir, &game.title)?;
            links.push(path);
        }
        if job.start_menu {
            let path = known_folder(&FOLDERID_Programs)?.join(format!("{title}.lnk"));
            make_link(&path, &target, &game.args, &dir, &game.title)?;
            links.push(path);
        }
    }

    let uninstaller = write_uninstaller(job.dest)?;
    let key = format!("Kryoto-{}", title.replace(' ', "-"));
    let info = InstallInfo {
        title: game.title.clone(),
        key: key.clone(),
        files: job.files,
        empty_dirs: job.manifest.empty_dirs.clone(),
        links,
    };
    std::fs::write(job.dest.join(INSTALL_INFO), postcard::to_stdvec(&info)?)?;

    let mut handle = HKEY::default();
    let path = HSTRING::from(uninstall_key(&key));
    // SAFETY: `handle` receives the key and is closed below.
    let made = unsafe {
        RegCreateKeyExW(HKEY_CURRENT_USER, &path, None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut handle, None)
    };
    if made.is_ok() {
        let size_kb = job.manifest.stats.original / 1024;
        set_string(handle, "DisplayName", &game.title);
        set_string(handle, "DisplayVersion", &game.version);
        set_string(handle, "Publisher", if game.repacker.is_empty() { "Kryoto" } else { &game.repacker });
        set_string(handle, "InstallLocation", &job.dest.to_string_lossy());
        set_string(handle, "UninstallString", &format!("\"{}\"", uninstaller.display()));
        set_string(handle, "DisplayIcon", &target.to_string_lossy());
        set_number(handle, "EstimatedSize", size_kb.min(u32::MAX as u64) as u32);
        set_number(handle, "NoModify", 1);
        set_number(handle, "NoRepair", 1);
        // SAFETY: opened above.
        let _ = unsafe { RegCloseKey(handle) };
    }
    Ok(())
}

/// Removes what the setup put down. Anything the game wrote itself, like saves, stays.
pub fn remove_install(dir: &Path, info: &InstallInfo, mut step: impl FnMut(f32)) -> Result<()> {
    let total = info.files.len().max(1) as f32;
    for (i, file) in info.files.iter().enumerate() {
        let path = kryo::thaw::long_path(&kryo::thaw::safe_join(dir, file)?);
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.permissions().readonly() {
                let mut p = meta.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                p.set_readonly(false);
                let _ = std::fs::set_permissions(&path, p);
            }
            let _ = std::fs::remove_file(&path);
        }
        step(i as f32 / total);
    }
    let mut parents: Vec<PathBuf> = info
        .files
        .iter()
        .filter_map(|f| kryo::thaw::safe_join(dir, f).ok())
        .flat_map(|p| p.ancestors().skip(1).map(Path::to_path_buf).collect::<Vec<_>>())
        .chain(info.empty_dirs.iter().filter_map(|d| kryo::thaw::safe_join(dir, d).ok()))
        .filter(|p| p.starts_with(dir) && p != dir)
        .collect();
    parents.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    parents.dedup();
    for parent in parents {
        let _ = std::fs::remove_dir(kryo::thaw::long_path(&parent));
    }
    for link in &info.links {
        let _ = std::fs::remove_file(link);
    }
    let path = HSTRING::from(uninstall_key(&info.key));
    // SAFETY: plain call on a constant root key.
    let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &path) };
    Ok(())
}

/// The uninstaller cannot delete itself while running, so a copy in Temp finishes the job.
pub fn hand_off_cleanup(dir: &Path) -> Result<()> {
    let me = std::env::current_exe()?;
    let copy = std::env::temp_dir().join(format!("kryoto-cleanup-{}.exe", std::process::id()));
    std::fs::copy(&me, &copy)?;
    std::process::Command::new(&copy).arg("--cleanup").arg(dir).spawn()?;
    Ok(())
}

pub fn cleanup(dir: &Path) {
    // The uninstaller stays open until the player closes it, which may take a while.
    for _ in 0..7200 {
        let gone = std::fs::remove_file(dir.join(UNINSTALLER)).is_ok() || !dir.join(UNINSTALLER).exists();
        if gone {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let _ = std::fs::remove_file(dir.join(INSTALL_INFO));
    let _ = std::fs::remove_dir(dir);
}

/// Starts this program again as administrator and waits for it.
pub fn run_elevated(args: &str, wait: bool) -> Result<u32> {
    let me = std::env::current_exe()?;
    let file = wide(me.as_os_str());
    let params = wide(OsStr::new(args));
    let verb = wide(OsStr::new("runas"));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: every pointer in `info` outlives the call.
    unsafe { ShellExecuteExW(&mut info) }.context("Windows refused to run the setup as administrator")?;
    if !wait || info.hProcess.is_invalid() {
        return Ok(0);
    }
    let mut code = 0u32;
    // SAFETY: the handle came from the call above and is closed here.
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
    }
    Ok(code)
}

/// Runs each runtime installer quietly. This process is already elevated. The game is
/// installed by now, so a runtime that will not start is skipped, not an error.
pub fn run_redists(manifest: &Manifest, dest: &Path) {
    // Keeps Windows' own "cannot run this program" boxes off the player's screen.
    // SAFETY: only sets this process's error mode.
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
    }
    for redist in &manifest.redists {
        let Ok(path) = kryo::thaw::safe_join(dest, &redist.path) else { continue };
        if path.exists() {
            let _ = std::process::Command::new(&path)
                .raw_arg_compat(&redist.args)
                .current_dir(path.parent().unwrap_or(dest))
                .status();
        }
    }
}

trait RawArg {
    fn raw_arg_compat(&mut self, args: &str) -> &mut Self;
}

impl RawArg for std::process::Command {
    fn raw_arg_compat(&mut self, args: &str) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.raw_arg(args)
    }
}

pub fn can_write(dest: &Path) -> bool {
    let mut probe = dest.to_path_buf();
    while !probe.exists() {
        if !probe.pop() {
            return false;
        }
    }
    let test = probe.join(format!(".kryo-write-test-{}", std::process::id()));
    match std::fs::write(&test, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(test);
            true
        }
        Err(_) => false,
    }
}

pub fn open_folder(path: &Path) {
    let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
}

pub fn launch_game(dest: &Path, manifest: &Manifest) {
    let target = dest.join(manifest.game.exe.replace('/', "\\"));
    let dir = target.parent().unwrap_or(dest).to_path_buf();
    let mut command = std::process::Command::new(&target);
    command.raw_arg_compat(&manifest.game.args).current_dir(dir);
    let _ = command.spawn();
}

pub fn shut_down_soon() {
    // Sixty seconds, and `shutdown /a` calls it off.
    let _ = std::process::Command::new("shutdown").args(["/s", "/t", "60"]).spawn();
}

pub fn open_site(url: &str) {
    if url.starts_with("https://") || url.starts_with("http://") {
        let url = HSTRING::from(url);
        // SAFETY: strings outlive the call.
        unsafe {
            ShellExecuteW(None, &HSTRING::from("open"), &url, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        }
    }
}
