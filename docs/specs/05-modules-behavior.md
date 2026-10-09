# Spec 05 — Behavior Modules: Keybinds, Look & Feel, Animations, Waybar, Notifications, OSD, Lock/Idle

Covers PRD M3–M8. All writes go through the apply pipeline (spec 01 §3); all file access through spec 02 paths and spec 03 parsers.

## 1. Keybinds (M3, v0.2)

### 1.1 Read side — ground truth + attribution

1. `hyprctl binds -j` → `Vec<HyprBind>` (modmask, key/keycode, dispatcher, arg, submap, flags e/r/l/m).
2. Attribution pass: parse the full source layer list (spec 03 §2) collecting `bind*`/`unbind` lines; match each runtime bind to its defining line → `SourceRef { file, layer: Default|Theme|User|Toggle }`. Unmatched runtime binds (edge: plugins) show layer `Runtime`.
3. Human rendering: modmask → `SUPER+SHIFT+…`; dispatcher → plain-language table (`exec`→"Run", `movefocus`→"Focus window ‹dir›", ~40 entries in `data/schemas/dispatchers.toml` with arg widgets and docs). Unknown dispatchers render raw — never hidden.

### 1.2 Write side

- User binds live **only** in the user's bindings file — `bindings.conf` on Omarchy ≤ 3, `bindings.lua` on 4 (spec 03 §2a). New binds append to the Studio-managed section; binds that already exist as loose hyprlang user lines are edited in place (line CST).
- On Omarchy 4, `hyprctl binds` reports every dispatcher as `__lua`, so moving an existing action onto a new chord means recovering its declaration from the Lua sources; a chord Omarchy generates in a loop can't be recovered and is refused with an explanation rather than written as a dead bind.
- **Override a default:** write `unbind = MODS, KEY` + new `bind = …` pair (tagged with a trailing `# omarchy-studio: overrides <default file>` comment for attribution round-trip). **Disable** = `unbind` only. **Reset** = remove our pair; the sourced default resurfaces on reload.
- Conflict check is pure: `fn conflicts(chord, submap) -> Option<ExistingBind>` over the effective set; UI offers Override / Pick another / Cancel.
- Apply plan: write → `hyprctl reload` → verify `configerrors` empty → rollback on failure with the offending line mapped to the row (FR3.6).

### 1.3 Chord capture widget

Crossterm kitty-keyboard-protocol (all three Omarchy terminals support it) reports full modifiers+keysym; capture mode grabs the next chord, renders it, warns on unbindable combos (bare modifier, XF86 keys ok). Fallback for non-kitty terminals: manual picker lists (mods checkboxes + key search). `hyprctl dispatch` "test" fires the dispatcher once without binding.

## 2. Look & Feel (M4, v0.3)

Schema-driven form (`data/schemas/looknfeel.toml`): `general:gaps_in/out`, `border_size`, `col.active_border` (theme-linked toggle ↔ custom), `decoration:rounding`, `blur:{enabled,size,passes,noise}`, `shadow:*`, `dim_inactive`, `active/inactive_opacity`, `general:layout` + dwindle/master sub-opts. Every field: `live = HyprctlKeyword(...)`, `target = hypr_user(looknfeel.{conf,lua})`. On Omarchy 4 the effective value is read from `hyprctl -j --batch getoption` rather than parsed out of the defaults file, since those are Lua.

- **Live loop:** change → `hyprctl keyword` immediately (batched at 60 ms); footer shows "● previewing — s save · esc revert". Save = pipeline (write file + `hyprctl reload` + verify). Esc/quit = `hyprctl reload` (file truth restores). Crash-safety: a `preview-active` marker file is written on first keyword; startup finds it stale → offers reload.
- Theme-linked colors: writing a custom border color inserts into user looknfeel.conf (wins over theme's `$activeBorderColor`); "re-link to theme" removes our line.
- Toggle flags (FR4.5): surface `~/.local/state/omarchy/toggles/hypr/*` as switches calling the corresponding `omarchy-toggle-*` bins where they exist **[gate]**, else writing the toggle conf directly.

## 3. Animations (M4, v0.3)

- **Preset format** (`data/animation-presets/*.toml`, user additions in `~/.config/omarchy-studio/animation-presets/`):

```toml
name = "Snappy"
description = "Fast, minimal overshoot; feels wired to your fingers"
[beziers]
snap = [0.2, 0.9, 0.1, 1.0]
[animations]           # target = enabled, duration(ds), curve, style?
windows    = { on = true, speed = 3, bezier = "snap", style = "popin 90%" }
workspaces = { on = true, speed = 4, bezier = "snap", style = "slide" }
fade       = { on = true, speed = 2, bezier = "default" }
border     = { on = true, speed = 5 }
```

Ship 8 (PRD FR4.3): omarchy-default (extracted from `default/hypr/looknfeel.conf` verbatim), snappy, silky, minimal, zoomy, slide, off-performance, battery-saver. `omarchy-default` is special: "reset to stock".
- **Try** = translate preset → `hyprctl --batch keyword animation …,bezier …` (no write). **Apply** = render into the managed section of the user's look & feel file via the pipeline — beziers then animation lines on hyprlang, `hl.curve` then `hl.animation` calls on Lua. Presets are defined structurally and rendered per dialect, so there is one definition rather than two.
- **Bezier editor (FR4.4):** braille-canvas curve plot (ratatui Canvas), handles nudged with hjkl/arrows, animated dot preview along the curve, save as personal preset. Behind `--features bezier-editor` until v0.3 polish.

## 4. Waybar (M5, v0.4)

- **Lanes editor:** three reorderable lists bound to `modules-left/center/right` arrays (JSONC CST, spec 03 §3). Module catalog `data/schemas/waybar-modules.toml`: ~30 stock modules with description, default config snippet, and deps (e.g. `wireplumber` module needs wireplumber — FR11.6 gate).
- **Module forms:** high-traffic settings typed (clock format w/ live strftime render, battery states, hyprland/workspaces opts, tray spacing); everything else in a generic key/value editor bound to the module's JSON object — unknown keys preserved.
- **Geometry & style (FR5.4):** position/height/margins/font/spacing rendered into the managed block of `style.css`, always after the theme `@import` (assert, spec 03 §4). Colors stay theme-owned; per-module color overrides only if Ariz answers PRD Q5 "yes" (adds `@define-color` overrides in the managed block).
- **Custom module wizard:** scaffolds `~/.config/waybar/scripts/<name>.sh` (exec bit set, stub with interval/json hints) + `custom/<name>` entry wired into a lane.
- **Apply:** validate JSONC → write both files → `omarchy-restart-waybar` → watchdog: waybar alive after 3 s else rollback + restart (FR5.6). Waybar 0.15 has no reliable reload signal path under Omarchy's uwsm launch — restart is the Omarchy-native mechanism anyway.

## 5. Notifications — mako (M6, v0.5)

Omarchy wiring recap (spec 02 §1): `~/.config/mako/config` → symlink → `current/theme/mako.ini` ← generated from `mako.ini.tpl` ← includes read-only `default/mako/core.ini`. Therefore:

- Studio materializes `~/.config/omarchy/themed/mako.ini.tpl` (user template, wins over built-in) shaped as:
  1. `include=~/.local/share/omarchy/default/mako/core.ini` (keep inheriting upstream behavior),
  2. theme color placeholders (copied from built-in tpl so theming still works),
  3. Studio-managed section: behavior overrides (later keys win in mako) + per-app/urgency criteria sections `[app-name=…]`, `[urgency=critical]`.
- Editable schema (`data/schemas/mako.toml`): default-timeout, anchor/position, max-visible, max-history, icon size/path, grouping, border/padding/width/height, per-urgency timeout+border-color(theme-linked), per-app rule builder (criteria: app-name, urgency, mode; actions: timeout, invisible, anchor).
- Apply = write tpl → `omarchy-theme-refresh` (regenerates mako.ini) → `makoctl reload` → verify exit 0. Live test row: `notify-send` low/normal/critical samples (FR6.3). DND toggle = `makoctl mode -a do-not-disturb` / `-r` reflecting current `makoctl mode`.
- Doctor check: warn if user replaced the symlink at `~/.config/mako/config` with a real file (Studio's mako editing then targets that file directly instead — degrade gracefully, banner explains).

## 6. SwayOSD (M6, v0.5)

`config.toml` (toml_edit): `top_margin`, `max-volume`, `show-percentage`; style knobs (size, radius — geometry only) in the managed block of `style.css` after the theme import. Apply → `omarchy-restart-swayosd` (systemd user unit dance is inside the omarchy bin — never reimplement) → self-test button: `swayosd-client --output-volume raise 0` (no-op bump) to flash the OSD.

## 7. Lock & Idle (M7, v0.5)

- **hypridle** (`~/.config/hypr/hypridle.conf`, line CST): model the `listener` blocks as a timeline (dim → lock → dpms off → suspend), each with timeout + on/off. Unknown listeners preserved untouched. AC/battery profiles = two listener sets switched by Studio's generated `on-battery` wrapper only if Ariz wants it (defer; v0.5 ships single profile). Apply → `omarchy-restart-hypridle`.
- **hyprlock** (`~/.config/hypr/hyprlock.conf`): edit `label` clock format, avatar `image` path (picker over `~/.config/omarchy/profile_images/`), `blur`/`dim` floats. Colors remain theme-owned (`current/theme/hyprlock.conf` sourcing stays intact). First edit: create `lock-screen-backup.<epoch>/` copy honoring Omarchy's convention *and* snapshot. "Preview" = run `hyprlock` (it's just… the lock screen; warn first).

## 8. Walker styling (M8, P2 — v1.0+)

`walker/config.toml` via toml_edit: width, max rows, fuzziness/score threshold, placeholders. Nothing else; walker.css is theme-owned.
