//! Works out which application a session is running inside, and — where the
//! application can be scripted precisely — brings that exact tab to the front.
//!
//! The registry's `entrypoint` says how Claude Code was launched, not where it
//! ended up. A session started with `claude` in VS Code's built-in terminal
//! records `cli`, the same as one in Terminal.app. Walking up the process tree
//! tells us the difference, which matters because only some hosts can be
//! focused down to the individual tab.

use std::collections::HashMap;
use std::process::Command;

/// How far up the process tree to look for a recognisable application before
/// giving up. Shells, login, and helper processes all sit in between.
const MAX_ANCESTRY_DEPTH: usize = 12;

/// A terminal we can drive with AppleScript down to the individual tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptableTerminal {
    AppleTerminal,
    ITerm2,
}

impl ScriptableTerminal {
    /// AppleScript reports a session's terminal device with a `/dev/` prefix,
    /// which `ps` leaves off.
    fn script(self, tty: &str) -> String {
        let device = format!("/dev/{tty}");
        match self {
            ScriptableTerminal::AppleTerminal => format!(
                r#"if application "Terminal" is running then
                     tell application "Terminal"
                       repeat with w in windows
                         repeat with t in tabs of w
                           if tty of t is "{device}" then
                             set selected of t to true
                             set index of w to 1
                             activate
                             return
                           end if
                         end repeat
                       end repeat
                     end tell
                   end if"#
            ),
            ScriptableTerminal::ITerm2 => format!(
                r#"if application "iTerm2" is running then
                     tell application "iTerm2"
                       repeat with w in windows
                         repeat with t in tabs of w
                           repeat with s in sessions of t
                             if tty of s is "{device}" then
                               select w
                               select t
                               select s
                               activate
                               return
                             end if
                           end repeat
                         end repeat
                       end repeat
                     end tell
                   end if"#
            ),
        }
    }
}

/// Whether the host is a terminal in its own right or an editor that happens
/// to contain one. It changes how the row should read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    Terminal,
    Editor,
}

/// The application a session is running inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub label: &'static str,
    /// What `open -a` calls this application, which is not always what we
    /// show — "Terminal.app" reads better than "Terminal" in a row, and
    /// "VS Code" is shorter than "Visual Studio Code".
    pub app_name: &'static str,
    pub kind: HostKind,
    /// Set only when we can select the exact tab.
    pub scriptable: Option<ScriptableTerminal>,
    /// The session's terminal device, needed to pick it out of the host.
    pub tty: Option<String>,
}

impl Host {
    /// Whether this session can be reached by clicking, which is the same
    /// question as whether we can select its exact tab.
    ///
    /// Only Terminal.app and iTerm2 qualify, because only they expose their
    /// tabs to scripting. Editors were tried and dropped: the nearest thing
    /// available is asking the system to open the session's folder, which
    /// means "open this document" and creates a new window whenever the
    /// editor does not already have that folder open the way it expects. A
    /// click that clutters your screen instead of taking you to your work is
    /// worse than a row that plainly does nothing, so those rows are not
    /// clickable.
    pub fn is_precise(&self) -> bool {
        self.scriptable.is_some() && self.tty.is_some()
    }
}

impl Host {
    /// Recognise an application from its process name. Names come from the
    /// executable, so the editors show up as their helper processes.
    #[allow(clippy::type_complexity)]
    fn from_process_name(
        name: &str,
    ) -> Option<(
        &'static str,
        &'static str,
        HostKind,
        Option<ScriptableTerminal>,
    )> {
        let host = match name {
            "Terminal" => (
                "Terminal.app",
                "Terminal",
                HostKind::Terminal,
                Some(ScriptableTerminal::AppleTerminal),
            ),
            "iTerm2" | "iTerm" => (
                "iTerm2",
                "iTerm",
                HostKind::Terminal,
                Some(ScriptableTerminal::ITerm2),
            ),
            "ghostty" | "Ghostty" => ("Ghostty", "Ghostty", HostKind::Terminal, None),
            "wezterm-gui" | "WezTerm" => ("WezTerm", "WezTerm", HostKind::Terminal, None),
            "alacritty" | "Alacritty" => ("Alacritty", "Alacritty", HostKind::Terminal, None),
            "kitty" => ("kitty", "kitty", HostKind::Terminal, None),
            "Warp" | "warp" | "stable" => ("Warp", "Warp", HostKind::Terminal, None),
            "Hyper" => ("Hyper", "Hyper", HostKind::Terminal, None),
            "Code" | "Code Helper" | "Code Helper (Plugin)" | "Code Helper (Renderer)" => {
                ("VS Code", "Visual Studio Code", HostKind::Editor, None)
            }
            "Cursor" | "Cursor Helper" | "Cursor Helper (Plugin)" => {
                ("Cursor", "Cursor", HostKind::Editor, None)
            }
            "Windsurf" => ("Windsurf", "Windsurf", HostKind::Editor, None),
            "pycharm" | "PyCharm" => ("PyCharm", "PyCharm", HostKind::Editor, None),
            "idea" | "IntelliJ IDEA" => ("IntelliJ IDEA", "IntelliJ IDEA", HostKind::Editor, None),
            "webstorm" | "WebStorm" => ("WebStorm", "WebStorm", HostKind::Editor, None),
            "goland" | "GoLand" => ("GoLand", "GoLand", HostKind::Editor, None),
            "Claude" => ("Claude Desktop", "Claude", HostKind::Editor, None),
            _ => return None,
        };
        Some(host)
    }

    /// Bring this session's tab to the front.
    ///
    /// Only the scriptable terminals can be reached at all. Editors are left
    /// alone entirely: see `is_precise` for why.
    pub fn focus(&self) {
        if self.is_precise() {
            self.focus_exact_tab();
        }
    }

    fn focus_exact_tab(&self) {
        let (Some(terminal), Some(tty)) = (self.scriptable, self.tty.as_deref()) else {
            return;
        };
        // The device name is interpolated into a script, so allow only the
        // characters a real one contains.
        if tty.is_empty()
            || !tty
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '/' || c == '-')
        {
            return;
        }

        run_script(terminal.script(tty), self.app_name);
    }
}

/// AppleScript can take a moment and must never block the panel, so it runs
/// off the main thread.
fn run_script(script: String, app: &'static str) {
    std::thread::spawn(
        move || match Command::new("osascript").arg("-e").arg(&script).output() {
            Ok(output) if !output.status.success() => eprintln!(
                "claude-overview: could not reach {app}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            Err(error) => eprintln!("claude-overview: could not run osascript: {error}"),
            _ => {}
        },
    );
}

/// One row of the process table.
#[derive(Debug, Clone)]
struct ProcessRow {
    parent: u32,
    tty: Option<String>,
    name: String,
}

/// Finds the application behind a process, and the terminal device it uses.
///
/// This reads the process table with `ps` rather than going through `sysinfo`,
/// which cannot see the parent of a setuid process. `login` sits between your
/// shell and Terminal.app and is exactly that, so `sysinfo` stops one step
/// short of the only host we can actually script.
pub struct HostResolver {
    table: HashMap<u32, ProcessRow>,
    /// Ancestry never changes for a live process, so a resolved host is kept
    /// rather than re-walked every refresh.
    resolved: HashMap<u32, Option<Host>>,
}

impl HostResolver {
    pub fn new() -> Self {
        Self {
            table: HashMap::new(),
            resolved: HashMap::new(),
        }
    }

    /// Walk up from the session process until something recognisable appears.
    pub fn resolve(&mut self, pid: u32) -> Option<Host> {
        if let Some(known) = self.resolved.get(&pid) {
            return known.clone();
        }
        // A process we have not seen means the table is stale.
        self.load_table();

        let host = self.walk(pid);
        self.resolved.insert(pid, host.clone());
        host
    }

    fn walk(&self, pid: u32) -> Option<Host> {
        let tty = self.table.get(&pid).and_then(|row| row.tty.clone());

        let mut current = pid;
        for _ in 0..MAX_ANCESTRY_DEPTH {
            let row = self.table.get(&current)?;
            if let Some((label, app_name, kind, scriptable)) = Host::from_process_name(&row.name) {
                return Some(Host {
                    label,
                    app_name,
                    kind,
                    scriptable,
                    // Only the scriptable hosts need to pick a tab out of a
                    // window, so only they need the device.
                    tty: scriptable.and(tty),
                });
            }
            if row.parent == current || row.parent <= 1 {
                break; // Reached the top of the tree.
            }
            current = row.parent;
        }
        None
    }

    /// One `ps` call covers every process, which is far cheaper than asking
    /// per session and gives parent links `sysinfo` will not.
    fn load_table(&mut self) {
        let Ok(output) = Command::new("ps")
            .args(["-Ao", "pid=,ppid=,tty=,comm="])
            .output()
        else {
            return;
        };

        self.table.clear();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some((pid, row)) = parse_ps_line(line) {
                self.table.insert(pid, row);
            }
        }
    }
}

/// Parse one `pid ppid tty comm` row.
///
/// `ps` right-aligns its columns, so fields are separated by runs of spaces
/// rather than single ones, and the command comes last and may itself contain
/// spaces — `Code Helper (Plugin)` being the case that matters here.
fn parse_ps_line(line: &str) -> Option<(u32, ProcessRow)> {
    let mut parts = line.split_whitespace();
    let pid = parts.next()?.parse::<u32>().ok()?;
    let parent = parts.next()?.parse::<u32>().ok()?;
    let tty = parts.next()?;
    let command = parts.collect::<Vec<_>>().join(" ");
    if command.is_empty() {
        return None;
    }

    Some((
        pid,
        ProcessRow {
            parent,
            // `??` is what `ps` prints for a process with no terminal.
            tty: (tty != "??").then(|| tty.to_string()),
            name: process_name(&command),
        },
    ))
}

/// Reduce a command path to the name we match on. Login shells are prefixed
/// with a dash, and applications are given as a full path to the binary.
fn process_name(command: &str) -> String {
    let command = command.trim();
    let base = command.rsplit('/').next().unwrap_or(command);
    base.strip_prefix('-').unwrap_or(base).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_terminals_editors_and_nothing_else() {
        let (label, app, kind, scriptable) = Host::from_process_name("Terminal").unwrap();
        assert_eq!(label, "Terminal.app");
        assert_eq!(app, "Terminal", "what `open -a` expects");
        assert_eq!(kind, HostKind::Terminal);
        assert_eq!(scriptable, Some(ScriptableTerminal::AppleTerminal));

        let (label, app, kind, scriptable) =
            Host::from_process_name("Code Helper (Plugin)").unwrap();
        assert_eq!(label, "VS Code");
        assert_eq!(app, "Visual Studio Code");
        assert_eq!(kind, HostKind::Editor);
        assert_eq!(scriptable, None);

        // A terminal we cannot script is still worth naming.
        let (label, _, kind, scriptable) = Host::from_process_name("ghostty").unwrap();
        assert_eq!(label, "Ghostty");
        assert_eq!(kind, HostKind::Terminal);
        assert_eq!(scriptable, None);

        assert!(Host::from_process_name("zsh").is_none());
        assert!(Host::from_process_name("login").is_none());
    }

    #[test]
    fn builds_a_script_that_targets_the_right_device() {
        let script = ScriptableTerminal::AppleTerminal.script("ttys009");
        assert!(script.contains(r#"tty of t is "/dev/ttys009""#));
        assert!(script.contains(r#"tell application "Terminal""#));

        let script = ScriptableTerminal::ITerm2.script("ttys012");
        assert!(script.contains(r#"tty of s is "/dev/ttys012""#));
        assert!(script.contains(r#"tell application "iTerm2""#));
    }

    #[test]
    fn reduces_commands_to_matchable_names() {
        assert_eq!(
            process_name("/Applications/Terminal.app/Contents/MacOS/Terminal"),
            "Terminal"
        );
        // Login shells arrive with a leading dash.
        assert_eq!(process_name("-zsh"), "zsh");
        assert_eq!(process_name("claude"), "claude");
        // Helper processes keep their spaces, which is how we recognise them.
        assert_eq!(
            process_name("/x/Code Helper (Plugin)"),
            "Code Helper (Plugin)"
        );
    }

    #[test]
    fn parses_the_padded_columns_ps_actually_prints() {
        // `ps` right-aligns, so these are the real separators, not single
        // spaces. Getting this wrong silently drops most of the table.
        let (pid, row) = parse_ps_line("18642 17010 ttys009  claude").unwrap();
        assert_eq!(pid, 18642);
        assert_eq!(row.parent, 17010);
        assert_eq!(row.tty.as_deref(), Some("ttys009"));
        assert_eq!(row.name, "claude");

        // Leading padding on narrow PIDs.
        let (pid, row) =
            parse_ps_line("  600     1 ??       /A/Terminal.app/C/MacOS/Terminal").unwrap();
        assert_eq!(pid, 600);
        assert_eq!(row.parent, 1);
        assert_eq!(row.tty, None, "?? means no terminal");
        assert_eq!(row.name, "Terminal");

        // A command containing spaces must survive intact.
        let (_, row) = parse_ps_line("  42     1 ??       /x/Code Helper (Plugin)").unwrap();
        assert_eq!(row.name, "Code Helper (Plugin)");

        // Header or junk lines are skipped rather than poisoning the table.
        assert!(parse_ps_line("").is_none());
        assert!(parse_ps_line("PID PPID TTY COMM").is_none());
        assert!(parse_ps_line("123 456 ttys001").is_none());
    }

    #[test]
    fn walks_past_a_setuid_parent_to_reach_the_terminal() {
        // The real shape of a Terminal.app session: claude < zsh < login <
        // Terminal, where login is the step sysinfo cannot see past.
        let mut resolver = HostResolver::new();
        resolver.table.insert(
            900,
            ProcessRow {
                parent: 800,
                tty: Some("ttys009".into()),
                name: "claude".into(),
            },
        );
        resolver.table.insert(
            800,
            ProcessRow {
                parent: 700,
                tty: Some("ttys009".into()),
                name: "zsh".into(),
            },
        );
        resolver.table.insert(
            700,
            ProcessRow {
                parent: 600,
                tty: Some("ttys009".into()),
                name: "login".into(),
            },
        );
        resolver.table.insert(
            600,
            ProcessRow {
                parent: 1,
                tty: None,
                name: "Terminal".into(),
            },
        );

        let host = resolver.walk(900).expect("should find Terminal.app");
        assert_eq!(host.label, "Terminal.app");
        assert_eq!(host.scriptable, Some(ScriptableTerminal::AppleTerminal));
        // The device comes from the session, not from the terminal process.
        assert_eq!(host.tty.as_deref(), Some("ttys009"));
    }

    #[test]
    fn gives_up_rather_than_looping_on_a_broken_tree() {
        let mut resolver = HostResolver::new();
        resolver.table.insert(
            10,
            ProcessRow {
                parent: 10,
                tty: None,
                name: "weird".into(),
            },
        );
        assert!(resolver.walk(10).is_none());
    }

    #[test]
    fn refuses_to_run_a_script_for_a_suspicious_device_name() {
        // Nothing to assert beyond it returning quietly, but this pins the
        // guard in place: the device name is interpolated into a script, so a
        // name containing quotes must never reach osascript.
        let host = Host {
            label: "Terminal.app",
            app_name: "Terminal",
            kind: HostKind::Terminal,
            scriptable: Some(ScriptableTerminal::AppleTerminal),
            tty: Some(r#"ttys009" & (do shell script "echo pwned") & ""#.to_string()),
        };
        host.focus_exact_tab();
    }
}
