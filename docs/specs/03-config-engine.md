# Spec 03 — Config Engine: Parsers, Managed Blocks, Snapshots

The trust core. Everything here is `studio-core::configfs` + `studio-core::snapshot`.

## 1. Dialects & strategy per file

| Dialect | Files | Strategy | Crate |
|---|---|---|---|
| hyprlang | `~/.config/hypr/*.conf` (Omarchy ≤ 3) | **Line CST** (§2) — full parse, comment-preserving edits | hand-rolled |
| Lua | `~/.config/hypr/*.lua` (Omarchy 4+) | **Emit-only** (§2a) — managed block of `hl.*` calls | hand-rolled |
| TOML | `colors.toml`, `walker/config.toml`, `swayosd/config.toml`, Studio's own | lossless document edit | `toml_edit` |
| JSONC | `waybar/config.jsonc` | CST with comment/whitespace tokens (§3) | `jsonc-parser` (CST mode) or hand-rolled tokenizer if insufficient |
| INI (mako flavor) | generated `themed/mako.ini.tpl` | Studio owns the file → simple line model | hand-rolled |
| CSS | `waybar/style.css`, `swayosd/style.css` | **managed blocks only** (§4) — never a real CSS parse | hand-rolled |
| JSONC | `extensions/omarchy-menu.jsonc` | one member only (spec 02 §3.1) | span CST (§3) |

Principle: parse fully only what we must *understand* (hyprlang, JSONC, TOML). For everything else, own a clearly-marked region and treat the rest as opaque text.

Which Hyprland dialect is in play is resolved by `omarchy::Dialect::probe`, which asks the running compositor (`hyprctl systeminfo` → `configProvider: lua`). That is authoritative in a way a file-existence check is not: an upgraded machine keeps both `hyprland.conf` and `hyprland.lua`, and only one is live. Writing the wrong one is silently inert — the compositor simply never reads it — so every module that writes Hyprland config resolves the dialect first and picks its file, comment style (`#` vs `--`) and renderer from it.

## 2. Hyprlang line CST

hyprlang is line-oriented: `key = value`, `category { … }` nesting, `source = path`, `$var = value`, comments `#`. Model:

```rust
pub struct HyprDoc { lines: Vec<HyprLine> }        // lossless: every byte round-trips
pub enum HyprLine {
    Blank(String), Comment(String),
    Assign { path: Vec<String>, key: String, value: String, comment: Option<String>, raw: String },
    CategoryOpen(String), CategoryClose,
    Source(String), Var { name: String, value: String },
    Unknown(String),                                // never dropped, never touched
}
```

Operations: `get(path)`, `set(path, value)` (in-place edit preserving alignment; else append in the right category, creating it if needed), `remove`, `append_bind(BindLine)`, `find_bind(chord)`. Round-trip invariant: `parse(s).to_string() == s` for **every** file in Omarchy's `default/hypr/` and stock user configs (golden tests, spec 09).

## 2a. Lua emission (Omarchy 4)

Omarchy 4 loads the user's Lua modules *after* its own defaults, and a later `hl.config({…})` deep-merges over an earlier one. A managed block appended to the end of a user file therefore wins, exactly as a managed block of hyprlang lines did — the write model carries over unchanged, only the rendering differs.

Studio **emits** Lua and never parses arbitrary Lua. Two consequences:

- **Reading effective values** goes through `hyprctl -j --batch "getoption …"` — one exec for a whole schema, typed per option, and authoritative on either dialect. Gap options come back as a `css` quad; the first component is taken.
- **Reading back Studio's own block** uses a strict reader that understands only the shape `config_call` emits, proven its exact inverse by round-trip tests. Anything unrecognised is skipped rather than guessed at, so a hand-edited block degrades to "not overridden" instead of a wrong value.

Rendering is deterministic (keys sort, formatting fixed), so rewriting an unchanged block is a no-op and diffs stay legible. Emitted blocks are syntax-checked by the real `luac` in the golden suite.

Call forms in use, each verified against the running compositor rather than assumed: `hl.config{}` for settings, `hl.monitor{}` for displays (`disabled = true` turns one off — not `disable`, not `enabled`), `hl.curve` / `hl.animation` for animation feels, `o.bind` / `hl.unbind` for keys, `o.window` for window rules, and `require("hypr.<name>")` where hyprlang used `source =`.

**Keybinds are the one lossy case.** In Lua mode `hyprctl binds` reports every bind's dispatcher as `__lua` — an opaque callback id — so a bind read from the runtime carries no reproducible action. To move an action onto another key, Studio finds the `o.bind(...)` that declared it in `$OMARCHY_PATH/default/hypr/bindings/*.lua` (or the user's `bindings.lua`) and re-emits that action argument verbatim. Chords Omarchy generates in a `for` loop have no literal to match; those refuse with an explanation rather than writing a bind that would do nothing.

**The managed keybinds block is rewritten only if it reads back exactly.** Every keybind write re-renders the whole `omarchy-studio:keybinds` block in `bindings.lua` from what Studio read out of it, so the reader works on whole call statements (a `function() … end` action may span lines), and each `o.bind`/`hl.unbind` it reads must re-render to the same Lua tokens (`configfs::lua::same_tokens`). If any statement in the block fails that — an options table, a computed chord, a non-call statement, unbalanced parens — the write is refused naming `bindings.lua:<line>`, and the file is left untouched.

**Layering model** (read-side): to attribute a setting/bind to its source, the engine reads the same file list Hyprland does, in order: `default/hypr/*.conf` (via the user `hyprland.conf` source list) → theme `hyprland.conf` → user files → toggles. Effective value = last write; each value carries `SourceRef { file, line, layer }`. Studio only ever *writes* to the user layer (or theme files it owns).

## 3. Waybar JSONC

Requirements: preserve `//` comments and key order; edit nested module options; reorder the three lane arrays (`modules-left/center/right`). Approach: CST from `jsonc-parser` — mutate token stream, not a `serde_json::Value` (which would nuke comments). Validation before write: strip comments → `serde_json::from_str` must succeed. If the crate's CST proves too limited for array reordering, fall back plan (accepted risk, PRD §13): rewrite arrays only, byte-splice the rest.

## 4. Managed blocks (shared-ownership files)

```
/* >>> omarchy-studio:<section> — generated; edit outside this block only >>> */
…generated content…
/* <<< omarchy-studio:<section> <<< */
```

(Comment syntax per dialect: `#`, `/* */`, `//`.) Rules:

1. One block per `<section>`; replace-in-place on write; position = **end of file** (CSS cascade: later wins), except `waybar/style.css` where the block must come *after* the theme `@import` line — asserted on every write.
2. Content inside is fully regenerated from the schema each apply — no incremental edits, no drift.
3. If markers are found malformed (user edited inside), doctor flags it; apply refuses until resolved (keep-theirs / keep-ours / diff).
4. Everything outside a block is opaque and untouchable.

## 5. Atomic writes

`write(file, new_content)`: refuse if file changed on disk since our parse (mtime+hash check → re-parse and re-plan) → write `file.tmp.omarchy-studio` → fsync → `rename(2)` → restore mode/ownership. Symlink targets (e.g. anything under `current/`) are **never** written through — writes go to the real user-owned source file only; `mako/config` being a symlink into `current/theme/` is exactly why mako edits go through the tpl override (spec 05 §5).

## 6. Snapshot store (M9)

Plain git repo at `~/.local/state/omarchy-studio/history/` (created by first run; `git init -q`, local identity "omarchy-studio").

- **Layout:** mirrors absolute paths under `files/` (e.g. `files/home/ariz/.config/hypr/bindings.conf`). A `manifest.toml` records tracked files + why (which module claimed them).
- **Write protocol:** the apply pipeline stages `pre` state of every file in the plan → commit `pre: <plan summary>` → applies → commits `post: <plan summary>` with structured trailer (`Module: m4`, `Setting: blur.size`, `Old: 8`, `New: 12`). Snapshot 0 = state of all touched files at first run ("pre-Studio").
- **Undo** (`u` / `snapshot restore`): checkout the chosen commit's version of the affected files → copy back to real paths (atomic writes) → run the reload steps recorded in the commit trailer → record the restore itself as a new commit (history is append-only; undo is never a reset).
- **Drift:** before any apply, if the on-disk file ≠ last `post` state, auto-commit a `drift:` snapshot first (hand-edits are preserved in history too — free backup for the user).
- Shelling out to `git` (always present on Omarchy — it's installed by omarchy itself) rather than `git2` keeps the binary small. **[gate]** probe `git` anyway; missing → snapshots disabled, every apply requires explicit confirm, banner explains why (spec 06 pattern).

## 7. Validation & verification hooks

Per dialect validators run **before** write (syntax) and pipeline `Verification` runs **after** reload (semantics — `hyprctl configerrors`, process-alive, exit codes). Both failures map back to `SettingId` so the TUI highlights the exact field, per PRD FR3.6.
