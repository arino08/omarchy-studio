# Handoff: Entering v0.9 Milestone

This document tracks the work completed during the recent session and outlines exactly where the next agent should pick up.

## 1. What was completed in the current session

### TUI Polish & Navigation
- **Mouse Support**: Implemented `EnableMouseCapture` and handled `on_mouse` events in `App`, allowing users to click on the left rail to instantly switch screens.
- **Keyboard Navigation**: Shifted the global keybar hint from `1-0` to `^j`/`^k` to reflect a more fluid module scrolling paradigm.
- **Rail State Tracking**: Implemented `ListState` for the left rail so that it scrolls correctly and tracks focus.
- **Aesthetics**: Removed the left border from the main panel so it visually merges with the rail's right border, and removed the numbers from the rail labels.

### Repository & Tooling Improvements
- **Installer (`install.sh`)**: Created a robust, one-shot install script that tries to fetch the latest pre-built binary and falls back to `cargo install --git` if no binaries are available.
- **GitHub Actions**: Added a `workflow_dispatch` trigger to `release.yml` so releases can be run manually.
- **Cleanup**: Removed the `.claude/` directory from git tracking to prevent cluttering the repo with agent-specific files.
- **README Updates**: Added a ToC, ASCII branding, and updated the Quick Start shortcut table to reflect the new `Ctrl+J/K` and mouse navigation.
- **Clippy Fixes**: Fixed `clippy::collapsible_if` warnings blocking the CI in `crates/omarchy-studio/src/tui/mod.rs` (`on_mouse`).

### Temporary fixes (resolved)
- **Updates (`omarchy-studio update`)**: this session briefly made `update::apply()` pipe `install.sh` to `sh` while the release workflow wasn't attaching binaries. That hack is **gone**. `release.yml` attaches `omarchy-studio-linux-x86_64` plus its `.sha256`, and `update::apply()` downloads the asset, refuses it unless it is an ELF whose SHA-256 matches the published one, then renames it over the running binary. Nothing here needs reverting.

## 2. Deferred / Postponed Work
- The user specifically requested to **pause** work on the `fzf`-style fuzzy finder (for themes/wallpapers) and transition animations for now. Do not prioritize these until explicitly asked.

## 3. Where to pick up (Next Steps)
We are now officially ready to tackle the **v0.9** roadmap milestone. 

1. **Review `ROADMAP.md`**: Look at the v0.9 section.
2. **DONE — Snapshot Timeline (0.9.4)**: shipped. The Snapshots screen (`crates/omarchy-studio/src/tui/screens/snapshots.rs`) lists the git snapshot history with a live colored diff pane and whole-tree restore; store gained `changed_files`/`diff`/`restore_files`; CLI gained `snapshot log`/`show <id>`/`restore <id> --files`. This was the last placeholder — every rail entry is now a real screen, so the `built()`/`arriving()`/`draw_placeholder` scaffolding was removed.
3. **Next Feature**: pick up the remaining v0.9 items in order — 0.9.1 community themes browser, 0.9.2 instant theme-from-wallpaper, 0.9.5 keymap cheatsheet, 0.9.6 rice bundle — or 0.7.2 TUI theme sync per the build order in `docs/handoff-v0.8.md` §7.
4. **Double-check CI**: Ensure the GitHub Actions builds are staying green across platforms as you build.
