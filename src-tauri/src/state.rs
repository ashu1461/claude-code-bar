//! Shared state: the registry, and the most recent snapshot taken from it.

use crate::sessions::{SessionRegistry, Snapshot};
use std::sync::Mutex;

pub struct AppState {
    /// Guarded because the polling thread and the panel's commands both reach
    /// for it.
    pub registry: Mutex<SessionRegistry>,
    /// The last snapshot the poller took, so opening the panel shows current
    /// data immediately rather than waiting for the next tick.
    pub latest: Mutex<Snapshot>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            registry: Mutex::new(SessionRegistry::new()),
            latest: Mutex::new(Snapshot::default()),
        }
    }
}
