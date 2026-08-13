//! The menu bar item: a compact tally in the bar, and the click that opens
//! the panel.

use crate::blocked::BlockedWatcher;
use crate::debug;
use crate::panel;
use crate::sessions::Snapshot;
use crate::state::AppState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

/// Claude Code rewrites a session file the moment its state changes, so this
/// is just how quickly we notice. Two seconds keeps the bar responsive while
/// costing a directory listing of a handful of small files.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

const TRAY_ID: &str = "claude-overview";
const QUIT_MENU_ID: &str = "quit";

/// Create the menu bar item and start the loop that keeps it current.
pub fn start<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    // Left click opens the panel; the menu is kept for right click, so there
    // is always an obvious way out of the app.
    let quit = MenuItemBuilder::with_id(QUIT_MENU_ID, "Quit Claude Overview")
        .accelerator("Cmd+Q")
        .build(app)?;
    let menu = MenuBuilder::new(app).item(&quit).build()?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon_as_template(false)
        .title("…")
        .tooltip("Claude Code sessions")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            if event.id() == QUIT_MENU_ID {
                app.exit(0);
            }
        })
        .on_tray_icon_event(|tray, event| {
            debug::log(format!("tray event: {event:?}"));
            // Every event carries the icon's rectangle, hover included, which
            // is how the panel knows where to open itself later.
            if let Some(rect) = tray_rect(&event) {
                panel::remember_anchor(rect.0, rect.1);
            }

            let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            else {
                return;
            };

            let app = tray.app_handle();
            let Some(window) = app.get_webview_window(panel::PANEL_LABEL) else {
                debug::log("no panel window found");
                return;
            };

            // Anchor the panel to the middle of the icon's bottom edge.
            let scale = window.scale_factor().unwrap_or(1.0);
            let position = rect.position.to_physical::<f64>(scale);
            let size = rect.size.to_physical::<f64>(scale);
            panel::toggle(
                &window,
                position.x + size.width / 2.0,
                position.y + size.height,
            );
        })
        .build(app)?;

    spawn_poller(app.clone());
    spawn_alert_animation(app.clone());
    Ok(())
}

/// Re-read the registry on a timer and push the tally into the menu bar.
fn spawn_poller<R: Runtime>(app: AppHandle<R>) {
    // Ends when the registry lock is poisoned by a panic elsewhere, or when
    // the app shuts down and stops accepting main-thread work.
    std::thread::spawn(move || {
        let mut watcher = BlockedWatcher::new();
        while let Some(snapshot) = take_snapshot(&app) {
            let should_open = !watcher.newly_blocked(&snapshot).is_empty();

            if should_open {
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || {
                    // Ask the icon where it is rather than relying on having
                    // seen an event from it, which matters right after launch.
                    if let Some(tray) = handle.tray_by_id(TRAY_ID) {
                        if let Ok(Some(rect)) = tray.rect() {
                            let position = rect.position.to_physical::<f64>(1.0);
                            let size = rect.size.to_physical::<f64>(1.0);
                            panel::remember_anchor(
                                position.x + size.width / 2.0,
                                position.y + size.height,
                            );
                        }
                    }
                    if let Some(window) = handle.get_webview_window(panel::PANEL_LABEL) {
                        panel::auto_open(&window);
                    }
                });
            }

            let title = snapshot.counts.tray_title();
            // The animation runs on its own timer, so the poll only reports
            // whether anything is blocked.
            ALARM.store(snapshot.counts.waiting > 0, Ordering::Relaxed);
            store_snapshot(&app, snapshot);

            // Tray updates have to happen on the main thread.
            let handle = app.clone();
            let dispatched = app.run_on_main_thread(move || {
                if let Some(tray) = handle.tray_by_id(TRAY_ID) {
                    if let Err(error) = tray.set_title(Some(title)) {
                        eprintln!("claude-overview: could not update the menu bar: {error}");
                    }
                }
            });
            if dispatched.is_err() {
                break; // The app is shutting down.
            }

            std::thread::sleep(POLL_INTERVAL);
        }
    });
}

/// Whether anything is currently blocked. Shared so the animation can run on
/// a faster timer than the two-second poll.
static ALARM: AtomicBool = AtomicBool::new(false);

/// How long each dot stays lit.
const FRAME_INTERVAL: Duration = Duration::from_millis(380);

/// Three red dots beside the counts, with the bright one travelling along
/// them. macOS has no animated menu bar image, so the frames are swapped by
/// hand — cheap, since each is a few hundred bytes and only runs while
/// something is actually blocked.
fn spawn_alert_animation<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || {
        let mut frame = 0usize;
        // Starts true so the first pass paints the resting mark, then tracks
        // whether the dots are up so they are removed exactly once.
        let mut showing = true;

        loop {
            let alarming = ALARM.load(Ordering::Relaxed);
            if alarming || showing {
                // The mark is always there so the item is recognisably Claude
                // Code; only the dots beside it come and go.
                let icon = if alarming {
                    alert_frame(frame)
                } else {
                    claude_mark()
                };
                let handle = app.clone();
                if app
                    .run_on_main_thread(move || {
                        if let Some(tray) = handle.tray_by_id(TRAY_ID) {
                            let _ = tray.set_icon(icon);
                        }
                    })
                    .is_err()
                {
                    break; // The app is shutting down.
                }
                showing = alarming;
                frame = (frame + 1) % ALERT_FRAMES.len();
            }
            std::thread::sleep(FRAME_INTERVAL);
        }
    });
}

const ALERT_FRAMES: [&[u8]; 3] = [
    include_bytes!("../icons/tray-alert-0.png"),
    include_bytes!("../icons/tray-alert-1.png"),
    include_bytes!("../icons/tray-alert-2.png"),
];

/// The resting menu bar icon: the Claude mark, so the item is identifiable
/// among a row of anonymous status items.
fn claude_mark() -> Option<tauri::image::Image<'static>> {
    static MARK: OnceLock<Option<tauri::image::Image<'static>>> = OnceLock::new();
    MARK.get_or_init(|| {
        match tauri::image::Image::from_bytes(include_bytes!("../icons/tray-claude.png")) {
            Ok(image) => Some(image),
            Err(error) => {
                eprintln!("claude-overview: could not load the menu bar mark: {error}");
                None
            }
        }
    })
    .clone()
}

/// Decode the frames once and reuse them, since they are set several times a
/// second while a session is blocked.
fn alert_frame(index: usize) -> Option<tauri::image::Image<'static>> {
    static FRAMES: OnceLock<Vec<tauri::image::Image<'static>>> = OnceLock::new();
    FRAMES
        .get_or_init(|| {
            ALERT_FRAMES
                .iter()
                .filter_map(|bytes| match tauri::image::Image::from_bytes(bytes) {
                    Ok(image) => Some(image),
                    Err(error) => {
                        eprintln!("claude-overview: could not load an alert frame: {error}");
                        None
                    }
                })
                .collect()
        })
        .get(index)
        .cloned()
}

/// Read the registry. Kept separate so the borrow of the managed state ends
/// before the snapshot is used.
fn take_snapshot<R: Runtime>(app: &AppHandle<R>) -> Option<Snapshot> {
    let state = app.state::<AppState>();
    let mut registry = state.registry.lock().ok()?;
    Some(registry.snapshot())
}

fn store_snapshot<R: Runtime>(app: &AppHandle<R>, snapshot: Snapshot) {
    let state = app.state::<AppState>();
    // `inner` hands back a reference tied to the app rather than to the local
    // handle, which is what lets the lock guard outlive this binding.
    if let Ok(mut latest) = state.inner().latest.lock() {
        *latest = snapshot;
    }
}

/// The centre-bottom of the tray icon, in physical coordinates, from whichever
/// event carried it.
fn tray_rect(event: &TrayIconEvent) -> Option<(f64, f64)> {
    let rect = match event {
        TrayIconEvent::Click { rect, .. }
        | TrayIconEvent::DoubleClick { rect, .. }
        | TrayIconEvent::Enter { rect, .. }
        | TrayIconEvent::Move { rect, .. }
        | TrayIconEvent::Leave { rect, .. } => rect,
        _ => return None,
    };
    let position = rect.position.to_physical::<f64>(1.0);
    let size = rect.size.to_physical::<f64>(1.0);
    Some((position.x + size.width / 2.0, position.y + size.height))
}
