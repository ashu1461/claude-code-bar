//! Status for sessions that do not publish any.
//!
//! Editor-extension sessions register themselves in `~/.claude/sessions` and
//! then go quiet: no `status`, and no writes at all while they run. There is
//! nothing on disk to read, so the app cannot say whether one is working or
//! blocked.
//!
//! Claude Code's hooks fill that gap. They are part of Claude Code itself
//! rather than any one front end, so they fire wherever a session runs. Each
//! hook invokes this binary with `--hook`, which records the session's state
//! in `~/.claude/claude-code-bar/<session_id>.json`. The registry still
//! supplies the process ID and the liveness check; hooks supply only the
//! status it is missing.
//!
//! Sessions that already publish a status — anything in a terminal — are
//! unaffected: the registry always wins.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The events worth listening to, and what each one means.
///
/// `PreToolUse` is deliberately absent: it fires on every tool call, and each
/// firing spawns a process. The three below change state, which is all we
/// need.
const HOOK_EVENTS: [&str; 4] = ["UserPromptSubmit", "Notification", "Stop", "SessionEnd"];

/// What a hook told us about a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookStatus {
    /// `waiting`, `busy` or `idle`, matching the registry's vocabulary.
    pub status: String,
    #[serde(default)]
    pub reason: Option<String>,
    /// Epoch seconds, for pruning long-dead files.
    #[serde(default)]
    pub updated_at: u64,
}

/// The payload Claude Code pipes into a hook. Only the fields we use.
#[derive(Debug, Deserialize)]
struct HookInput {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    hook_event_name: Option<String>,
    #[serde(default)]
    notification_type: Option<String>,
}

/// Handle one hook invocation: read the event from stdin, record the state.
///
/// Anything unexpected exits quietly. A hook that fails loudly would put
/// errors in front of someone using Claude Code, which is far worse than this
/// app briefly not knowing a session's state.
pub fn record_event() {
    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        return;
    }
    let Ok(input) = serde_json::from_str::<HookInput>(&raw) else {
        return;
    };
    let Some(session_id) = input.session_id.filter(|id| is_safe_id(id)) else {
        return;
    };

    let Some(event) = input.hook_event_name.as_deref() else {
        return;
    };

    let dir = state_dir();
    let path = dir.join(format!("{session_id}.json"));

    // The session is over, so its state file should go with it.
    if event == "SessionEnd" {
        let _ = std::fs::remove_file(path);
        return;
    }

    let status = match event {
        "UserPromptSubmit" => "busy",
        "Notification" => "waiting",
        "Stop" => "idle",
        _ => return,
    };

    let state = HookStatus {
        status: status.to_string(),
        reason: (status == "waiting")
            .then(|| describe(input.notification_type.as_deref()))
            .flatten(),
        updated_at: now_secs(),
    };

    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(text) = serde_json::to_string(&state) {
        let _ = std::fs::write(path, text);
    }
}

/// Turn a notification type into something worth showing on a row.
fn describe(notification_type: Option<&str>) -> Option<String> {
    let raw = notification_type?;
    let readable = raw.replace('_', " ");
    let reason = match readable.as_str() {
        r if r.contains("permission") => "permission prompt".to_string(),
        r if r.contains("input") => "input needed".to_string(),
        "" => return None,
        other => other.to_string(),
    };
    Some(reason)
}

/// Everything the hooks have recorded, keyed by session id.
#[derive(Debug, Default)]
pub struct HookStates {
    states: HashMap<String, HookStatus>,
}

impl HookStates {
    pub fn read() -> Self {
        Self::read_from(&state_dir())
    }

    pub fn read_from(dir: &Path) -> Self {
        let mut states = HashMap::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Self { states };
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(session_id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Ok(state) = serde_json::from_str::<HookStatus>(&text) {
                states.insert(session_id.to_string(), state);
            }
        }
        Self { states }
    }

    pub fn get(&self, session_id: &str) -> Option<&HookStatus> {
        self.states.get(session_id)
    }
}

/// Session ids are used as filenames, so they must not be able to escape the
/// directory.
fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn claude_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_OVERVIEW_CLAUDE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".claude")
}

fn state_dir() -> PathBuf {
    claude_dir().join("claude-code-bar")
}

// ---------------------------------------------------------------------------
// Installing the hooks
// ---------------------------------------------------------------------------

/// Add the hooks to `~/.claude/settings.json`, if they are not already there.
///
/// This edits a file the user owns, so it is careful about it: the settings
/// are parsed and re-serialised whole, so anything already configured is kept
/// exactly as it was; a backup is taken before the first change; the write is
/// atomic; and it does nothing at all when the hooks are already correct.
///
/// Set `CLAUDE_CODE_BAR_NO_HOOK_INSTALL=1` to skip it entirely.
pub fn ensure_installed() {
    if std::env::var("CLAUDE_CODE_BAR_NO_HOOK_INSTALL").is_ok_and(|v| !v.is_empty()) {
        return;
    }
    let Some(command) = hook_command() else {
        return;
    };
    let settings = claude_dir().join("settings.json");

    let mut root = match std::fs::read_to_string(&settings) {
        Ok(text) => serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({})),
        Err(_) => json!({}),
    };
    if !root.is_object() {
        return;
    }

    let changed = apply(&mut root, &command);
    if !changed {
        return;
    }

    // Keep one copy of whatever was there before we first touched it.
    let backup = settings.with_extension("json.before-claude-code-bar");
    if settings.exists() && !backup.exists() {
        let _ = std::fs::copy(&settings, &backup);
    }

    let Ok(text) = serde_json::to_string_pretty(&root) else {
        return;
    };
    // Write beside the target and rename, so a crash cannot leave the user
    // with a truncated settings file.
    let temp = settings.with_extension("json.claude-code-bar-tmp");
    if std::fs::write(&temp, text + "\n").is_ok() && std::fs::rename(&temp, &settings).is_ok() {
        eprintln!("claude-code-bar: installed status hooks into {settings:?}");
    } else {
        let _ = std::fs::remove_file(&temp);
    }
}

/// Insert our command into each event, leaving every other hook untouched.
/// Returns whether anything actually changed.
fn apply(root: &mut Value, command: &str) -> bool {
    let mut changed = false;
    let hooks = root
        .as_object_mut()
        .expect("checked above")
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        return false;
    }

    for event in HOOK_EVENTS {
        let entries = hooks
            .as_object_mut()
            .expect("checked above")
            .entry(event)
            .or_insert_with(|| json!([]));
        let Some(list) = entries.as_array_mut() else {
            continue;
        };

        // Ours is recognised by the binary it points at, so a moved app
        // updates its own entry rather than adding a second one.
        let mine = list.iter_mut().find(|group| mentions_us(group));
        match mine {
            Some(group) => {
                if !points_at(group, command) {
                    *group = hook_group(command);
                    changed = true;
                }
            }
            None => {
                list.push(hook_group(command));
                changed = true;
            }
        }
    }
    changed
}

fn hook_group(command: &str) -> Value {
    json!({ "hooks": [ { "type": "command", "command": command } ] })
}

/// Any command mentioning this app, whatever path it currently has.
fn mentions_us(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .is_some_and(|hooks| {
            hooks.iter().any(|hook| {
                hook.get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| c.contains(HOOK_MARKER))
            })
        })
}

fn points_at(group: &Value, command: &str) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .is_some_and(|hooks| {
            hooks
                .iter()
                .any(|hook| hook.get("command").and_then(|c| c.as_str()) == Some(command))
        })
}

/// Distinctive enough to recognise our own entry, stable across app moves.
const HOOK_MARKER: &str = "--hook";

/// The command a hook should run: this very binary.
fn hook_command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.to_str()?;
    // Paths contain spaces — "/Applications/Claude Overview.app/…".
    Some(format!("\"{path}\" --hook"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_events_to_the_registry_vocabulary() {
        assert_eq!(
            describe(Some("agent_needs_permission")).as_deref(),
            Some("permission prompt")
        );
        assert_eq!(
            describe(Some("agent_needs_input")).as_deref(),
            Some("input needed")
        );
        assert_eq!(
            describe(Some("something_else")).as_deref(),
            Some("something else")
        );
        assert_eq!(describe(None), None);
    }

    #[test]
    fn refuses_session_ids_that_could_escape_the_directory() {
        assert!(is_safe_id("b5f8f7b4-47f8-4ad6-9238-32dc9194ddf5"));
        assert!(!is_safe_id("../../etc/passwd"));
        assert!(!is_safe_id("a/b"));
        assert!(!is_safe_id(""));
    }

    #[test]
    fn adds_our_hooks_without_disturbing_existing_ones() {
        let mut root = json!({
            "model": "opus",
            "hooks": {
                "SessionStart": [
                    { "hooks": [ { "type": "command", "command": "some-other-tool" } ] }
                ]
            }
        });
        assert!(apply(&mut root, "\"/A/app\" --hook"));

        // Untouched.
        assert_eq!(root["model"], "opus");
        assert_eq!(
            root["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "some-other-tool"
        );
        // Added.
        for event in HOOK_EVENTS {
            let list = root["hooks"][event].as_array().unwrap();
            assert!(list.iter().any(|g| mentions_us(g)), "{event} missing");
        }
    }

    #[test]
    fn is_idempotent() {
        let mut root = json!({});
        assert!(apply(&mut root, "\"/A/app\" --hook"));
        // Running again must not add a second copy, or every launch would
        // grow the settings file.
        assert!(!apply(&mut root, "\"/A/app\" --hook"));
        assert_eq!(root["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn updates_the_path_when_the_app_moves() {
        let mut root = json!({});
        apply(&mut root, "\"/old/app\" --hook");
        assert!(apply(&mut root, "\"/new/app\" --hook"));

        let list = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(list.len(), 1, "should update, not duplicate");
        assert_eq!(list[0]["hooks"][0]["command"], "\"/new/app\" --hook");
    }

    #[test]
    fn reads_back_what_a_hook_wrote() {
        let dir = std::env::temp_dir().join("claude-code-bar-hooks-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("abc.json"),
            r#"{"status":"waiting","reason":"permission prompt","updated_at":123}"#,
        )
        .unwrap();

        let states = HookStates::read_from(&dir);
        let state = states.get("abc").unwrap();
        assert_eq!(state.status, "waiting");
        assert_eq!(state.reason.as_deref(), Some("permission prompt"));
        assert!(states.get("nope").is_none());
    }
}
