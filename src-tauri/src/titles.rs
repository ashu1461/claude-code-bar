//! Finds the human-readable title Claude Code gives each session.
//!
//! The session registry only carries a name derived from the directory, like
//! `my-project-c2`. Claude Code also writes a generated title describing what
//! the session is actually about — "Build Mac menu bar app for Claude session
//! monitoring" — into the session transcript at
//! `~/.claude/projects/<project>/<sessionId>.jsonl`, as repeated lines of
//! `{"type":"ai-title","aiTitle":"…"}`. The last one wins.
//!
//! Transcripts grow to megabytes, so we never read one whole: only the tail,
//! and only when the file has changed since we last looked.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::SystemTime;

/// How much of the end of a transcript to scan. Titles are rewritten on most
/// turns, so the newest one sits very close to the end.
const TAIL_BYTES: u64 = 128 * 1024;

/// What we remember about one transcript between polls.
struct CachedTitle {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
    title: Option<String>,
}

impl CachedTitle {
    /// A transcript that has neither grown nor been touched cannot have a new
    /// title, so the cached answer still stands.
    fn is_current(&self, len: u64, modified: Option<SystemTime>) -> bool {
        self.len == len && self.modified == modified
    }
}

/// Looks up session titles, remembering what it has already read.
pub struct TitleCache {
    projects_dir: PathBuf,
    entries: HashMap<String, CachedTitle>,
}

impl TitleCache {
    pub fn new() -> Self {
        Self::with_dir(default_projects_dir())
    }

    pub fn with_dir(projects_dir: PathBuf) -> Self {
        Self {
            projects_dir,
            entries: HashMap::new(),
        }
    }

    /// The title for a session, or `None` if it has not been given one yet.
    pub fn title_for(&mut self, session_id: &str) -> Option<String> {
        let path = match self.entries.get(session_id) {
            Some(cached) => cached.path.clone(),
            None => self.locate(session_id)?,
        };

        let (len, modified) = match path.metadata() {
            Ok(meta) => (meta.len(), meta.modified().ok()),
            // The transcript vanished; forget it and try to find it again next
            // time rather than holding a stale title.
            Err(_) => {
                self.entries.remove(session_id);
                return None;
            }
        };

        if let Some(cached) = self.entries.get(session_id) {
            if cached.is_current(len, modified) {
                return cached.title.clone();
            }
        }

        let title = read_last_title(&path);
        self.entries.insert(
            session_id.to_string(),
            CachedTitle {
                path,
                len,
                modified,
                title: title.clone(),
            },
        );
        title
    }

    /// Transcripts live under a directory named after the project, which we
    /// would have to reconstruct from the session's path. Searching the
    /// project directories for the file is simpler and immune to however
    /// Claude Code chooses to mangle a path into a directory name.
    fn locate(&self, session_id: &str) -> Option<PathBuf> {
        let file_name = format!("{session_id}.jsonl");
        let entries = std::fs::read_dir(&self.projects_dir).ok()?;
        for entry in entries.flatten() {
            let candidate = entry.path().join(&file_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }
}

/// Scan the tail of a transcript for the most recent title.
fn read_last_title(path: &PathBuf) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;

    let mut tail = String::new();
    // Transcripts are UTF-8, but seeking to a fixed offset can land mid
    // character, so read bytes and convert lossily rather than failing.
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    tail.push_str(&String::from_utf8_lossy(&bytes));

    let mut lines: Vec<&str> = tail.lines().collect();
    // When we seeked, the first line is almost certainly a fragment.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }

    for line in lines.iter().rev() {
        if !line.contains("ai-title") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|t| t.as_str()) != Some("ai-title") {
            continue;
        }
        if let Some(title) = value.get("aiTitle").and_then(|t| t.as_str()) {
            let title = title.trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }
    None
}

fn default_projects_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_OVERVIEW_PROJECTS_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".claude").join("projects")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("claude-overview-titles-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("project-a")).unwrap();
        dir
    }

    fn write_transcript(dir: &PathBuf, session_id: &str, lines: &[&str]) -> PathBuf {
        let path = dir.join("project-a").join(format!("{session_id}.jsonl"));
        let mut file = File::create(&path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        path
    }

    #[test]
    fn finds_the_most_recent_title() {
        let dir = scratch("recent");
        write_transcript(
            &dir,
            "abc",
            &[
                r#"{"type":"user","message":{"content":"hello"}}"#,
                r#"{"type":"ai-title","aiTitle":"An early guess","sessionId":"abc"}"#,
                r#"{"type":"assistant"}"#,
                r#"{"type":"ai-title","aiTitle":"The settled title","sessionId":"abc"}"#,
            ],
        );
        let mut cache = TitleCache::with_dir(dir);
        assert_eq!(cache.title_for("abc").as_deref(), Some("The settled title"));
    }

    #[test]
    fn returns_nothing_when_the_session_has_no_title_yet() {
        let dir = scratch("untitled");
        write_transcript(
            &dir,
            "def",
            &[r#"{"type":"user","message":{"content":"hi"}}"#],
        );
        let mut cache = TitleCache::with_dir(dir);
        assert_eq!(cache.title_for("def"), None);
    }

    #[test]
    fn returns_nothing_for_a_session_with_no_transcript() {
        let dir = scratch("missing");
        let mut cache = TitleCache::with_dir(dir);
        assert_eq!(cache.title_for("nope"), None);
    }

    #[test]
    fn picks_up_a_title_added_after_the_first_look() {
        let dir = scratch("updated");
        let path = write_transcript(&dir, "ghi", &[r#"{"type":"user"}"#]);
        let mut cache = TitleCache::with_dir(dir);
        assert_eq!(cache.title_for("ghi"), None);

        // Appending has to invalidate the cache, or the menu would never
        // notice a session being titled.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, r#"{{"type":"ai-title","aiTitle":"Named at last"}}"#).unwrap();
        drop(file);

        assert_eq!(cache.title_for("ghi").as_deref(), Some("Named at last"));
    }

    #[test]
    fn survives_a_transcript_larger_than_the_tail_window() {
        let dir = scratch("large");
        let filler = format!(r#"{{"type":"assistant","text":"{}"}}"#, "x".repeat(4000));
        let mut lines: Vec<&str> = Vec::new();
        let early = r#"{"type":"ai-title","aiTitle":"Buried far too early"}"#;
        lines.push(early);
        for _ in 0..80 {
            lines.push(&filler);
        }
        let late = r#"{"type":"ai-title","aiTitle":"Near the end"}"#;
        lines.push(late);
        write_transcript(&dir, "jkl", &lines);

        let mut cache = TitleCache::with_dir(dir);
        assert_eq!(cache.title_for("jkl").as_deref(), Some("Near the end"));
    }
}
