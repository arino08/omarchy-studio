# Spec 02 — Omarchy Integration Contract

Everything Studio knows about Omarchy lives in `studio-core::omarchy`. **No other module may hardcode an Omarchy path or command.** Verified against Omarchy 3.8.2 (2026-07-04); anything marked **[gate]** goes through the capability probe (§5).

## 1. Path table

```rust
pub struct OmarchyPaths {
    pub system: PathBuf,        // ~/.local/share/omarchy        ($OMARCHY_PATH)
    pub config: PathBuf,        // ~/.config/omarchy
    pub state: PathBuf,         // ~/.local/state/omarchy
}
```

| Accessor | Path | Notes |
|---|---|---|
| `system_themes()` | `$OMARCHY_PATH/themes/` | read-only, 19 built-ins |
| `user_themes()` | `~/.config/omarchy/themes/` | user overlay dirs; same-slug = per-file overlay |
| `current_theme_dir()` | `~/.config/omarchy/current/theme/` | materialized merged dir (NOT a symlink) |
| `current_theme_name()` | `~/.config/omarchy/current/theme.name` | slug, authoritative |
| `current_background()` | `~/.config/omarchy/current/background` | symlink to active wallpaper |
| `user_templates()` | `~/.config/omarchy/themed/` | user `*.tpl` overrides (win over built-in) |
| `builtin_templates()` | `$OMARCHY_PATH/default/themed/` | `{{ key }}`, `{{ key_strip }}`, `{{ key_rgb }}` |
| `hooks_dir()` | `~/.config/omarchy/hooks/` | `<event>` file + `<event>.d/*` |
| `menu_ext()` | `~/.config/omarchy/extensions/omarchy-menu.jsonc` | single JSONC file, merged by the Quickshell menu plugin (Omarchy 4+) |
| `elephant_menus()` | `~/.config/elephant/menus/` | Walker Lua providers |
| `user_backgrounds(theme)` | `~/.config/omarchy/backgrounds/<theme>/` | user wallpaper additions |
| `toggles()` | `~/.local/state/omarchy/toggles/hypr/` | dynamic flag confs sourced by Hyprland |
| `hypr_user(file)` | `~/.config/hypr/{bindings,looknfeel,input,monitors,autostart,hypridle,hyprlock}.conf` | user layer, sourced LAST (wins) |
| `hypr_defaults()` | `$OMARCHY_PATH/default/hypr/` | read-only; bindings/*.conf, looknfeel.conf etc. |
| `mako_config()` | `~/.config/mako/config` | **symlink → current/theme/mako.ini**; behavior via tpl override only |
| `waybar()` | `~/.config/waybar/{config.jsonc,style.css}` | style.css line 1 `@import`s theme css — preserve it |
| `swayosd()` | `~/.config/swayosd/{config.toml,style.css}` | same @import pattern |
| `walker_config()` | `~/.config/walker/config.toml` | behavior; walker.css is theme-owned |

## 2. Command wrappers

All external invocations are typed constructors in `omarchy::cmd` (single choke point for testing/mocking and for the `--dry-run` global flag):

| Wrapper | Invokes | Used for |
|---|---|---|
| `theme_set(name)` | `omarchy-theme-set <name>` | apply theme (full pipeline incl. wallpaper + hooks) |
| `theme_refresh()` | `omarchy-theme-refresh` | re-template current theme, **no wallpaper change** — the post-palette-edit primitive |
| `theme_list/current/install(url)/remove(name)/update()` | matching bins | wrap, never reimplement |
| `bg_set(path)` / `bg_next()` | `omarchy-theme-bg-set/-next` | honors the machine's video patch (mpvpaper-rs/swww) **[gate]** |
| `restart(Component)` | `omarchy-restart-{waybar,mako,swayosd,hypridle,walker,terminal,btop}` | post-write reloads |
| `hypr_reload()` | `hyprctl reload` | restore/persist file truth |
| `hypr_keyword(k,v)` / `hypr_batch(pairs)` | `hyprctl [--batch] keyword …` | live preview (no file write) |
| `hypr_binds()` | `hyprctl binds -j` → serde | ground truth for M3 |
| `hypr_configerrors()` | `hyprctl configerrors` | verification **[gate]** |
| `hypr_dispatch(d, args)` | `hyprctl dispatch …` | "test bind" |
| `makoctl(reload\|mode…)` | `makoctl` | notification reloads, DND |
| `hook_install(event, file)` | `omarchy-hook-install` | §4 |
| `menu(deeplink)` | `omarchy-menu <substring>` | jump into Omarchy's own menus |
| `notify(urgency, msg)` | `notify-send` | M6 live test |

Execution: `Cmd` runs with a 10 s default timeout, captured stdout/stderr, env `LC_ALL=C`; failures surface as `StudioError::External` with the captured stderr in the "what happened" slot.

## 3. Menu integration (FR10.1/FR10.2)

### 3.1 `extensions/omarchy-menu.jsonc` row

Omarchy 4 ("Quattro") replaced the bash `omarchy-menu` with a Quickshell plugin. It merges **one** user JSONC file over its own defaults at startup, and hot-reloads it on save. Studio declares a single row there and leaves every other member — and the file's shipped help comments — byte-for-byte alone:

```jsonc
"style.studio": {"icon":"󰏘","label":"Studio","aliases":["studio","omarchy-studio"],"action":"omarchy-launch-floating-terminal-with-presentation omarchy-studio"}
```

The id is dotted, so the parent (`style`) is inferred and the row lands in the Style submenu. Declaring a row is **additive**, unlike the pre-4 mechanism below, so stock Style entries added by a later Omarchy still appear without Studio being reinstalled.

Install algorithm (idempotent):
1. File absent/blank → create `{}` and insert the row.
2. Key already present → replace its value in place (never a second copy).
3. Key absent → append after the last member, matching its indentation. A comment-only stub gets the row without a stray separating comma.
4. Uninstall removes exactly that member, by span rather than by comma-split chunk, so a preceding comment block survives.
5. Either direction also clears the dead pre-4 block (§3.1.1).

Both files are snapshotted before a change (`integration::managed_files`), so the migration stays undoable.

#### 3.1.1 Pre-Quattro `extensions/menu.sh` (removed)

Through Omarchy 3.x, `omarchy-menu` was bash and sourced `extensions/menu.sh`; Studio managed a marked block there that redefined `show_style_menu`. **Nothing sources that file in Omarchy 4** — which is why the entry silently vanished from every menu after the update. Studio no longer writes it, and install/uninstall delete its managed block (and the file, when Studio's block was all it held), leaving any of the user's own overrides in place.

The row's deep-links (entries like `omarchy-studio keybinds`) are defined in spec 07 §6.

### 3.2 Elephant provider

Ship `data/elephant/omarchy_studio.lua` → installed to `~/.config/elephant/menus/omarchy_studio.lua` (plain copy, header-marked). Entries: each Studio screen + saved profiles; `Action = "omarchy-launch-floating-terminal-with-presentation omarchy-studio <screen>"`. Registered automatically because Omarchy's walker already globs that dir. Removed on uninstall.

### 3.3 Launch & window behavior (FR10.3)

The TUI sets its terminal window class to `TUI.float.omarchy-studio` (via the launcher wrapper using `$TERMINAL --class=…` / alacritty `--class`), which Omarchy's stock rule (`default/hypr/apps/system.conf`: tag `floating-window` → float, center, 875×600) picks up with **zero rule installation**. **[gate]**: probe that the `TUI.float` pattern still exists; fallback = install our own windowrule line into `~/.config/hypr/looknfeel.conf` managed block.

## 4. Hooks (FR10.5)

| Hook file | Event | Studio behavior |
|---|---|---|
| `theme-set.d/50-omarchy-studio` | after any theme apply (slug in `$1`) | re-assert managed CSS blocks that themes could shadow; refresh Studio's cached palette |
| `post-update.d/50-omarchy-studio` | after `omarchy-update` | run `omarchy-studio doctor --quiet`; `notify-send` if drift/clobber found |

Installed via `omarchy-hook-install <event> <file>` (copies mode 755). Hook scripts are ≤10 lines of bash that just call `omarchy-studio` verbs — logic stays in the binary.

## 5. Capability probe (D7, PRD §10.4)

At startup (cached in `probe-cache.toml`, invalidated when `$OMARCHY_PATH/version` or `hyprctl version` changes):

```rust
pub struct Capabilities {
    pub omarchy_version: Version,        // ~/.local/share/omarchy/version
    pub hyprland_version: Version,       // hyprctl version
    pub has_configerrors: bool,          // hyprctl configerrors exits 0
    pub has_templates: bool,             // default/themed/ exists
    pub has_hooks: bool,                 // bin/omarchy-hook exists
    pub tui_float_rule: bool,            // grep default/hypr/apps/system.conf
    pub video_wallpapers: bool,          // mpvpaper-rs or mpvpaper in PATH
    pub omarchy_dirty: bool,             // git -C $OMARCHY_PATH status --porcelain non-empty
    pub graphics: TermGraphics,          // Kitty | Sixel | None (query at TUI start, not cached)
}
```

Policy: **unknown-newer Omarchy** → warn once, run with all features but verification extra-strict; **unknown-older** (< 3.0, no templates) → theme editing degrades to per-app file mode with a banner. `omarchy_dirty` → doctor warning (the reference machine's video-wallpaper patch is the canonical case).

## 6. Update survival rules (restate of PRD §3.6 — enforceable checklist)

1. Never write under `$OMARCHY_PATH` (CI greps the codebase for the literal path outside `omarchy/paths.rs`).
2. Everything Studio installs is listed in a manifest (`~/.local/state/omarchy-studio/installed.toml`) so doctor and uninstall are exact.
3. Files Omarchy's `omarchy-refresh-*` may clobber (`waybar/*`, `hypr/*`) get re-checked by the post-update hook: if a `.bak.<epoch>` sibling is newer than our last snapshot of that file, doctor offers 3-way: keep new default / re-apply Studio deltas / open diff.
