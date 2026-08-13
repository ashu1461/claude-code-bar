//! Spots the moment a session starts waiting on you.
//!
//! This is what the panel reacts to: it opens itself when something crosses
//! into the waiting state, which is the case that actually costs time — a
//! session quietly blocking on a prompt while you are doing something else.

use crate::sessions::{SessionView, Snapshot};
use std::collections::HashMap;

/// Watches for sessions crossing into the waiting state.
///
/// Only crossings count. A session that is already waiting when the app starts
/// is not announced — you did not just get blocked, you were already blocked,
/// and a burst of notifications at launch would train you to ignore them.
pub struct BlockedWatcher {
    previous: HashMap<u32, String>,
    /// Nothing is announced from the very first reading, which establishes
    /// what "before" looked like.
    primed: bool,
}

impl BlockedWatcher {
    pub fn new() -> Self {
        Self {
            previous: HashMap::new(),
            primed: false,
        }
    }

    /// Sessions that were doing something else a moment ago and are now
    /// blocked.
    pub fn newly_blocked<'a>(&mut self, snapshot: &'a Snapshot) -> Vec<&'a SessionView> {
        let mut blocked = Vec::new();

        for session in &snapshot.sessions {
            let was = self.previous.get(&session.pid).map(String::as_str);
            let is_waiting = session.bucket == "waiting";
            // A session we have never seen has not *become* blocked, so it is
            // recorded and left alone.
            if self.primed && is_waiting && was.is_some_and(|was| was != "waiting") {
                blocked.push(session);
            }
        }

        self.previous = snapshot
            .sessions
            .iter()
            .map(|session| (session.pid, session.bucket.to_string()))
            .collect();
        self.primed = true;

        blocked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::Counts;

    fn session(pid: u32, bucket: &'static str) -> SessionView {
        SessionView {
            pid,
            bucket,
            title: None,
            project: format!("project-{pid}"),
            host: "Terminal.app".to_string(),
            waiting_for: None,
            focusable: true,
            precise: true,
        }
    }

    fn snapshot(sessions: Vec<SessionView>) -> Snapshot {
        Snapshot {
            counts: Counts::default(),
            sessions,
        }
    }

    #[test]
    fn says_nothing_on_the_first_reading() {
        let mut watcher = BlockedWatcher::new();
        // Already blocked at launch is not news.
        let first = snapshot(vec![session(1, "waiting"), session(2, "busy")]);
        assert!(watcher.newly_blocked(&first).is_empty());
    }

    #[test]
    fn announces_a_session_that_becomes_blocked() {
        let mut watcher = BlockedWatcher::new();
        watcher.newly_blocked(&snapshot(vec![session(1, "busy")]));

        let now = snapshot(vec![session(1, "waiting")]);
        let blocked = watcher.newly_blocked(&now);
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].pid, 1);
    }

    #[test]
    fn announces_a_session_only_once_while_it_stays_blocked() {
        let mut watcher = BlockedWatcher::new();
        watcher.newly_blocked(&snapshot(vec![session(1, "busy")]));
        assert_eq!(
            watcher
                .newly_blocked(&snapshot(vec![session(1, "waiting")]))
                .len(),
            1
        );
        // Still waiting on the next poll, and every poll after that.
        assert!(watcher
            .newly_blocked(&snapshot(vec![session(1, "waiting")]))
            .is_empty());
        assert!(watcher
            .newly_blocked(&snapshot(vec![session(1, "waiting")]))
            .is_empty());
    }

    #[test]
    fn announces_again_after_the_session_is_unblocked_and_blocks_once_more() {
        let mut watcher = BlockedWatcher::new();
        watcher.newly_blocked(&snapshot(vec![session(1, "busy")]));
        assert_eq!(
            watcher
                .newly_blocked(&snapshot(vec![session(1, "waiting")]))
                .len(),
            1
        );
        assert!(watcher
            .newly_blocked(&snapshot(vec![session(1, "busy")]))
            .is_empty());
        assert_eq!(
            watcher
                .newly_blocked(&snapshot(vec![session(1, "waiting")]))
                .len(),
            1
        );
    }

    #[test]
    fn a_brand_new_session_that_arrives_blocked_is_not_announced() {
        let mut watcher = BlockedWatcher::new();
        watcher.newly_blocked(&snapshot(vec![session(1, "busy")]));
        // Session 2 appears for the first time, already waiting.
        let now = snapshot(vec![session(1, "busy"), session(2, "waiting")]);
        let blocked = watcher.newly_blocked(&now);
        assert!(blocked.is_empty());
    }
}
