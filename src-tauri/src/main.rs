// Hide the console window on Windows release builds. Harmless on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod blocked;
mod debug;
mod focus;
mod panel;
mod sessions;
mod state;
mod titles;
mod tray;

use sessions::{Bucket, SessionRegistry, Snapshot};
use state::AppState;
use tauri::Manager;

/// What the panel renders. Taken from the poller rather than read fresh, so
/// opening the panel costs nothing.
#[tauri::command]
fn get_snapshot(state: tauri::State<'_, AppState>) -> Snapshot {
    state
        .latest
        .lock()
        .map(|latest| latest.clone())
        .unwrap_or_default()
}

/// Bring a session's terminal tab to the front.
#[tauri::command]
fn focus_session(state: tauri::State<'_, AppState>, pid: u32) {
    if let Ok(registry) = state.registry.lock() {
        registry.focus(pid);
    }
}

/// Let the panel shrink or grow to fit what it is showing.
#[tauri::command]
fn resize_panel(app: tauri::AppHandle, height: f64) {
    let _ = panel::resize(&app, height);
}

#[tauri::command]
fn hide_panel(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(panel::PANEL_LABEL) {
        let _ = window.hide();
    }
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

fn main() {
    // `--counts` prints what the panel would show and exits, so you can check
    // what the app is reading — handy for scripting, or for working out why a
    // number looks wrong.
    if std::env::args().any(|arg| arg == "--counts") {
        print_snapshot();
        return;
    }

    tauri::Builder::default()
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            focus_session,
            resize_panel,
            hide_panel,
            quit_app
        ])
        .setup(|app| {
            // Accessory keeps the app out of the Dock and the app switcher —
            // it lives in the menu bar only.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            panel::create(app.handle())?;
            tray::start(app.handle())?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start Claude Overview");
}

fn print_snapshot() {
    let snapshot = SessionRegistry::new().snapshot();
    println!("Menu bar shows: {}", snapshot.counts.tray_title());
    println!();

    if snapshot.sessions.is_empty() {
        println!("Nothing running");
        return;
    }

    for bucket in Bucket::ALL {
        let total = snapshot.counts.of(bucket);
        if total == 0 {
            continue;
        }
        println!("{} {} — {}", bucket.marker(), bucket.label(), total);
        for session in snapshot
            .sessions
            .iter()
            .filter(|s| s.bucket == bucket.key())
        {
            let mut row = format!("      {} · {}", session.project, session.host);
            if let Some(reason) = &session.waiting_for {
                row.push_str(&format!(" · {reason}"));
            }
            if let Some(title) = &session.title {
                row.push_str(&format!("\n            “{title}”"));
            }
            println!("{row}");
        }
    }
}
