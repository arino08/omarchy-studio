```text
o m a r c h y
                                       ▄██  ▄█▄           
 ▄██████▄    ▄██       ▄█   █▄    ▄███▄███        ▄█████▄ 
███   █▀   ▄▄███▄▄▄   ███   ███  ███   ███  ▄██  ███   ███
███        ▀▀███▀▀▀   ███   ███  ███   ███  ███  ███   ███
 ▀██████▄    ███      ███   ███  ███   ███  ███  ███   ███
      ███    ███      ███   ███  ███   ███  ███  ███   ███
▄█    ███    ███      ███   ███  ███   ███  ███  ███   ███
███   ███    ███ ▄█   ███   ███  ███   ███  ███  ███   ███
 ▀█████▀     ▀████▀    ▀█████▀    ▀█████▀   ▀█▀   ▀█████▀ 
```

[![GitHub release (latest by date)](https://img.shields.io/github/v/release/arino08/omarchy-studio?style=flat-square&color=cba6f7)](https://github.com/arino08/omarchy-studio/releases/latest)
[![CI status](https://img.shields.io/github/actions/workflow/status/arino08/omarchy-studio/ci.yml?branch=main&style=flat-square&color=a6e3a1)](https://github.com/arino08/omarchy-studio/actions/workflows/ci.yml)
[![Platform: Linux](https://img.shields.io/badge/platform-linux%20%7C%20wayland-89b4fa?style=flat-square)](https://github.com/arino08/omarchy-studio)

**Rice your [Omarchy](https://omarchy.org) desktop without editing a single config file.**

Themes, keybinds, Waybar, animations, monitors, notifications — one keyboard-driven TUI, every change snapshotted to git, all of it undone with one key. There's a CLI twin for every screen when you'd rather script it.

![Tour: themes with live preview, wallpaper browser, theme wizard, integrations, power, doctor](docs/assets/tour.gif)

```bash
curl -sL https://raw.githubusercontent.com/arino08/omarchy-studio/main/install.sh | sh
omarchy-studio
```

> **Status: alpha, and honest about it.** Everything below is built, tested (264 tests, plus the TUI itself driven in a pty on every CI run) and drives the real Omarchy config on disk. v0.9.0 is the current release. Tested against Omarchy 3.8 / Hyprland 0.55 — Studio warns, but never refuses to run, on versions it hasn't seen.

## Why

Omarchy's menu picks a theme. Everything past that is hand-editing five config dialects across four directory trees, with no discoverability and no undo — and one typo in `hyprland.conf` away from a session that won't come back.

The community has solved colors six times over. Nobody built the control centre for the *behavioural* half: keybinds, animations, bar layout, notification rules, displays. Studio does both, and it does them the careful way: **it never writes to Omarchy's own files**, so `omarchy-update` can't clobber your work and your work can't break the update.

## Nothing you do here is one-way

This is the part that matters, so it's not buried at the bottom:

- **Every change is a git commit.** Studio snapshots the files before and after it touches them. `omarchy-studio snapshot undo` reverses the last change; the Snapshots screen browses the whole history with a diff and restores any point — and the restore is itself snapshotted.
- **A change that breaks your desktop rolls itself back.** Applies are verified (`hyprctl configerrors`, is the bar still alive?) and reverted automatically when the check fails. The Waybar watchdog is why you can't lock yourself out of your own bar.
- **Your hand-edits survive.** Edits are comment-preserving span splices into managed blocks, not rewrites. Studio's changes and yours coexist byte-for-byte, and drift is detected rather than steamrolled.
- **Omarchy's vendored files are never touched.** Only user-owned files and sanctioned extension surfaces, applied through `omarchy-theme-set` and friends.

## What it looks like

| | |
|---|---|
| ![Themes with live preview](docs/assets/themes.png) | ![Wallpaper browser with in-terminal previews](docs/assets/wallpapers.png) |
| ![Theme-from-wallpaper wizard](docs/assets/wizard.png) | ![Doctor health view](docs/assets/doctor.png) |

## What works today

**Look:** themes with a live preview (the theme's wallpaper plus a mock terminal in its palette), a 114-theme community browser that previews a theme's wallpaper *before* installing it, wallpapers with in-terminal previews from four sources, wallhaven search without leaving the TUI, and a wizard that builds a complete theme out of any image.

**Behaviour:** keybinds with live capture, 56 Hyprland look-and-feel settings with live preview on every adjust, animation presets, Waybar module layout with a crash-watchdog, mako notification rules, swayosd geometry, the hypridle timeline, and displays.

**System:** safe app removal with a real `pacman -Rs` cascade preview, power profiles and battery thresholds, one-key reversible tweaks, and a Doctor that tells you what's wrong in one screen.

**Moving and keeping:** rice bundles that carry a whole setup to another machine and replay it through the same verified pipeline, a keymap cheatsheet exported to themed HTML, theme-sync so fzf and lazygit follow your colours, and a self-updater that defers to pacman when it should.

<details>
<summary><strong>Every module in detail</strong> (what you can do, and when it landed)</summary>

| Module | What you can do | Since |
|---|---|---|
| **Themes** | List, show current, apply, fork — with a live preview pane: the theme's wallpaper + a mock terminal drawn in its palette | v0.1 |
| **Community themes** | Browse the 114-theme extra-themes directory in-TUI (`c`) with a **pre-install preview** — the theme's wallpaper and palette fetched straight from its repo, before anything touches disk; filter, installed markers, one-key install + apply | v0.9 |
| **Keybinds** | Browse and rebind Hyprland keybindings, live capture | v0.2 |
| **Look & Feel** | 56 Hyprland settings across Windows / Layout / Behavior / Decoration / Blur / Shadow / Input / Touchpad — live preview on every adjust, six presets, snapshot undo; input settings land in `input.conf`, the rest in `looknfeel.conf` | v0.3 · v0.8 |
| **Animations** | Apply curated animation presets | v0.3 |
| **Waybar** | Reorder/add/remove modules across lanes, tweak settings, font-size & radius, build custom modules — with a crash-watchdog that auto-reverts a config that kills the bar. Edits your *own* bar even when it lives outside the default path: detects a running `waybar -c/-s` and lets you point Studio anywhere (`f` in the screen, or `target set`) | v0.4 · v0.8 |
| **Notifications (mako)** | Behavior schema (timeouts, layout, urgency rules), do-not-disturb, live sample notifications | v0.5 |
| **OSD (swayosd)** | Volume/brightness popup geometry, percentage, margins, self-test | v0.5 |
| **Lock & Idle** | Retime the hypridle timeline (screensaver → lock → screen-off → suspend), hyprlock avatar/blur/dim | v0.5 |
| **Update survival** | Lifecycle hooks (`theme-set`, `post-update`) re-assert Studio's style blocks after theme changes and flag drift/clobbers after `omarchy-update` | v0.5 |
| **Doctor** | One health view: system facts, capability probes, hook status, drift report — in the TUI and the CLI | v0.5 |
| **Wallpapers** | Browse all four background sources (yours / theme / videos) with in-terminal previews (kitty / sixel / half-blocks), set/cycle/add/remove, `o` opens in imv/mpv | v0.6 |
| **Palette extraction** | Median-cut engine: image → full `colors.toml` (normal / muted / material modes, dark/light bias, WCAG-safe) | v0.6 |
| **Theme wizard** | `t` on any wallpaper (or `theme new --from-image`): live extraction → named, 100 % standard theme dir with preview.png | v0.6 |
| **wallhaven** | Browse wallhaven.cc without leaving the TUI (`w` in Wallpapers): live search, sort/ratio/color-match filters, thumbnail previews, download-and-set or feed straight into the theme wizard — plus the same search on the CLI | v0.6–0.7 |
| **Integrations** | Dependency health + companion-tool detection (Aether, Omarchist, matugen, hyprmon) with launch actions; "Open in Aether" appears in the wallpaper browser when installed | v0.6 |
| **Power** | Battery charge thresholds on ThinkPads & friends (`charge_control_*_threshold`) — no TLP needed; CLI can persist them across reboots. Plus power profiles (switch & persist at login) and an AC/battery auto-switch udev rule shown for approval before any root write | v0.6 · v0.8 |
| **Apps & services** | Remove bundled apps and web apps safely: a `pacman -Rs --print` **cascade preview** shows every orphaned dependency, enabled systemd units are disabled first, and pacman's refusal to break a dependency is surfaced as a blocker (never forced). Every removal is logged so `apps restore <id>` can reinstall it. No bulk-confirm bypass | v0.8 |
| **Monitors** | Detect displays (`hyprctl monitors -j`), identify which panel is which, set resolution and refresh rate from the modes the panel actually advertises (asking a 100Hz display for 200Hz is refused with the list of what it can do, not silently dropped), adjust scale with the effective resolution shown live, disable a display — written to `monitors.conf` as a managed block with a hotplug fallback, snapshot-backed | v0.8 |
| **Quick tweaks** | One-key reversible toggles (Caps→Escape, inactive-window transparency, screenshot/screencast folders…) — each a self-contained managed block, individually revertible, never touching Omarchy's vendored files | v0.8 |
| **Nice Launcher** | Drive the Nice Launcher (Quickshell app-search overlay): visual mode (constellation / spotlight / orbital / grid), backdrop, animation toggles and providers written to `nova.json`, snapshot-backed; optional launch keybind through the managed keybinds block; launch or install it straight from the rail. CLI `nova show|mode|set|anim|providers|keybind|launch|install|uninstall` | — |
| **Snapshots** | Browse every change Studio has ever made as a timeline with a live colored diff pane, and roll the whole tree back to any point — the restore is itself recorded, so it too can be undone. Only possible because the undo store is a real git repo. CLI `snapshot log`/`show <id>`/`restore <id>` | v0.9 |
| **Rice bundle** | Move a whole setup to another machine: `rice export` archives every config file Studio wrote (grouped by module) plus your own themes, and `rice import` **replays each file through the apply pipeline** — pre-snapshot, drift check, reload, verification, automatic rollback — instead of untarring over a live desktop. Machine-specific files (`monitors.conf`) and themes you already have are left alone unless asked for; `--dry-run` shows the whole plan first | v0.9 |
| **Theme sync** | Opt-in tools that follow your Omarchy theme — fzf and lazygit re-tint from the active `colors.toml` on every theme switch (via the `theme-set` hook). Studio never rewrites the tool's own config: it generates standalone files wired in through a single `~/.bashrc` managed block. CLI `themesync list`/`enable <tool>`/`apply` | v0.7 |
| **Self-update** | Daily release check; `U` in the TUI (or `omarchy-studio update`) downloads, swaps, and restarts — hands off to pacman for packaged installs | v0.7 |


</details>

## Install

Needs an Omarchy system (Arch + Hyprland). The installer downloads a release binary — no Rust toolchain required.

```bash
curl -sL https://raw.githubusercontent.com/arino08/omarchy-studio/main/install.sh | sh
```

It installs over whatever `omarchy-studio` is already on your `PATH` (or `~/.local/bin`), checks that what it downloaded is really a binary before replacing anything, and prints the version it installed.

<details>
<summary>Build from source instead (needs Rust 1.95+)</summary>

```bash
git clone https://github.com/arino08/omarchy-studio
cd omarchy-studio
cargo build --release
install -Dm755 target/release/omarchy-studio ~/.local/bin/omarchy-studio
```

</details>

Optional — add Studio to the Omarchy menu as a floating terminal app:

```bash
omarchy-studio install-integration    # undo with: omarchy-studio uninstall
```

Recommended — install the update-survival hooks so Studio's changes live through theme switches and `omarchy-update`:

```bash
omarchy-studio hooks install          # undo with: omarchy-studio hooks remove
```

## Quick start — the TUI

```bash
omarchy-studio          # launch the full-screen cockpit
```

| Key | Action |
|---|---|
| <kbd>Tab</kbd> / <kbd>Shift</kbd>+<kbd>Tab</kbd> | next / previous module |
| <kbd>Ctrl</kbd>+<kbd>j</kbd>/<kbd>k</kbd>, <kbd>Ctrl</kbd>+<kbd>Up</kbd>/<kbd>Down</kbd> | globally cycle through modules |
| **Mouse Click** | click rail tabs to instantly switch modules |
| <kbd>1</kbd>–<kbd>9</kbd>, <kbd>0</kbd> | jump to one of the first ten modules (from the rail) |
| <kbd>j</kbd> / <kbd>k</kbd>, arrows | move within a screen |
| <kbd>h</kbd> / <kbd>l</kbd> | adjust the selected value |
| <kbd>Enter</kbd> | open a picker (avatar, theme, …) |
| <kbd>c</kbd> | (themes) browse the community directory — preview before installing |
| <kbd>t</kbd> | (wallpapers) craft a theme from the selected image |
| <kbd>w</kbd> | (wallpapers) browse wallhaven.cc — enter sets, `t` themes |
| <kbd>o</kbd> | (wallpapers) open in imv / mpv |
| <kbd>s</kbd> | save pending edits (snapshotted first) |
| <kbd>U</kbd> | install a waiting update & restart |
| <kbd>/</kbd> | search · <kbd>?</kbd> help · <kbd>q</kbd> quit |

Studio themes itself from your active Omarchy theme — panels, highlights, and the wordmark all re-tint with every theme switch. Every rail entry is a real, working screen.

## CLI reference

Everything the TUI does is scriptable. Each mutating command prints its undo hint.

<details>
<summary><strong>View full CLI reference</strong></summary>

```bash
# Themes
omarchy-studio theme list | current | apply <name> | fork <src> <new>
omarchy-studio theme new <name> --from-image <path> [--mode normal|muted|material] [--bias auto|dark|light] [--apply]
omarchy-studio theme extract <image> [normal|muted|material] [auto|dark|light]
omarchy-studio theme community list | search <q> | install <name|owner/repo|url>   # install clones + applies
omarchy-studio theme new [name] --from-current-wallpaper [--apply]   # instant theme from what's on screen
omarchy-studio theme keybind install [MODS KEY] | remove | show      # opt-in SUPER+SHIFT+T for the above

# Wallpapers
omarchy-studio wallpaper list | current | set <n|name|path> | next | add <file> | remove <name>
omarchy-studio wallpaper wallhaven search <query> [--color <hex>] [--ratio 16x9] [--top] [--page N] [--download <n>]

# Keymap cheatsheet (effective binds + where each is defined, themed HTML)
omarchy-studio keybind cheatsheet [--out keymap.html]   # print-friendly, self-contained

# Snapshots / undo
omarchy-studio snapshot log                      # timeline: id, kind, summary, time
omarchy-studio snapshot show <id>                # files changed + colored diff
omarchy-studio snapshot restore <id>             # roll the whole tree back to <id>
omarchy-studio snapshot restore <id> --files <path>…   # restore only these files
omarchy-studio snapshot list | undo

# Theme sync (fzf, lazygit follow the active theme)
omarchy-studio rice export [--out f.tar.gz]      # bundle Studio's config files + your themes
omarchy-studio rice show <f.tar.gz>              # what's inside, by module
omarchy-studio rice import <f.tar.gz> [--themes] [--only waybar,mako] [--with-monitors] [--dry-run]
omarchy-studio themesync list                    # tools + on/off + installed state
omarchy-studio themesync enable <tool>           # opt a tool in, regenerate now
omarchy-studio themesync disable <tool>
omarchy-studio themesync apply                   # regenerate from the current theme

# Look & Feel
omarchy-studio looknfeel list | get <key> | set <key> <value>

# Animations
omarchy-studio animations list | current | apply <name>

# Presets (bundled look & feel + animation combos)
omarchy-studio preset list | try <name> | apply <name>

# Toggles
omarchy-studio toggle <name>

# Waybar
omarchy-studio waybar modules
omarchy-studio waybar add <lane> <id> | remove <lane> <id> | move <id> <lane>
omarchy-studio waybar set <path> <value>
omarchy-studio waybar new <name> --exec <cmd> [--interval N] [--format F] [--on-click C] [--lane left|center|right]
omarchy-studio waybar style show | font-size <n> | radius <n> | reset

# Custom config targets — point Studio at a relocated config
omarchy-studio target list                       # resolved path + source per module
omarchy-studio target set waybar.config <path>   # override (validated on set)
omarchy-studio target reset waybar.config        # back to the Omarchy default

# Apps & services — safe debloat with cascade preview + restore
omarchy-studio apps list [--installed] [--all]   # catalog with live install markers
omarchy-studio apps remove <id…> [--dry-run] [--yes]  # preview cascade/services, then run
omarchy-studio apps restore <id>                 # reinstall a logged removal
omarchy-studio apps leftovers                    # config dirs left by removed apps

# Monitors
omarchy-studio monitor list                      # displays, modes, scale, effective size
omarchy-studio monitor identify                  # flash each monitor's name on-screen
omarchy-studio monitor modes <name>              # every resolution/refresh the panel supports
omarchy-studio monitor mode <name> <WxH[@Hz]|Hz|preferred> [--dry-run]
omarchy-studio monitor scale <name> <f> [--dry-run]
omarchy-studio monitor apply [--dry-run]         # persist the current layout

# Quick tweaks — one-key reversible toggles
omarchy-studio tweak list                        # catalog with live on/off state
omarchy-studio tweak <id> on | off               # e.g. caps-escape, inactive-transparency

# Power profiles + AC/battery auto-switch
omarchy-studio power profile list | get          # profiles, active marker
omarchy-studio power profile set <name> [--persist]
omarchy-studio power auto <ac> <bat> [--yes]     # udev rule, previewed before the root write

# Notifications (mako)
omarchy-studio notif list | get <key> | set <key> <value>
omarchy-studio notif rule add <crit> <action> | rule remove <n>
omarchy-studio notif dnd on|off|status | test low|normal|critical

# OSD (swayosd)
omarchy-studio osd show
omarchy-studio osd set show-percentage <on|off> | max-volume <n> | top-margin <0..1> | radius <n> | font <pt>
omarchy-studio osd test

# Lock & Idle
omarchy-studio idle timeline
omarchy-studio idle set <screensaver|lock|screen-off|suspend> <seconds>
omarchy-studio lock show | avatar <path> | avatar list | size <px> | blur <n> | dim <0..1> | preview

# Battery charge thresholds (ThinkPads & friends — no TLP needed)
omarchy-studio battery [status]
omarchy-studio battery limit <start> <stop> [--persist]   # --persist under sudo survives reboots

# Update-survival hooks
omarchy-studio hooks install | remove | status

# Self-update (defers to pacman for packaged installs)
omarchy-studio update [--check]

# Health check (--quiet: terse drift report, exit 1 when something needs a look)
omarchy-studio doctor [--deps] [--quiet]  # warns when Omarchy is a major version we haven't tested
omarchy-studio --help | <command> --help  # the command list, and each command's usage
```

</details>

## Updating

Studio keeps itself current. On launch the TUI checks GitHub for a newer release (once a day, off-thread, silent when offline); when one exists the keybar shows **`U` update!** — press it and Studio downloads the release binary, swaps it in place, and restarts itself. The same flow is scriptable:

```bash
omarchy-studio update           # check + install + tell you to restart
omarchy-studio update --check   # just report
```

If the binary is owned by a pacman package (AUR install), Studio never touches it — it tells you to update through your package manager instead. Two knobs in `~/.config/omarchy-studio/config.toml`:

```toml
[update]
check = false   # disable the launch check entirely
auto = true     # swap + restart without waiting for U
```

## Design pillars

The five rules every feature is held to. The first four are why [nothing here is one-way](#nothing-you-do-here-is-one-way); the fifth is why a half-supported feature is never a silent failure.

1. **Never fight Omarchy** — only user-owned files and sanctioned extension surfaces.
2. **The file is still the truth** — comment-preserving span splices; your edits and Studio's coexist byte-for-byte.
3. **Show, don't describe** — live preview and confirm-or-revert, not a form you submit and hope.
4. **Undo is sacred** — git-backed snapshots of every change.
5. **Never strand the user** — a missing dependency is a visible-but-disabled feature carrying the exact install command.

## Repository map

| Path | What |
|---|---|
| `docs/PRD.md` | Product requirements — the *what* and *why* |
| `docs/specs/` | Build specs — the *how* (start at `00-overview.md`) |
| `ROADMAP.md` | Milestones broken into issue-sized tasks |
| `docs/comparison.md` | How Studio compares to A La Carchy |
| `docs/handoff-v0.8.md`, `docs/handoff-v0.9.md` | Architecture guides for the v0.8 / v0.9 milestones |
| `crates/studio-core/` | Engine library: config model, comment-preserving parsers, apply pipeline, snapshots, Omarchy adapter |
| `crates/omarchy-studio/` | The binary: TUI + CLI frontends |
| `tools/termshot/` | Renders tmux captures to the README media (screenshots + tour GIF) |
| `tools/e2e.sh` | Drives the TUI in a tmux pty against a fixture Omarchy — the CI journey test |
| `packaging/aur/` | PKGBUILD, refreshed against a released tag by `tools/refresh-pkgbuild.sh` |
| `data/animation-presets/` | Reference animation blocks (the shipped presets are compiled in) |

## License

MIT.
