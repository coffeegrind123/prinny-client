#[cfg(target_os = "windows")]
mod win {
    use std::cell::RefCell;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{BOOL, HWND};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateIconFromResourceEx, DestroyIcon, HICON, LR_DEFAULTCOLOR,
    };

    thread_local! {
        static TASKBAR: RefCell<Option<ITaskbarList3>> = RefCell::new(None);
    }

    /// The size the shell draws a taskbar overlay at.
    const OVERLAY_PX: i32 = 16;

    /// `CreateIconFromResourceEx`'s version word for an icon resource. Fixed by
    /// the API, not by the file: 0x00030000 is what every caller passes.
    const ICON_RESOURCE_VERSION: u32 = 0x0003_0000;

    fn get_taskbar() -> Option<ITaskbarList3> {
        TASKBAR.with(|cell| {
            if cell.borrow().is_none() {
                unsafe {
                    *cell.borrow_mut() =
                        CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER).ok();
                }
            }
            cell.borrow().clone()
        })
    }

    /// The image inside an `.ico` file that best matches the overlay size.
    ///
    /// An `.ico` is a directory (`ICONDIR`: reserved, type, count) followed by
    /// one 16-byte `ICONDIRENTRY` per image, each naming a byte range elsewhere
    /// in the file. `CreateIconFromResourceEx` wants that byte range alone — the
    /// image, not the file that contains it — so the directory is walked here
    /// rather than handed to the OS.
    ///
    /// Every offset is bounds-checked against the buffer. These are our own
    /// bundled icons today, but a parser that trusts its input is a parser that
    /// reads out of bounds the day its input changes.
    fn best_image(data: &[u8]) -> Option<&[u8]> {
        const DIR_LEN: usize = 6;
        const ENTRY_LEN: usize = 16;
        const TYPE_ICON: u16 = 1;

        if data.len() < DIR_LEN {
            return None;
        }
        let reserved = u16::from_le_bytes([data[0], data[1]]);
        let kind = u16::from_le_bytes([data[2], data[3]]);
        let count = u16::from_le_bytes([data[4], data[5]]) as usize;
        if reserved != 0 || kind != TYPE_ICON || count == 0 {
            return None;
        }

        let mut best: Option<(&[u8], u32)> = None;
        for index in 0..count {
            let entry = DIR_LEN + index * ENTRY_LEN;
            let fields = match data.get(entry..entry + ENTRY_LEN) {
                Some(fields) => fields,
                // The directory claims more entries than the file holds.
                None => break,
            };
            // A zero width means 256 — the format has one byte for it.
            let width = if fields[0] == 0 { 256u32 } else { u32::from(fields[0]) };
            let size = u32::from_le_bytes([fields[8], fields[9], fields[10], fields[11]]) as usize;
            let offset =
                u32::from_le_bytes([fields[12], fields[13], fields[14], fields[15]]) as usize;
            let end = match offset.checked_add(size) {
                Some(end) if size > 0 && end <= data.len() => end,
                // An entry pointing outside the file, or at nothing at all.
                _ => continue,
            };

            let distance = width.abs_diff(OVERLAY_PX as u32);
            let better = match best {
                Some((_, best_distance)) => distance < best_distance,
                None => true,
            };
            if better {
                best = Some((&data[offset..end], distance));
            }
        }

        best.map(|(image, _)| image)
    }

    /// An overlay icon built from `.ico` bytes, or `None` if they do not parse.
    ///
    /// From memory rather than through a temporary file: the previous form
    /// wrote the bytes to `%TEMP%`, called `LoadImageW(LR_LOADFROMFILE)` on the
    /// path and deleted it again, which put a disk write and two file-system
    /// round trips on the path of every unread-count change.
    fn create_icon(data: &[u8]) -> Option<HICON> {
        let image = best_image(data)?;
        unsafe {
            CreateIconFromResourceEx(
                image,
                // TRUE: this is an icon, not a cursor.
                BOOL(1),
                ICON_RESOURCE_VERSION,
                OVERLAY_PX,
                OVERLAY_PX,
                LR_DEFAULTCOLOR,
            )
        }
        .ok()
    }

    /// hwnd_raw: raw HWND as isize (from tauri::Window::hwnd())
    pub fn set_overlay(hwnd_raw: isize, icon_data: Option<&[u8]>) {
        let taskbar = match get_taskbar() {
            Some(tb) => tb,
            None => return,
        };

        let hicon = icon_data.and_then(create_icon);

        unsafe {
            // A null HICON is how the overlay is *removed*, so a count of zero
            // and an icon that would not parse both take this path deliberately.
            let _ = taskbar.SetOverlayIcon(
                HWND(hwnd_raw as *mut _),
                hicon.unwrap_or(HICON(std::ptr::null_mut())),
                PCWSTR::null(),
            );

            // The shell copies the icon into its own button state rather than
            // taking ownership of the handle, so this one is ours to free — and
            // freeing it is not optional. Every unread-count change lands here,
            // and an icon leaked per change walks a long session towards the
            // 10,000-object per-process GDI limit, at which point the app stops
            // being able to create the handles it needs to *draw*. The failure
            // is a UI that quietly stops rendering, a long way from its cause.
            if let Some(icon) = hicon {
                let _ = DestroyIcon(icon);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
#[allow(dead_code, unused_imports)]
mod win {
    pub fn set_overlay(_hwnd: isize, _icon_data: Option<&[u8]>) {}
}

#[allow(unused_imports)]
pub use win::set_overlay;
