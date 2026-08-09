# Spec 01 — Architecture

## 1. Workspace layout

```
crates/
├── studio-core/          # everything testable without a terminal
│   └── src/
│       ├── lib.rs
│       ├── omarchy/      # adapter: paths, commands, probing (spec 02)
│       ├── configfs/     # dialect parsers/writers + managed blocks (spec 03)
│       ├── engine/       # settings schema, apply pipeline, verification
│       ├── snapshot/     # git history store, undo, restore (spec 03 §6)
│       ├── deps/         # dependency & tool registry, guided install (spec 06)
│       ├── modules/      # per-module domain logic: themes, keybinds, looknfeel,
│       │                 #   waybar, notify, lockidle, wallpapers, extraction
│       └── error.rs      # StudioError taxonomy
└── omarchy-studio/       # the binary
    └── src/
        ├── main.rs       # arg dispatch: no args → TUI; subcommand → CLI (spec 08)
        ├── tui/          # ratatui app: screens, widgets, event loop (spec 07)
        └── cli/          # clap tree mapping 1:1 onto core operations
```

Rule: **`studio-core` never prints, never draws, never prompts.** It returns typed results; frontends render them. This is what keeps a future GUI (or a Walker-native flow) cheap.

## 2. The settings schema (D7)

Every editable field in Studio is an instance of:

```rust
pub struct Setting {
    pub id: SettingId,            // "looknfeel.blur.size"
    pub target: Target,           // file + dialect + key path (see spec 03)
    pub widget: Widget,           // Stepper{min,max,step} | Toggle | Color | Select | Chord | Text
    pub doc: &'static str,        // one-line human explanation (the "jargon strip" source)
    pub live: Option<LiveApply>,  // e.g. HyprctlKeyword("decoration:blur:size")
    pub requires: &'static [DepId], // dependency gates (spec 06)
    pub gate: VersionGate,        // omarchy/hyprland version constraints [gate]
}
```

Schemas are TOML files embedded via `include_str!` at build time (`data/schemas/*.toml` — added per module milestone), parsed once at startup into a registry. The TUI renders forms *from the schema*; the CLI resolves `get/set` paths against it. Adding a setting = adding data, not UI code.

## 3. The apply pipeline (every mutation goes through this)

```
plan → snapshot(pre) → write(atomic) → reload → verify → commit | rollback
```

```rust
pub struct ApplyPlan {
    pub edits: Vec<FileEdit>,          // produced by configfs from setting changes
    pub reload: Vec<ReloadStep>,       // ordered omarchy-restart-* / hyprctl reload / theme-refresh
    pub verify: Vec<Verification>,     // see below
    pub risk: Risk,                    // Safe | Restarty | Risky(revert_timeout)
}

pub enum Verification {
    HyprctlConfigErrorsEmpty,          // `hyprctl configerrors` [gate: HL ≥ 0.39]
    ProcessAlive { name: &'static str, grace: Duration },   // waybar watchdog
    CommandOk(Cmd),                    // e.g. `makoctl reload` exit 0
}
```

- `Risk::Risky` arms the 10-second confirm-or-revert countdown in the frontend (display-settings pattern).
- Rollback = restore pre-snapshot files + rerun the plan's reload steps. Rollback failures are loud, never silent, and print the snapshot id so the user can restore by hand (`omarchy-studio snapshot restore <id>`).
- Live preview (`LiveApply`) deliberately bypasses the pipeline — it must be instant — but every screen exit path ends with either `apply` (pipeline) or `revert` (`hyprctl reload`, restoring file truth).

## 4. Error model

```rust
pub enum StudioError {
    MissingDependency(DepReport),   // never a raw "No such file or directory"
    ParseFailed { file, line, hint },
    VerifyFailed { step, stderr, rolled_back: bool },
    OmarchyMismatch { expected, found, action },  // capability probe failures
    Io(...), External(...),
}
```

Frontend contract: every error renders as *what happened → why → what to do next* in plain language. `MissingDependency` renders as the guided-install banner (spec 06 §3), not as an error at all. Stack traces only under `--debug`.

## 5. Concurrency model (D3)

Single-threaded core; worker threads only for: wallhaven HTTP + thumbnail decode, local preview decode (one long-lived coalescing worker behind `tui::imagecell`), palette extraction, and `Cmd` execution with timeout. Frontends communicate via `std::sync::mpsc`. No tokio unless a real need appears (it hasn't: hyprctl/omarchy commands are all fast local execs).

## 6. Dependency policy for the crates themselves

Additions gated per milestone (see workspace `Cargo.toml` comments). Hard rules: no GTK/Qt linkage, no openssl (use rustls via ureq), binary target < 15 MB stripped, `cargo check` clean on stable. The skeleton compiles with **zero** external crates so CI is meaningful from day one.

## 7. Runtime file locations (XDG)

| Path | Purpose |
|---|---|
| `~/.config/omarchy-studio/config.toml` | Studio's own settings (wallhaven key, preview prefs) |
| `~/.config/omarchy-studio/integrations.toml` | User overrides/additions to the tool registry |
| `~/.local/state/omarchy-studio/history/` | Snapshot git repo (D4) |
| `~/.local/state/omarchy-studio/probe-cache.toml` | Last capability probe result + omarchy version seen |
| `~/.cache/omarchy-studio/wallhaven/` | Thumbnail cache (LRU, 200 MB cap) |

Studio never stores secrets anywhere else; the wallhaven API key lives in `config.toml` chmod 600.
