//! OS-wide shortcut that shows or hides the main window. Set from Settings →
//! Keybinds (`useToggleWindowShortcut.ts`); off until the user records one.
//!
//! Only this app command can register it. The plugin's own JS commands are not
//! granted to any capability, so the page cannot bind arbitrary global keys.

#[cfg(not(mobile))]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

#[cfg(not(mobile))]
use tauri::{AppHandle, Manager, Runtime};
#[cfg(not(mobile))]
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

#[cfg(not(mobile))]
#[derive(Default)]
pub struct ToggleShortcut {
    current: Mutex<Option<Shortcut>>,
    // Mirrors the frontend's minimizeToTray: hide to the tray when it is on,
    // otherwise minimize, so the window is never left with no way back.
    hide_to_tray: AtomicBool,
}

#[cfg(not(mobile))]
fn toggle_main_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    let visible = window.is_visible().unwrap_or(false);
    let minimized = window.is_minimized().unwrap_or(false);
    let focused = window.is_focused().unwrap_or(false);

    // Only a window the user is looking at goes away. One that is open but
    // behind other windows is brought forward instead, which is what pressing
    // the shortcut from another app means.
    if visible && !minimized && focused {
        let hide = app
            .try_state::<ToggleShortcut>()
            .map(|s| s.hide_to_tray.load(Ordering::Relaxed))
            .unwrap_or(false);
        let _ = if hide {
            window.hide()
        } else {
            window.minimize()
        };
        return;
    }

    // Same order as the single-instance handler: show before focus for a
    // window hidden to the tray, unminimize for a minimized one.
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

/// Replaces the toggle shortcut. `None` or an empty string turns it off.
///
/// Async on purpose: the plugin registers on the main thread and blocks until
/// it has, so a synchronous command (which runs on the main thread) would
/// deadlock.
#[cfg(not(mobile))]
#[tauri::command]
pub async fn set_toggle_window_shortcut<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, ToggleShortcut>,
    shortcut: Option<String>,
    hide_to_tray: bool,
) -> Result<(), String> {
    state.hide_to_tray.store(hide_to_tray, Ordering::Relaxed);

    let next = match shortcut.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(text) => Some(
            text.parse::<Shortcut>()
                .map_err(|e| format!("not a shortcut: {text}: {e}"))?,
        ),
        None => None,
    };

    let mut current = state
        .current
        .lock()
        .map_err(|_| "state poisoned".to_string())?;
    if *current == next {
        return Ok(());
    }

    if let Some(old) = current.take() {
        if let Err(e) = app.global_shortcut().unregister(old) {
            eprintln!("[toggle-shortcut] could not unregister {old}: {e}");
        }
    }

    if let Some(new) = next {
        app.global_shortcut()
            .on_shortcut(new, |app, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    toggle_main_window(app);
                }
            })
            .map_err(|e| format!("could not register {new}: {e}"))?;
        *current = Some(new);
    }

    Ok(())
}

// `generate_handler!` lists the same commands on every target; there is no
// global keyboard on a phone.
#[cfg(mobile)]
#[tauri::command]
pub fn set_toggle_window_shortcut(
    shortcut: Option<String>,
    hide_to_tray: bool,
) -> Result<(), String> {
    let _ = (shortcut, hide_to_tray);
    Err("set_toggle_window_shortcut is desktop only".to_owned())
}
