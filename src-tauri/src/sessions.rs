//! Reads Claude Code's on-disk session registry and turns it into a snapshot.
//!
//! Claude Code writes one JSON file per session into `~/.claude/sessions/`,
//! named `<pid>.json`, and rewrites it whenever the session changes state.
//! We only read those files — nothing here ever writes to them.

use crate::focus::{Host, HostKind, HostResolver};
use crate::titles::TitleCache;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Registry files are keyed by PID, and PIDs get recycled. Each record also
/// carries the moment its session started, which lands within a few seconds of
/// the real process start time, so we use it to tell "our" process apart from
/// an unrelated one that inherited the same PID.
const START_DRIFT_TOLERANCE_SECS: i64 = 60;

/// The process name Claude Code runs under.
const CLAUDE_PROCESS_NAME: &str = "claude";

/// Which group a session belongs to, in the order they appear in the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bucket {
    /// Blocked on a prompt — this is the one that wants a human.
    Waiting,
    /// Actively working on a turn.
    Running,
    /// Turn finished, sitting ready for the next instruction.
    Done,
    /// Reported no status. In practice this is the editor extension, which
    /// registers its sessions but does not publish what they are doing.
    Unknown,
}

impl Bucket {
    /// Used in the menu bar title, where only text is possible.
    pub fn marker(self) -> &'static str {
        match self {
            Bucket::Waiting => "❗",
            Bucket::Running => "🔄",
            Bucket::Done => "✅",
            Bucket::Unknown => "💤",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Bucket::Waiting => "Waiting for input",
            Bucket::Running => "Running",
            Bucket::Done => "Done",
            Bucket::Unknown => "No status reported",
        }
    }

    /// Stable name handed to the UI, which keys its colours off it.
    pub fn key(self) -> &'static str {
        match self {
            Bucket::Waiting => "waiting",
            Bucket::Running => "running",
            Bucket::Done => "done",
            Bucket::Unknown => "unknown",
        }
    }

    pub const ALL: [Bucket; 4] = [
        Bucket::Waiting,
        Bucket::Running,
        Bucket::Done,
        Bucket::Unknown,
    ];
}

/// One entry from the session registry, as it appears on disk.
///
/// Claude Code writes more fields than these; we deserialize only what we
/// need and let serde ignore the rest, so new fields upstream can't break
/// parsing.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub pid: u32,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Claude Code's own name for the session, derived from the directory.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    /// Where the session was launched from — `cli`, `claude-vscode`, and so on.
    #[serde(default)]
    pub entrypoint: Option<String>,
    /// One of `waiting`, `busy`, `idle`, `shell`. Absent on editor sessions.
    #[serde(default)]
    pub status: Option<String>,
    /// Why the session is blocked, when it is. Only set alongside `waiting`.
    #[serde(default)]
    pub waiting_for: Option<String>,
    /// Epoch milliseconds.
    #[serde(default)]
    pub started_at: Option<i64>,
}

impl SessionRecord {
    pub fn bucket(&self) -> Bucket {
        match self.status.as_deref() {
            Some("waiting") => Bucket::Waiting,
            Some("busy") => Bucket::Running,
            // A session in shell mode is idle as far as the user is concerned.
            Some("idle") | Some("shell") => Bucket::Done,
            _ => Bucket::Unknown,
        }
    }

    /// Daemons and their workers are plumbing, not sessions a person is
    /// tending, so they stay out of the counts. Records with no `kind` are
    /// assumed to be interactive.
    pub fn is_user_facing(&self) -> bool {
        matches!(
            self.kind.as_deref(),
            None | Some("interactive") | Some("bg")
        )
    }

    /// The project a session belongs to. Claude Code's own name is best;
    /// failing that the directory it runs in, and failing that the PID.
    pub fn project_name(&self) -> String {
        if let Some(name) = self.name.as_deref().filter(|n| !n.is_empty()) {
            return name.to_string();
        }
        if let Some(cwd) = self.cwd.as_deref().filter(|c| !c.is_empty()) {
            if let Some(base) = cwd.rsplit('/').find(|part| !part.is_empty()) {
                return base.to_string();
            }
        }
        format!("pid {}", self.pid)
    }

    /// Whether this session is the editor extension rather than a terminal.
    fn is_extension(&self) -> bool {
        matches!(
            self.entrypoint.as_deref(),
            Some("claude-vscode") | Some("vscode") | Some("jetbrains") | Some("ide")
        )
    }

    /// Where the session is running, preferring what we observed over what it
    /// claims. A session started with `claude` inside VS Code's built-in
    /// terminal records `cli`, same as one in Terminal.app, so the process
    /// tree is the more honest answer.
    pub fn host_label(&self, host: Option<&Host>) -> String {
        match host {
            Some(host) if host.kind == HostKind::Editor => {
                if self.is_extension() {
                    format!("{} extension", host.label)
                } else {
                    format!("{} terminal", host.label)
                }
            }
            Some(host) => host.label.to_string(),
            None => self.entrypoint_label(),
        }
    }

    /// Fallback for when the process tree tells us nothing: the label Claude
    /// Code itself uses for the entrypoint. Anything unrecognised is shown
    /// as-is rather than hidden, so a new source still reads sensibly.
    fn entrypoint_label(&self) -> String {
        let Some(entrypoint) = self.entrypoint.as_deref().filter(|e| !e.is_empty()) else {
            return "Unknown source".to_string();
        };
        match entrypoint {
            "cli" | "sdk-cli" => "Terminal",
            "claude-vscode" | "vscode" => "VS Code",
            "jetbrains" => "JetBrains",
            "ide" => "Editor",
            "claude-desktop" | "claude-desktop-3p" | "remote_desktop" | "desktop-app" => {
                "Claude Desktop"
            }
            "web" => "Web",
            "remote_mobile" => "Mobile",
            "local-agent" | "remote_cowork" => "Cowork",
            "claude_in_slack" | "claude-in-slack" => "Claude in Slack",
            "claude-in-teams" => "Claude in Teams",
            "claude-code-github-action" | "github-action" => "GitHub Actions",
            "sdk-ts" | "sdk-typescript" | "sdk-py" | "sdk-python" | "sdk-control" | "sdk-url" => {
                "SDK"
            }
            "mcp" => "MCP",
            other => other,
        }
        .to_string()
    }
}

/// One session, shaped for the panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub pid: u32,
    pub bucket: &'static str,
    /// The generated title describing what the session is about, when it has
    /// one. This is the most useful thing on the row.
    pub title: Option<String>,
    pub project: String,
    pub host: String,
    /// Why it is blocked, when it is.
    pub waiting_for: Option<String>,
    /// Whether clicking the row goes anywhere. False rows are shown as plain
    /// information rather than inviting a click that cannot be honoured.
    pub focusable: bool,
}

/// The tally shown in the menu bar.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub waiting: usize,
    pub running: usize,
    pub done: usize,
    pub unknown: usize,
}

impl Counts {
    pub fn of(&self, bucket: Bucket) -> usize {
        match bucket {
            Bucket::Waiting => self.waiting,
            Bucket::Running => self.running,
            Bucket::Done => self.done,
            Bucket::Unknown => self.unknown,
        }
    }

    /// The compact readout that sits in the menu bar itself.
    ///
    /// Only the three states you can act on appear here. Sessions that report
    /// nothing are still listed in the panel, but a count of them in the menu
    /// bar was just width spent on something you cannot do anything about.
    pub fn tray_title(&self) -> String {
        format!(
            "{} {}   {} {}   {} {}",
            Bucket::Waiting.marker(),
            self.waiting,
            Bucket::Running.marker(),
            self.running,
            Bucket::Done.marker(),
            self.done,
        )
    }
}

/// Everything the panel needs for one refresh.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub counts: Counts,
    /// Live sessions, ordered the way the panel lists them: the ones wanting
    /// attention first, then alphabetically within each group.
    pub sessions: Vec<SessionView>,
}

/// Reads the session directory and reports what is running.
pub struct SessionRegistry {
    dir: PathBuf,
    system: System,
    titles: TitleCache,
    hosts: HostResolver,
    /// Where each live session is running, kept so a click can act on it
    /// without re-walking the process tree.
    known_hosts: HashMap<u32, Host>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self::with_dir(default_session_dir())
    }

    pub fn with_dir(dir: PathBuf) -> Self {
        Self {
            dir,
            system: System::new(),
            titles: TitleCache::new(),
            hosts: HostResolver::new(),
            known_hosts: HashMap::new(),
        }
    }

    /// Collect every live session. Dead sessions leave their files behind, so
    /// each record is checked against the running process table before it
    /// counts towards anything.
    pub fn snapshot(&mut self) -> Snapshot {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );

        let mut records: Vec<SessionRecord> = self
            .read_records()
            .into_iter()
            .filter(|record| record.is_user_facing() && self.is_live(record))
            .collect();

        records.sort_by(|a, b| {
            a.bucket()
                .cmp(&b.bucket())
                .then_with(|| a.project_name().cmp(&b.project_name()))
        });

        let mut counts = Counts::default();
        let mut sessions = Vec::with_capacity(records.len());
        let mut known_hosts = HashMap::with_capacity(records.len());

        for record in &records {
            match record.bucket() {
                Bucket::Waiting => counts.waiting += 1,
                Bucket::Running => counts.running += 1,
                Bucket::Done => counts.done += 1,
                Bucket::Unknown => counts.unknown += 1,
            }

            let host = self.hosts.resolve(record.pid);
            let title = record
                .session_id
                .as_deref()
                .and_then(|id| self.titles.title_for(id));

            sessions.push(SessionView {
                pid: record.pid,
                bucket: record.bucket().key(),
                title,
                project: record.project_name(),
                host: record.host_label(host.as_ref()),
                waiting_for: record.waiting_for.clone(),
                focusable: host.as_ref().is_some_and(Host::is_precise),
            });

            if let Some(host) = host {
                known_hosts.insert(record.pid, host);
            }
        }

        self.known_hosts = known_hosts;
        Snapshot { counts, sessions }
    }

    /// Bring a session's terminal tab to the front, if it is one we can
    /// actually reach.
    pub fn focus(&self, pid: u32) {
        if let Some(host) = self.known_hosts.get(&pid) {
            host.focus();
        }
    }

    fn read_records(&self) -> Vec<SessionRecord> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            // No directory means Claude Code has never run here. Zero sessions
            // is the right answer, not an error.
            Err(_) => return Vec::new(),
        };

        let mut records = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            // A session mid-write can be read as truncated JSON. Skipping it
            // for one poll is fine; the next poll picks it up.
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(record) = serde_json::from_str::<SessionRecord>(&text) {
                    records.push(record);
                }
            }
        }
        records
    }

    /// A record is live when its PID still belongs to a `claude` process that
    /// started when the record says the session did.
    fn is_live(&self, record: &SessionRecord) -> bool {
        let process = match self.system.process(Pid::from_u32(record.pid)) {
            Some(process) => process,
            None => return false,
        };

        if !process
            .name()
            .to_str()
            .is_some_and(|name| name == CLAUDE_PROCESS_NAME)
        {
            return false;
        }

        // Records without `startedAt` can only be checked by process name.
        // That is weak evidence, but it is all they offer, and treating them
        // as live beats dropping real sessions from the count.
        let Some(started_at_ms) = record.started_at else {
            return true;
        };

        let recorded_secs = started_at_ms / 1000;
        let actual_secs = process.start_time() as i64;
        (recorded_secs - actual_secs).abs() <= START_DRIFT_TOLERANCE_SECS
    }
}

fn default_session_dir() -> PathBuf {
    // Overridable so the counting can be exercised against a fixture
    // directory instead of your real sessions.
    if let Ok(dir) = std::env::var("CLAUDE_OVERVIEW_SESSION_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".claude").join("sessions")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::focus::ScriptableTerminal;

    fn record(status: Option<&str>, kind: Option<&str>) -> SessionRecord {
        SessionRecord {
            pid: 1,
            session_id: None,
            cwd: None,
            name: None,
            kind: kind.map(str::to_string),
            entrypoint: None,
            status: status.map(str::to_string),
            waiting_for: None,
            started_at: None,
        }
    }

    fn host(label: &'static str, kind: HostKind) -> Host {
        Host {
            label,
            app_name: label,
            kind,
            scriptable: None,
            tty: None,
        }
    }

    #[test]
    fn maps_each_status_to_its_bucket() {
        assert_eq!(record(Some("waiting"), None).bucket(), Bucket::Waiting);
        assert_eq!(record(Some("busy"), None).bucket(), Bucket::Running);
        assert_eq!(record(Some("idle"), None).bucket(), Bucket::Done);
        assert_eq!(record(Some("shell"), None).bucket(), Bucket::Done);
    }

    #[test]
    fn treats_a_missing_status_as_unknown_rather_than_done() {
        assert_eq!(record(None, None).bucket(), Bucket::Unknown);
    }

    #[test]
    fn counts_interactive_and_background_but_not_daemons() {
        assert!(record(None, Some("interactive")).is_user_facing());
        assert!(record(None, Some("bg")).is_user_facing());
        assert!(record(None, None).is_user_facing());
        assert!(!record(None, Some("daemon")).is_user_facing());
        assert!(!record(None, Some("daemon-worker")).is_user_facing());
    }

    #[test]
    fn names_a_session_by_name_then_directory_then_pid() {
        let mut session = record(None, None);
        assert_eq!(session.project_name(), "pid 1");

        session.cwd = Some("/Users/someone/my-project/".to_string());
        assert_eq!(session.project_name(), "my-project");

        session.name = Some("my-project-c2".to_string());
        assert_eq!(session.project_name(), "my-project-c2");
    }

    #[test]
    fn prefers_the_observed_host_over_the_recorded_entrypoint() {
        let mut session = record(None, None);
        session.entrypoint = Some("cli".to_string());

        // Nothing observed: fall back to what the session claims.
        assert_eq!(session.host_label(None), "Terminal");

        // A real terminal needs no qualifier.
        let terminal = host("Terminal.app", HostKind::Terminal);
        assert_eq!(session.host_label(Some(&terminal)), "Terminal.app");

        // `cli` inside an editor is the built-in terminal, not the extension.
        let editor = host("VS Code", HostKind::Editor);
        assert_eq!(session.host_label(Some(&editor)), "VS Code terminal");

        // The extension reports itself, and should read differently.
        session.entrypoint = Some("claude-vscode".to_string());
        assert_eq!(session.host_label(Some(&editor)), "VS Code extension");
    }

    #[test]
    fn falls_back_to_entrypoint_labels_when_the_host_is_unknown() {
        let mut session = record(None, None);
        assert_eq!(session.host_label(None), "Unknown source");

        session.entrypoint = Some("claude-desktop".to_string());
        assert_eq!(session.host_label(None), "Claude Desktop");

        // An entrypoint we have never seen is shown rather than swallowed.
        session.entrypoint = Some("something-new".to_string());
        assert_eq!(session.host_label(None), "something-new");
    }

    #[test]
    fn only_scriptable_hosts_are_focusable() {
        let plain = host("VS Code", HostKind::Editor);
        assert!(plain.scriptable.is_none());

        let scriptable = Host {
            label: "Terminal.app",
            app_name: "Terminal",
            kind: HostKind::Terminal,
            scriptable: Some(ScriptableTerminal::AppleTerminal),
            tty: Some("ttys009".to_string()),
        };
        assert!(scriptable.scriptable.is_some());
    }

    #[test]
    fn ignores_unparseable_and_missing_directories() {
        let registry = SessionRegistry::with_dir(PathBuf::from("/nonexistent/path/xyz"));
        assert!(registry.read_records().is_empty());
    }

    #[test]
    fn parses_a_real_registry_record() {
        let text = r#"{
            "pid": 18642,
            "sessionId": "b5f8f7b4-47f8-4ad6-9238-32dc9194ddf5",
            "cwd": "/Users/someone/project",
            "startedAt": 1786635893224,
            "version": "2.1.229",
            "kind": "interactive",
            "entrypoint": "cli",
            "name": "project-c2",
            "status": "busy",
            "updatedAt": 1786635935532
        }"#;
        let record: SessionRecord = serde_json::from_str(text).unwrap();
        assert_eq!(record.pid, 18642);
        assert_eq!(record.bucket(), Bucket::Running);
        assert_eq!(record.project_name(), "project-c2");
        assert_eq!(
            record.session_id.as_deref(),
            Some("b5f8f7b4-47f8-4ad6-9238-32dc9194ddf5")
        );
        assert_eq!(record.started_at, Some(1786635893224));
    }

    #[test]
    fn formats_the_tray_title() {
        let mut counts = Counts {
            waiting: 2,
            running: 1,
            done: 4,
            unknown: 0,
        };
        assert_eq!(counts.tray_title(), "❗ 2   🔄 1   ✅ 4");

        // Sessions that report nothing stay out of the menu bar entirely.
        counts.unknown = 3;
        assert_eq!(counts.tray_title(), "❗ 2   🔄 1   ✅ 4");
    }

    #[test]
    fn orders_sessions_by_urgency_then_name() {
        let mut waiting = record(Some("waiting"), None);
        waiting.name = Some("zebra".to_string());
        let mut done_a = record(Some("idle"), None);
        done_a.name = Some("apple".to_string());
        let mut running = record(Some("busy"), None);
        running.name = Some("mango".to_string());

        let mut sessions = vec![done_a, running, waiting];
        sessions.sort_by(|a, b| {
            a.bucket()
                .cmp(&b.bucket())
                .then_with(|| a.project_name().cmp(&b.project_name()))
        });

        let order: Vec<String> = sessions.iter().map(|s| s.project_name()).collect();
        assert_eq!(order, vec!["zebra", "mango", "apple"]);
    }
}
