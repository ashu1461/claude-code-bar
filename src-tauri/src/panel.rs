//! The dropdown panel.
//!
//! macOS menus can only render plain rows of text, so the panel is a small
//! borderless window instead: it can carry typography, colour, and grouping
//! the way a menu never could. It behaves like a menu — appears under the
//! menu bar icon, closes as soon as it loses focus.

use crate::debug;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, PhysicalPosition, Runtime, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

pub const PANEL_LABEL: &str = "panel";

const PANEL_WIDTH: f64 = 380.0;
const PANEL_HEIGHT: f64 = 520.0;

/// Distance kept between the menu bar and the top of the panel.
const MENU_BAR_GAP: f64 = 6.0;

/// Build the panel up front and keep it hidden, so opening it is instant
/// rather than paying for a webview launch on the first click.
pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    let window = WebviewWindowBuilder::new(app, PANEL_LABEL, WebviewUrl::App("index.html".into()))
        .title("Claude Overview")
        .inner_size(PANEL_WIDTH, PANEL_HEIGHT)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        // Without this the panel belongs to the desktop it was created on, so
        // opening it from another Space would either do nothing visible or
        // drag you across.
        .visible_on_all_workspaces(true)
        .build()?;

    // The translucent menu-like background, and the rounded corners that go
    // with it. Failing here costs looks, not function, so it is not fatal.
    #[cfg(target_os = "macos")]
    {
        use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
        if let Err(error) = apply_vibrancy(
            &window,
            NSVisualEffectMaterial::Popover,
            Some(NSVisualEffectState::Active),
            Some(12.0),
        ) {
            eprintln!("claude-overview: could not apply the panel background: {error}");
        }
    }

    // Opening the panel normally needs a click on the menu bar icon, which is
    // awkward when you are working on how the panel looks.
    if std::env::var("CLAUDE_OVERVIEW_SHOW_PANEL").is_ok_and(|value| !value.is_empty()) {
        let _ = window.set_position(PhysicalPosition::new(200.0, 100.0));
        let _ = window.show();
        return Ok(window);
    }

    // A menu closes when you click away, and so should this.
    //
    // The grace period matters: an app with no Dock icon does not always
    // become active the instant it asks to, so the panel can be told it lost
    // focus in the same breath as being shown. Without this it would hide
    // itself before you ever saw it, and the click would look like it did
    // nothing at all.
    let hidden = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            if last_shown_elapsed() < FOCUS_GRACE {
                debug::log("panel lost focus within grace period -> staying open");
                return;
            }
            debug::log("panel lost focus -> hiding");
            let _ = hidden.hide();
        }
    });

    Ok(window)
}

/// How long after opening the panel a focus loss is ignored.
const FOCUS_GRACE: Duration = Duration::from_millis(500);

fn last_shown() -> &'static Mutex<Option<Instant>> {
    static LAST_SHOWN: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    LAST_SHOWN.get_or_init(|| Mutex::new(None))
}

fn mark_shown() {
    if let Ok(mut shown) = last_shown().lock() {
        *shown = Some(Instant::now());
    }
}

fn last_shown_elapsed() -> Duration {
    last_shown()
        .lock()
        .ok()
        .and_then(|shown| shown.map(|at| at.elapsed()))
        .unwrap_or(Duration::MAX)
}

/// Where the menu bar icon last was, so the panel can open itself in the
/// right place without waiting for a click to tell it.
fn anchor() -> &'static Mutex<Option<(f64, f64)>> {
    static ANCHOR: OnceLock<Mutex<Option<(f64, f64)>>> = OnceLock::new();
    ANCHOR.get_or_init(|| Mutex::new(None))
}

/// Tray events carry the icon's rectangle, so every one of them — hover
/// included — keeps this current.
pub fn remember_anchor(centre_x: f64, bottom_y: f64) {
    if let Ok(mut anchor) = anchor().lock() {
        *anchor = Some((centre_x, bottom_y));
    }
}

/// Whether the panel is up because it opened itself rather than because the
/// icon was clicked. Only a deliberate open may take focus, and so only a
/// deliberate open dismisses itself on click-away.
static AUTO_OPENED: AtomicBool = AtomicBool::new(false);

/// Open the panel by itself when something starts waiting.
///
/// Deliberately does not call `set_focus`: the panel appears while you are
/// typing, and taking focus would send your next keystrokes here instead of
/// wherever you were working.
pub fn auto_open<R: Runtime>(window: &WebviewWindow<R>) {
    if window.is_visible().unwrap_or(false) {
        return;
    }

    match anchor().lock().ok().and_then(|anchor| *anchor) {
        Some((x, y)) => position(window, x, y),
        // Nobody has been near the icon yet, so fall back to the top-right
        // of the main display, which is where the icon almost certainly is.
        None => position_top_right(window),
    }

    AUTO_OPENED.store(true, Ordering::Relaxed);
    mark_shown();
    let _ = window.show();
    debug::log("panel auto-opened for a blocked session");
}

/// Show the panel under the menu bar icon, or hide it if it is already up.
pub fn toggle<R: Runtime>(window: &WebviewWindow<R>, icon_centre_x: f64, icon_bottom_y: f64) {
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        AUTO_OPENED.store(false, Ordering::Relaxed);
        return;
    }
    position(window, icon_centre_x, icon_bottom_y);
    // Opened deliberately, so it may take focus and dismiss on click-away.
    AUTO_OPENED.store(false, Ordering::Relaxed);
    mark_shown();
    let shown = window.show();
    let focused = window.set_focus();
    debug::log(format!(
        "toggle -> show={:?} focus={:?} visible={:?} pos={:?} size={:?}",
        shown.is_ok(),
        focused.is_ok(),
        window.is_visible(),
        window.outer_position(),
        window.outer_size()
    ));
}

/// Centre the panel on the menu bar icon, keeping it fully on screen.
///
/// Displays can have different scale factors, and the panel may currently be
/// parked on one while the menu bar icon lives on another. So everything is
/// worked out against the display the *icon* is on, and the final move is
/// made in logical points: macOS lays screens out in points, and asking for
/// physical pixels would be re-scaled through whichever display the window
/// happens to be sitting on.
fn position<R: Runtime>(window: &WebviewWindow<R>, icon_centre_x: f64, icon_bottom_y: f64) {
    // Both the icon rectangle and the monitor geometry come from the same
    // physical coordinate space, so they can be compared directly.
    let screen = monitor_containing(window, icon_centre_x, icon_bottom_y);
    let scale = screen
        .as_ref()
        .map(|m| m.scale_factor())
        .unwrap_or_else(|| window.scale_factor().unwrap_or(1.0));

    let width = PANEL_WIDTH * scale;
    let mut x = icon_centre_x - width / 2.0;
    let y = icon_bottom_y + MENU_BAR_GAP * scale;

    // A status item near the right edge would otherwise push the panel off
    // the side of its screen.
    if let Some(screen) = &screen {
        let left = screen.position().x as f64;
        let right = left + screen.size().width as f64;
        let margin = 8.0 * scale;
        x = x.clamp(left + margin, (right - width - margin).max(left + margin));
    }

    debug::log(format!(
        "position -> icon=({icon_centre_x}, {icon_bottom_y}) scale={scale} \
         screen={:?} physical=({x}, {y}) logical=({}, {})",
        screen.as_ref().map(|m| (
            m.position().x,
            m.position().y,
            m.size().width,
            m.size().height
        )),
        x / scale,
        y / scale,
    ));

    let _ = window.set_position(LogicalPosition::new(x / scale, y / scale));
}

/// Fall back to the top-right of the main display, where the menu bar icon
/// lives even if we have not seen an event from it yet.
fn position_top_right<R: Runtime>(window: &WebviewWindow<R>) {
    let Ok(Some(screen)) = window.primary_monitor() else {
        return;
    };
    let scale = screen.scale_factor();
    let right = screen.position().x as f64 + screen.size().width as f64;
    let top = screen.position().y as f64;
    // Roughly the height of the menu bar, so the panel clears it.
    let x = right - PANEL_WIDTH * scale - 8.0 * scale;
    let y = top + (24.0 + MENU_BAR_GAP) * scale;
    let _ = window.set_position(LogicalPosition::new(x / scale, y / scale));
}

/// The display holding a given point, in physical coordinates.
fn monitor_containing<R: Runtime>(
    window: &WebviewWindow<R>,
    x: f64,
    y: f64,
) -> Option<tauri::Monitor> {
    let monitors = window.available_monitors().ok()?;
    monitors.into_iter().find(|monitor| {
        let position = monitor.position();
        let size = monitor.size();
        let left = position.x as f64;
        let top = position.y as f64;
        x >= left && x < left + size.width as f64 && y >= top && y < top + size.height as f64
    })
}

/// Let the panel size itself to its content, so a single session does not sit
/// in a tall empty box and a long list is not needlessly cramped.
pub fn resize<R: Runtime>(app: &AppHandle<R>, height: f64) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window(PANEL_LABEL) else {
        return Ok(());
    };
    let height = height.clamp(120.0, PANEL_HEIGHT);
    window.set_size(LogicalSize::new(PANEL_WIDTH, height))
}
