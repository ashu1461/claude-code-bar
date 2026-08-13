//! Which project folders are currently open in an editor.
//!
//! This exists to stop a click creating windows. Handing a folder to an
//! editor means "open this", so if the folder is not already open you get a
//! brand new window instead of being taken to your work. Knowing what is
//! already open lets us pass a folder only when doing so will focus an
//! existing window, and otherwise just bring the application forward.
//!
//! Claude Code's editor extensions advertise themselves in
//! `~/.claude/ide/<pid>.lock`, which conveniently records the workspace each
//! one has open.

use serde::Deserialize;
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdeLock {
    pid: u32,
    #[serde(default)]
    workspace_folders: Vec<String>,
}

/// The set of folders open in a running editor right now.
#[derive(Debug, Default, Clone)]
pub struct OpenWorkspaces {
    folders: HashSet<String>,
}

impl OpenWorkspaces {
    /// Read the lock files, keeping only those whose editor is still running.
    /// Editors leave their locks behind, exactly as sessions do.
    pub fn read(is_running: impl Fn(u32) -> bool) -> Self {
        Self::read_from(default_ide_dir(), is_running)
    }

    pub fn read_from(dir: PathBuf, is_running: impl Fn(u32) -> bool) -> Self {
        let mut folders = HashSet::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Self { folders };
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("lock") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(lock) = serde_json::from_str::<IdeLock>(&text) else {
                continue;
            };
            if !is_running(lock.pid) {
                continue;
            }
            for folder in lock.workspace_folders {
                if !folder.is_empty() {
                    folders.insert(normalise(&folder));
                }
            }
        }

        Self { folders }
    }

    /// Whether opening this folder will land on a window that already exists.
    pub fn is_open(&self, folder: &str) -> bool {
        self.folders.contains(&normalise(folder))
    }
}

/// Trailing slashes differ between what a session records and what an editor
/// reports, and would otherwise cause a false miss.
fn normalise(path: &str) -> String {
    path.trim_end_matches('/').to_string()
}

fn default_ide_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_OVERVIEW_IDE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".claude").join("ide")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("claude-overview-ide-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_lock(dir: &PathBuf, pid: u32, folders: &[&str]) {
        let list = folders
            .iter()
            .map(|f| format!("\"{f}\""))
            .collect::<Vec<_>>()
            .join(",");
        let mut file = std::fs::File::create(dir.join(format!("{pid}.lock"))).unwrap();
        write!(
            file,
            r#"{{"pid":{pid},"workspaceFolders":[{list}],"ideName":"Visual Studio Code"}}"#
        )
        .unwrap();
    }

    #[test]
    fn reports_folders_open_in_a_running_editor() {
        let dir = scratch("running");
        write_lock(&dir, 100, &["/Users/me/project-a"]);
        let open = OpenWorkspaces::read_from(dir, |_| true);
        assert!(open.is_open("/Users/me/project-a"));
        assert!(!open.is_open("/Users/me/somewhere-else"));
    }

    #[test]
    fn ignores_locks_left_behind_by_editors_that_have_quit() {
        let dir = scratch("stale");
        write_lock(&dir, 100, &["/Users/me/gone"]);
        // The whole point: a stale lock must not convince us a window exists,
        // or the click opens a new one.
        let open = OpenWorkspaces::read_from(dir, |_| false);
        assert!(!open.is_open("/Users/me/gone"));
    }

    #[test]
    fn matches_regardless_of_a_trailing_slash() {
        let dir = scratch("slash");
        write_lock(&dir, 100, &["/Users/me/project-a/"]);
        let open = OpenWorkspaces::read_from(dir, |_| true);
        assert!(open.is_open("/Users/me/project-a"));
        assert!(open.is_open("/Users/me/project-a/"));
    }

    #[test]
    fn copes_with_an_editor_reporting_no_workspace() {
        let dir = scratch("empty");
        write_lock(&dir, 100, &[]);
        let open = OpenWorkspaces::read_from(dir, |_| true);
        assert!(!open.is_open(""));
    }

    #[test]
    fn a_missing_directory_means_nothing_is_open() {
        let open = OpenWorkspaces::read_from(PathBuf::from("/nonexistent/xyz"), |_| true);
        assert!(!open.is_open("/Users/me/project-a"));
    }
}
