# How Claude Code Bar works

This is the internal guide. If you only want to install and use the app, the [README](../README.md) is enough. Read this if you are changing the code, or if the app is showing you something that looks wrong and you want to know why.

## Contents

1. [Where the data comes from](#1-where-the-data-comes-from)
2. [Where session titles come from](#2-where-session-titles-come-from)
3. [Telling live sessions from dead ones](#3-telling-live-sessions-from-dead-ones)
4. [Working out which app a session is in](#4-working-out-which-app-a-session-is-in)
5. [Jumping to a session](#5-jumping-to-a-session)
6. [The panel, and why it is a window](#6-the-panel-and-why-it-is-a-window)
7. [Multiple displays and Spaces](#7-multiple-displays-and-spaces)
8. [Code layout](#8-code-layout)
9. [Checking what it sees](#9-checking-what-it-sees)
10. [Running the tests](#10-running-the-tests)
11. [Things worth knowing before you change it](#11-things-worth-knowing-before-you-change-it)

## 1. Where the data comes from

Claude Code already keeps a registry of its own sessions on disk. Nothing had to be invented for this app — it just reads what is already there.

Every session writes one JSON file into `~/.claude/sessions/`, named after its process ID, and rewrites it whenever its state changes:

```json
{
  "pid": 18642,
  "sessionId": "b5f8f7b4-47f8-4ad6-9238-32dc9194ddf5",
  "cwd": "/Users/you/some-project",
  "name": "some-project-c2",
  "kind": "interactive",
  "entrypoint": "cli",
  "status": "busy",
  "waitingFor": null,
  "startedAt": 1786635893224
}
```

The app polls that directory every two seconds and rebuilds the panel from it.

The fields that matter:

| Field | Used for |
|---|---|
| `status` | Which group the session goes in. One of `waiting`, `busy`, `idle`, `shell` |
| `waitingFor` | Why it is blocked — "permission prompt", "input needed", "sandbox request", "dialog open" |
| `name` | The project name shown on the row |
| `cwd` | Opening the right project when you click an editor row |
| `entrypoint` | Where it was launched from, used only when the process tree tells us nothing |
| `kind` | `interactive` and `bg` are counted; daemons and their workers are not |
| `sessionId` | Finding the transcript, which is where the title lives |
| `startedAt` | Telling this session apart from a recycled process ID |

Only these are read. Unknown fields are ignored, so Claude Code adding new ones will not break parsing.

**A session that reports no status at all** is almost always the VS Code extension. It registers itself but never publishes what it is doing. That is a limit of what Claude Code writes down, not something this app can work around, so those sessions get their own group.

## 2. Where session titles come from

The registry only has a name derived from the folder, like `my-project-c2`. That tells you where a session is, not what it is doing.

Claude Code also writes a generated one-line description into the session transcript at `~/.claude/projects/<project>/<sessionId>.jsonl`, as repeated lines:

```json
{"type":"ai-title","aiTitle":"Build Mac menu bar app for Claude sessions"}
```

The last one wins. Transcripts run to megabytes, so the app never reads one whole — it reads the last 128 KB and scans backwards, and only re-reads when the file's size or timestamp has changed since the last look. Everything else comes from cache.

The transcript is found by searching the project directories for `<sessionId>.jsonl`, rather than by reconstructing the folder name from the session's path. That way, however Claude Code chooses to mangle a path into a directory name, this keeps working.

## 3. Telling live sessions from dead ones

This is the fiddliest part of the app, and the easiest thing to get subtly wrong.

When a session ends, **its file stays behind**. Counting files would badly overcount — you would see sessions that ended days ago.

The obvious fix is to check whether the process ID is still alive. That is not enough either: **macOS recycles process IDs**, so a dead session's ID may now belong to something else entirely.

So a session counts only if **both** are true:

1. Its process ID belongs to a running process named `claude`.
2. That process started when the file says the session started, within a minute's tolerance.

`startedAt` lands within about three seconds of the real process start time — comfortably inside a minute, and nowhere near the gap you would see from a recycled ID.

## 4. Working out which app a session is in

A row says `Terminal.app` or `VS Code terminal` or `PyCharm terminal`. Two sources feed that, and the app prefers the more honest one.

The registry's `entrypoint` says how Claude Code was *launched*, not where it ended up. **A session started by typing `claude` in VS Code's built-in terminal records `cli` — exactly the same as one in Terminal.app.** Only the process tree can tell them apart, so that is what the label follows. `entrypoint` is the fallback for when no local process explains the session, which is how remote sources like Claude Desktop, Web, Slack and GitHub Actions get labelled.

### The setuid trap

Walking up the process tree sounds simple. Here is the chain for a Terminal.app session:

```
claude  →  zsh  →  login  →  Terminal
```

`login` is setuid root. **Most process-listing libraries cannot read its parent**, so they stop one step short — and Terminal.app is the only host we can actually script precisely. This cost real debugging time: everything worked for VS Code and PyCharm, and silently failed for exactly the case that mattered.

So the process table is built from `ps` instead, which can see past it:

```bash
ps -Ao pid=,ppid=,tty=,comm=
```

One call gives parent links, process names, and the terminal devices needed for jumping to a tab. It is called once and cached, since a live process never changes its ancestry.

**Watch the parsing.** `ps` right-aligns its columns, so fields are separated by runs of spaces, and the command comes last and may contain spaces of its own — `Code Helper (Plugin)` is the case that matters. Splitting on the first three whitespace *characters* silently drops most of the table. There is a test pinning this with real padded output.

## 5. Jumping to a session

One strategy, and one deliberate refusal.

**Exact — Terminal.app and iTerm2.** Both expose each tab's TTY device to AppleScript. Since every Claude session owns a distinct TTY, we can match it and select that precise tab:

```applescript
tell application "Terminal"
  activate
  repeat with w in windows
    repeat with t in tabs of w
      if tty of t is "/dev/ttys009" then
        set selected of t to true
        set index of w to 1
        return
      end if
    end repeat
  end repeat
end tell
```

The device name is interpolated into a script, so it is validated against a strict character set first. A test covers that.

Two details in that script matter. It is wrapped in `if application "Terminal" is running`, because referring to a stopped application's windows would **launch it** — and launching Terminal creates a window. And `activate` comes *after* a tab matches, so a session whose tab has gone does not steal focus for nothing.

**By folder — editors, but only sometimes.** VS Code, Cursor and the JetBrains editors expose no scriptable route to an individual Claude panel. The nearest thing is `open -a "<app>" <folder>`, which brings forward the window showing that folder.

That comes with a trap, and it was a real bug. **`open` means "open this document".** If the folder is not already open in the editor, the editor obeys literally and creates a **new window** — so a click meant to take you to your work instead clutters the screen with an empty one. It only misbehaves for sessions whose folder happens not to be open, which is why it looked intermittent.

So the app first works out what is genuinely open. Claude Code's editor extensions advertise themselves in `~/.claude/ide/<pid>.lock`, and each lock records the workspace its editor has open:

```json
{"pid": 55635, "workspaceFolders": ["/Users/you/some-project"], "ideName": "Visual Studio Code"}
```

Those locks outlive their editors exactly as session files do, so they are filtered against the process table first. A folder is passed to `open` **only** if a running editor is holding it. See `workspaces.rs`.

**There is no fallback beyond that**, deliberately. Merely bringing an application forward neither reaches the session nor justifies a click that implies it will, so those rows are not clickable at all. `Host::can_focus` is the single place that decides, and the UI marks the outcome with `›`, `↗`, or no arrow.

**If several sessions share a folder**, every one of those rows brings up the same window. Nothing distinguishes them from outside the editor.

## 6. The panel, and why it is a window

macOS menus can only draw plain rows of text. No colour, no type hierarchy, no grouping. So the panel is a borderless, translucent window with real macOS vibrancy, and it behaves like a menu: it appears under the icon and closes when it loses focus.

**It opens itself** when a session crosses into the waiting state. `BlockedWatcher` tracks each session's previous state and reports only genuine crossings — a session that is already stuck stays quiet, and nothing fires on the first reading after launch.

**It does not take focus when it opens itself.** It appears while you are typing, and taking focus would send your keystrokes to it. There is a consequence worth knowing: a window with no focus receives no blur event, so **clicking away in another app will not dismiss an auto-opened panel**. You close it with the menu bar icon, or by clicking a row. Real click-anywhere dismissal would need a system-wide event tap and an Accessibility permission, which is a poor trade for this.

A panel you opened yourself *does* take focus, and does dismiss on click-away as expected.

There is also a short grace period after showing, during which a focus loss is ignored. An app with no Dock icon does not always become active the instant it asks to, and without the grace period the panel could hide itself in the same breath as being shown — which looks exactly like a click that did nothing.

## 7. Multiple displays and Spaces

macOS draws a menu bar on every display but renders status items on whichever display currently holds the menu bar, so the counts follow you rather than duplicating.

**The panel must open on the display holding the icon**, and this is easy to get wrong. Displays can have different scale factors. The panel may be parked on one display while the icon lives on another. So:

- The target display is found by testing which monitor's rectangle contains the icon.
- All the arithmetic is done against *that* display's scale factor.
- The final move is made in **logical points**, because asking for physical pixels gets re-scaled through whichever display the window currently sits on.

Getting this wrong puts the panel silently off-screen, which is indistinguishable from a dead click.

**Spaces** need an explicit opt-in. A window belongs to the desktop it was created on, so a panel that opened itself while you were on another Space would either fail to appear or drag you across. The panel sets `visible_on_all_workspaces`, so it follows you.

## 8. Code layout

```
claude-code-bar/
├── src-tauri/
│   ├── src/
│   │   ├── main.rs        # startup, the panel's commands, --counts
│   │   ├── sessions.rs    # reads the registry, decides what is live, groups it
│   │   ├── titles.rs      # digs the session title out of the transcript
│   │   ├── focus.rs       # which app a session runs in, and jumping to it
│   │   ├── blocked.rs     # spots sessions crossing into the waiting state
│   │   ├── panel.rs       # the dropdown window, and where it opens
│   │   ├── tray.rs        # the menu bar item, the poll loop, the dot animation
│   │   ├── state.rs       # shared registry and latest snapshot
│   │   └── debug.rs       # opt-in tracing
│   ├── icons/             # app icon, the Claude mark, three animation frames
│   ├── capabilities/      # what the panel window is allowed to do
│   ├── Info.plist         # LSUIElement: keeps it out of the Dock
│   └── tauri.conf.json
└── ui/
    └── index.html         # the panel: markup, styling, and rendering
```

Most of the interesting logic is in `sessions.rs` and `focus.rs`.

**The panel is plain HTML with no build step.** Rust polls, the panel asks for the latest snapshot once a second while visible, and rebuilds its list. Session titles come from conversation content, so the UI builds rows with `createElement` and `textContent` — never by interpolating into HTML.

**The menu bar animation** swaps between three pre-rendered PNGs on a 380 ms timer, because macOS has no animated status item image. That timer only does work while something is blocked.

**The mark is indigo, a shade lighter than the app icon's tile**, because a saturated indigo goes muddy against a dark menu bar. **The item carries no text**, which is a width decision rather than a style one. macOS silently drops status items when the bar runs out of room, and it drops the widest first. With the counts rendered as text the item measured 159pt — more than most applications take in total. As the mark alone it measures 20pt. The counts live in the tooltip and the panel instead. `tray.rect()` reports the item's real size, which is how to check this if you change the artwork.

## 9. Checking what it sees

When a number looks wrong, ask the app instead of guessing:

```bash
./target/release/claude-overview --counts
```

```
Claude Code — 0 waiting · 2 running · 3 done

🔄 Running — 2
      claude-code-bar-c2 · Terminal.app
            "Build Mac menu bar app for Claude sessions"
      payments-api-4f · VS Code terminal
            "Refactor the checkout flow"
```

It prints exactly what the panel would show and exits without touching the menu bar, so it works from a script too.

| Variable | Effect |
|---|---|
| `CLAUDE_OVERVIEW_DEBUG=1` | Trace the menu bar and panel to stderr. The quickest way to tell a click that never arrived from a panel that opened somewhere unhelpful |
| `CLAUDE_OVERVIEW_SESSION_DIR` | Read sessions from a fixture directory instead of your real ones |
| `CLAUDE_OVERVIEW_PROJECTS_DIR` | Same, for the transcripts titles come from |
| `CLAUDE_OVERVIEW_SHOW_PANEL=1` | Open the panel at launch, for working on how it looks without clicking |

## 10. Running the tests

```bash
cd src-tauri
cargo test
cargo clippy
cargo fmt
```

The suite covers status mapping, keeping unreported sessions out of the "done" count, excluding daemons, naming, host labelling, title extraction including cache invalidation and oversized transcripts, `ps` output parsing, walking past a setuid parent, AppleScript device validation, and the blocked-transition rules.

Three things can only be checked by hand, because they depend on the live system:

1. **The liveness check.** Copy some of your own session files into a scratch directory, edit their `status` values, add one with a process ID that cannot exist, and run `--counts` with `CLAUDE_OVERVIEW_SESSION_DIR`. The dead one must not appear.
2. **Jumping to a tab**, which needs a real terminal and the Automation permission.
3. **Where the panel lands** on a multi-display or multi-Space setup.

## 11. Things worth knowing before you change it

- **This reads internal files Claude Code does not document.** They have been stable in practice, and unknown fields are ignored, but a future release could rename or move them. If the app suddenly shows nothing, look there first.
- **macOS only.** The registry exists everywhere, but the menu bar item, the vibrancy and the tab scripting are all Mac-specific.
- **Only your own sessions**, since it reads your home directory.
- **Launch it as an app, not as a bare binary**, if you add anything that needs a bundle identity. Running `target/release/claude-overview` directly works for everything the app currently does, but macOS treats a loose executable as having no identity, which silently disables some system integrations.
- **Cost is small but not zero.** Every two seconds: one directory listing, one `ps` call at most, and a few file timestamp checks. The animation adds a frame swap every 380 ms while something is blocked. Memory sits around 45 MB, which is the Tauri runtime rather than anything this app does.
