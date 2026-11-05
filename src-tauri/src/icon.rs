//! Giving the setup the game's own icon. The icon is read out of the game's program and
//! written into a copy of the setup before anything is appended to it.

use anyhow::{bail, Context, Result};
use std::path::Path;
use windows::core::{BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// One image of an icon, with the directory entry that describes it.
struct Image {
    entry: [u8; 12],
    data: Vec<u8>,
}

fn int_resource(id: u16) -> PCWSTR {
    PCWSTR(id as usize as *const u16)
}

unsafe fn resource(module: HMODULE, name: PCWSTR, kind: PCWSTR) -> Option<Vec<u8>> {
    let info = FindResourceW(Some(module), name, kind);
    if info.is_invalid() {
        return None;
    }
    let handle = LoadResource(Some(module), info).ok()?;
    let pointer = LockResource(handle) as *const u8;
    if pointer.is_null() {
        return None;
    }
    let size = SizeofResource(Some(module), info) as usize;
    Some(std::slice::from_raw_parts(pointer, size).to_vec())
}

unsafe extern "system" fn first_name(_module: HMODULE, _kind: PCWSTR, name: PCWSTR, found: isize) -> BOOL {
    let out = &mut *(found as *mut Option<GroupName>);
    *out = Some(if (name.0 as usize) >> 16 == 0 {
        GroupName::Id(name.0 as usize as u16)
    } else {
        GroupName::Text(name.to_string().unwrap_or_default())
    });
    BOOL(0)
}

enum GroupName {
    Id(u16),
    Text(String),
}

/// The first icon of a program, with all its sizes.
fn from_program(path: &Path) -> Result<Vec<Image>> {
    let name = HSTRING::from(path.as_os_str());
    // SAFETY: the module is only read as data and is freed before returning.
    unsafe {
        let module = LoadLibraryExW(&name, None, LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE)
            .context("reading the game's program")?;
        let mut group: Option<GroupName> = None;
        let _ = EnumResourceNamesW(Some(module), RT_GROUP_ICON, Some(first_name), &mut group as *mut _ as isize);
        let directory = match &group {
            Some(GroupName::Id(id)) => resource(module, int_resource(*id), RT_GROUP_ICON),
            Some(GroupName::Text(text)) => resource(module, PCWSTR(HSTRING::from(text.as_str()).as_ptr()), RT_GROUP_ICON),
            None => None,
        };
        let mut images = Vec::new();
        if let Some(directory) = directory {
            let count = u16::from_le_bytes([directory[4], directory[5]]) as usize;
            for i in 0..count {
                let at = 6 + i * 14;
                let Some(row) = directory.get(at..at + 14) else { break };
                let id = u16::from_le_bytes([row[12], row[13]]);
                if let Some(data) = resource(module, int_resource(id), RT_ICON) {
                    let mut entry = [0u8; 12];
                    entry.copy_from_slice(&row[..12]);
                    images.push(Image { entry, data });
                }
            }
        }
        let _ = FreeLibrary(module);
        if images.is_empty() {
            bail!("That program has no icon.");
        }
        Ok(images)
    }
}

fn from_ico(path: &Path) -> Result<Vec<Image>> {
    let data = std::fs::read(path).context("reading the icon")?;
    if data.len() < 6 || data[..4] != [0, 0, 1, 0] {
        bail!("That is not an .ico file.");
    }
    let count = u16::from_le_bytes([data[4], data[5]]) as usize;
    let mut images = Vec::new();
    for i in 0..count {
        let at = 6 + i * 16;
        let row = data.get(at..at + 16).context("That .ico is damaged.")?;
        let size = u32::from_le_bytes(row[8..12].try_into()?) as usize;
        let offset = u32::from_le_bytes(row[12..16].try_into()?) as usize;
        let body = data.get(offset..offset + size).context("That .ico is damaged.")?;
        let mut entry = [0u8; 12];
        entry.copy_from_slice(&row[..12]);
        images.push(Image { entry, data: body.to_vec() });
    }
    Ok(images)
}

/// Replaces the icon of the program at `target` with the one in `source` (.exe or .ico).
pub fn apply(target: &Path, source: &Path) -> Result<()> {
    let is_ico = source.extension().is_some_and(|e| e.eq_ignore_ascii_case("ico"));
    let images = if is_ico { from_ico(source)? } else { from_program(source)? };
    let name = HSTRING::from(target.as_os_str());
    // SAFETY: the update handle is closed on every path; data outlives each call.
    unsafe {
        let update = BeginUpdateResourceW(&name, false).context("opening the setup")?;
        let mut directory = vec![0u8, 0, 1, 0];
        directory.extend_from_slice(&(images.len() as u16).to_le_bytes());
        for (i, image) in images.iter().enumerate() {
            let id = i as u16 + 1;
            directory.extend_from_slice(&image.entry);
            directory.extend_from_slice(&id.to_le_bytes());
            UpdateResourceW(update, RT_ICON, int_resource(id), 0, Some(image.data.as_ptr() as *const _), image.data.len() as u32)
                .context("writing the icon")?;
        }
        UpdateResourceW(update, RT_GROUP_ICON, int_resource(1), 0, Some(directory.as_ptr() as *const _), directory.len() as u32)
            .context("writing the icon")?;
        EndUpdateResourceW(update, false).context("saving the icon")?;
    }
    Ok(())
}
