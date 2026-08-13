//! Opt-in diagnostics. Set `CLAUDE_OVERVIEW_DEBUG=1` to trace what the menu
//! bar item and the panel are doing, which is the quickest way to tell a
//! click that never arrived from a panel that opened somewhere unhelpful.

use std::sync::OnceLock;

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED
        .get_or_init(|| std::env::var("CLAUDE_OVERVIEW_DEBUG").is_ok_and(|value| !value.is_empty()))
}

pub fn log(message: impl AsRef<str>) {
    if enabled() {
        eprintln!("[claude-overview] {}", message.as_ref());
    }
}
