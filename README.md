# Claude Code Bar

A small macOS menu bar app that shows what all your Claude Code sessions are doing — and pops open by itself the moment one gets stuck waiting for you.

The menu bar shows the Claude mark, and three red dots beside it when a session is waiting on you. Hover for the counts; click for the detail.

```
    ✳            nothing needs you
    ✳ ●●●        something is waiting
```

## Contents

- [What is it?](#what-is-it)
- [How it looks](#how-it-looks)
- [How to install](#how-to-install)
- [Documentation](#documentation)
- [License](#license)

## What is it?

If you run Claude Code in more than one place, you lose track of it. One session is sitting on a permission prompt, another finished five minutes ago, a third is still working — and the only way to find out is to go hunting through terminal tabs and editor windows.

Claude Code Bar keeps that answer in your menu bar, next to the clock.

**The problem it actually solves** is the first number. A session waiting on a prompt is doing nothing at all, and every minute you don't notice is a minute wasted. So the app does not wait for you to look:

- While something is blocked, **three red dots** animate next to the Claude mark in the menu bar.
- **The panel opens by itself**, so you find out without checking.

It never takes keyboard focus when it opens on its own. It appears while you are typing, and stealing focus would send your next keystrokes to it instead of your editor.

The other two numbers are for deciding whether to switch: something running is best left alone, something done is worth picking up.

Everything it shows is read from files Claude Code already writes. It never writes to `~/.claude`, never talks to the network, and never touches your sessions.

## How it looks

The menu bar item is deliberately tiny — about 20pt, smaller than most status icons. It is the Claude mark on its own, with three animated red dots beside it while a session is blocked.

That is a deliberate trade. An earlier version put the counts in the bar as text and came to 159pt, which is wider than most applications take in total, and wide enough that macOS starts silently dropping the item when your menu bar fills up. The counts moved to the tooltip, which costs no width at all:

```
Claude Code — 2 waiting · 1 running · 4 done
```

Click it — or wait for it to open itself — and you get the full picture:

```
Claude Code                                    9 sessions
─────────────────────────────────────────────────────────
 ● 2 sessions need your input
─────────────────────────────────────────────────────────
● WAITING FOR INPUT                                     2
   Refactor the checkout flow                        ›
   payments-api-4f · Terminal.app
   permission prompt

   Fix flaky auth tests                              ↗
   web-frontend-c2 · VS Code extension
   input needed

● RUNNING                                               1
   Build Mac menu bar app for Claude sessions        ›
   claude-code-bar-c2 · Terminal.app

● DONE                                                  4
   Implement markdown blog with design mockups       ↗
   new-blog-51 · VS Code terminal
   …
─────────────────────────────────────────────────────────
Quit Claude Code Bar
```

Every row leads with the **session title** — Claude Code's own one-line description of what that session is about — then the project and the application it is running in.

| | Group | What it means |
|---|---|---|
| 🔴 | Waiting for input | Blocked on a prompt. **This one needs you.** |
| 🟠 | Running | Working on your turn right now. |
| 🟢 | Done | Finished, ready for the next instruction. |
| ⚪ | No status reported | Running, but not saying what it is doing. Almost always the VS Code extension, which registers its sessions without publishing a status. |

Sessions with no status stay in the panel but are kept out of the menu bar counts — a number you cannot act on is not worth the width.

**Sessions running in Terminal.app or iTerm2 are clickable** — a `›` marks them, and clicking selects that exact tab. Both terminals expose their tabs to scripting, so the app can match the session's terminal device and land on precisely the right one.

**Every other row is plain information.** Editors have no scriptable route to an individual Claude panel. The nearest thing available is asking the system to open the session's folder, which means "open this document" — and that reliably produced **new windows** instead of taking you to your work. A click that clutters your screen is worse than a row that plainly does nothing, so those rows do not invite one.

## How to install

You need [Rust](https://rustup.rs) and Xcode Command Line Tools. On a Mac that has never had Rust:

```bash
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then build the app:

```bash
git clone https://github.com/ashu1461/claude-code-bar.git
cd claude-code-bar/src-tauri

cargo install tauri-cli --version "^2"   # once per machine
cargo tauri build
```

The first build pulls in the whole Tauri dependency tree and takes a few minutes. Later builds take about a minute. The result is roughly 5 MB:

```
src-tauri/target/release/bundle/macos/Claude Overview.app
```

Drag it into `/Applications` and open it:

```bash
open "target/release/bundle/macos/Claude Overview.app"
```

macOS will warn you that the app is from an unidentified developer, because it is not code signed. Right-click it and choose **Open** the first time to get past that.

The app has no Dock icon and no window of its own. **Left click** the menu bar item to open the panel, **right click** for Quit.

**To start it at login:** System Settings → General → Login Items → **+** under "Open at Login" → pick the app.

**The first time you click a row** to jump to a Terminal.app or iTerm2 session, macOS asks permission for the app to control that terminal. Opening an editor needs no permission.

## Documentation

| Doc | What is in it |
|---|---|
| [docs/how-it-works.md](docs/how-it-works.md) | Where the data comes from, how live sessions are told from dead ones, how the host application is detected, the code layout, and how to run the tests |

Quick reference for working on it:

```bash
cd src-tauri
cargo test        # unit tests
cargo clippy      # linter
cargo fmt         # formatter

./target/release/claude-overview --counts   # print what the panel would show
```

Contributions welcome. Some obvious next steps:

- Show how long each session has been in its current state.
- Support jumping to sessions in more terminals as they gain scripting support.
- A preferences window for the poll interval and whether the panel opens itself.

## License

MIT — see [LICENSE](LICENSE).

Not affiliated with Anthropic. The menu bar mark is an original drawing, not an official Anthropic asset.
