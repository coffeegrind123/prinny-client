//! "Edit in your text editor" for custom CSS, desktop half.
//!
//! The frontend hands over the full stylesheet; this writes it to a fixed file
//! in the app data directory, opens that file with the OS default editor, and
//! watches it. Every save is read back and emitted to the page as
//! `custom-css-changed`, where it is diffed against the defaults and applied.
//!
//! The path is fixed and never comes from the page, so these commands cannot be
//! used to read or write anything else on disk.
//!
//! Watching polls metadata instead of using OS file notifications: editors save
//! in very different ways (in place, temp file + rename, delete + recreate) and
//! a twice-a-second `stat` of one file sees all of them, at no measurable cost
//! and with no extra dependency.

/// Emitted to the page with the file's new content after each save.
#[cfg(not(mobile))]
const CHANGED_EVENT: &str = "custom-css-changed";

/// Same bound as the frontend's `MAX_FILE_BYTES`; the real file is ~0.3 MB.
#[cfg(not(mobile))]
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

#[cfg(not(mobile))]
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

#[cfg(not(mobile))]
const DIR_NAME: &str = "custom-css";
#[cfg(not(mobile))]
const FILE_NAME: &str = "prinny.css";

#[cfg(not(mobile))]
#[derive(Clone, serde::Serialize)]
struct Changed {
    content: String,
}

#[cfg(not(mobile))]
struct Watcher {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(not(mobile))]
impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The running watcher, if any. Replacing or clearing it stops the old thread.
#[derive(Default)]
pub struct CustomCssWatch {
    #[cfg(not(mobile))]
    watcher: std::sync::Mutex<Option<Watcher>>,
}

#[cfg(not(mobile))]
fn file_path<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<std::path::PathBuf, String> {
    use tauri::Manager;

    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data directory: {e}"))?
        .join(DIR_NAME);
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    Ok(dir.join(FILE_NAME))
}

/// (modified time, length) - what a save changes.
#[cfg(not(mobile))]
fn stamp(path: &std::path::Path) -> Option<(std::time::SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

#[cfg(not(mobile))]
fn watch<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    path: std::path::PathBuf,
    written: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    use tauri::Emitter;

    std::thread::spawn(move || {
        let mut last_stamp = stamp(&path);
        // Content we already know about. Some editors touch the file on open
        // without changing it; that must not count as an edit.
        let mut last_content = written;

        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(POLL_INTERVAL);

            // Missing is normal for a moment during a temp-file-and-rename save.
            let Some(current) = stamp(&path) else { continue };
            if Some(current) == last_stamp {
                continue;
            }
            last_stamp = Some(current);

            if current.1 > MAX_FILE_BYTES {
                eprintln!(
                    "[custom-css] {} is {} bytes, over the {MAX_FILE_BYTES} limit; ignored",
                    path.display(),
                    current.1
                );
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(e) => {
                    eprintln!("[custom-css] reading {}: {e}", path.display());
                    continue;
                }
            };
            if content == last_content {
                continue;
            }
            last_content = content.clone();

            if let Err(e) = app.emit(CHANGED_EVENT, Changed { content }) {
                eprintln!("[custom-css] emitting change: {e}");
            }
        }
    });
}

/// Writes `content` to the custom CSS file, opens it in the default editor and
/// starts watching it. Returns the file's path for display.
#[cfg(not(mobile))]
#[tauri::command]
pub fn custom_css_edit<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, CustomCssWatch>,
    content: String,
) -> Result<String, String> {
    use tauri_plugin_opener::OpenerExt;

    if content.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("stylesheet is {} bytes, over the {MAX_FILE_BYTES} limit", content.len()));
    }

    let path = file_path(&app)?;
    std::fs::write(&path, &content).map_err(|e| format!("writing {}: {e}", path.display()))?;

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let mut guard = state.watcher.lock().map_err(|_| "state poisoned".to_string())?;
        // Dropping the previous watcher stops its thread.
        *guard = Some(Watcher { stop: stop.clone() });
    }
    watch(app.clone(), path.clone(), content, stop);

    let display = path.display().to_string();
    app.opener()
        .open_path(display.clone(), None::<&str>)
        .map_err(|e| format!("opening {display} in the default editor: {e}"))?;

    Ok(display)
}

/// Stops watching the file. The file itself is left in place.
#[cfg(not(mobile))]
#[tauri::command]
pub fn custom_css_stop(state: tauri::State<'_, CustomCssWatch>) -> Result<(), String> {
    let mut guard = state.watcher.lock().map_err(|_| "state poisoned".to_string())?;
    *guard = None;
    Ok(())
}

// Mobile: `generate_handler!` lists the same commands on every target. Android
// edits through the `custom-css-editor` Kotlin plugin instead, and the frontend
// never calls these there.
#[cfg(mobile)]
#[tauri::command]
pub fn custom_css_edit(content: String) -> Result<String, String> {
    let _ = content;
    Err("custom_css_edit is desktop only".to_owned())
}

#[cfg(mobile)]
#[tauri::command]
pub fn custom_css_stop() -> Result<(), String> {
    Ok(())
}
