# What file did that touch?

Studio only writes files *you* own, never the files Omarchy ships under
`~/.local/share/omarchy`. Most edits go into a **managed block** inside a
file, fenced by comments like this one:

```ini
# >>> omarchy-studio:looknfeel — generated; edit outside this block only >>>
general {
    gaps_in = 8
}
# <<< omarchy-studio:looknfeel <<<
```

Everything outside the fence is yours and is kept byte-for-byte. If you edit
inside it by hand, Studio notices the drift and snapshots your version
before writing over it.

## Ask Studio instead of guessing

Every change Studio makes is a commit in its snapshot store, and each commit
lists the files it touched:

```bash
omarchy-studio snapshot log            # newest first: id, kind, what, when
omarchy-studio snapshot show <id>      # the files that change touched + a coloured diff
omarchy-studio snapshot undo           # reverse the last change
```

The Snapshots screen in the TUI shows the same history, with the diff in a
side pane.

## The map

Paths are relative to your home directory. Omarchy 4 ("Quattro") moved
Hyprland's config from hyprlang (`.conf`) to Lua (`.lua`). Studio asks
Hyprland which one it's reading (`hyprctl systeminfo`) and writes that one.
On an upgraded machine the old `.conf` files are still there, but nothing
reads them. `omarchy-studio migrate` cleans Studio's blocks out of them.

### Hyprland

| Module | Omarchy 3 | Omarchy 4 |
|---|---|---|
| Look & Feel (window, layout, decoration, blur, shadow) | `.config/hypr/looknfeel.conf` | `.config/hypr/looknfeel.lua` |
| Look & Feel (input, touchpad) | `.config/hypr/input.conf` | `.config/hypr/input.lua` |
| Animations | `.config/hypr/looknfeel.conf` | `.config/hypr/looknfeel.lua` |
| Keybinds, plus the opt-in instant-theme bind (`theme keybind install`) | `.config/hypr/bindings.conf` | `.config/hypr/bindings.lua` |
| Monitors | `.config/hypr/monitors.conf` | `.config/hypr/monitors.lua` |
| Quick tweaks: Caps→Escape | `.config/hypr/input.conf` | `.config/hypr/input.lua` |
| Quick tweaks: inactive-window transparency | `.config/hypr/looknfeel.conf` | `.config/hypr/looknfeel.lua` |
| Power profile kept across logins (`power profile set --persist`) | `.config/hypr/autostart.conf` | `.config/hypr/autostart.lua` |
| Niri Mode (ScrollOverview) settings | `.config/hypr/scrolloverview.conf`, sourced from `hyprland.conf`; plugin autoload in `autostart.conf` | `.config/hypr/scrolloverview.lua`, required from `hyprland.lua`; plugin autoload in `autostart.lua` |

### Bar, notifications, OSD, lock and idle

Omarchy 4 replaced Waybar, Mako, SwayOSD and hypridle with its Quickshell
shell, so those screens switch over to the shell there.

| Module | Omarchy 3 | Omarchy 4 |
|---|---|---|
| Bar layout | `.config/waybar/config.jsonc` (or your `target set` override) | `.config/omarchy/shell.json`, through Omarchy's own `omarchy bar` / `omarchy plugin` |
| Bar styling (font size, radius) | `.config/waybar/style.css` | `.config/omarchy/shell.toml` (`[font]`, `[bar]`, `[spacing]` only) |
| Notifications | `.config/omarchy/themed/mako.ini.tpl`, a user template Omarchy prefers over its own, so it survives theme switches | — |
| OSD | `.config/swayosd/config.toml`, `.config/swayosd/style.css` | — |
| Idle timeline | `.config/hypr/hypridle.conf` | `.config/omarchy/shell.json` (`idle`) |
| Lock screen | `.config/hypr/hyprlock.conf`; avatars in `.config/omarchy/profile_images/` | — |

### Themes and wallpapers

| Action | Writes |
|---|---|
| Fork a theme, the theme wizard, `theme new` | `.config/omarchy/themes/<slug>/` (a new folder) |
| Edit a palette | `.config/omarchy/themes/<slug>/colors.toml`. For a built-in theme this is an overlay folder of the same name, and the original stays untouched |
| Install a community theme | `.config/omarchy/themes/<slug>/`, via Omarchy's `omarchy-theme-install` |
| Apply a theme | Nothing directly: Studio runs `omarchy-theme-set` |
| Add a wallpaper | `.config/omarchy/backgrounds/<theme>/` |
| wallhaven download | `.config/omarchy/backgrounds/<theme>/` |

### Everything else

| Module | Writes |
|---|---|
| Theme sync (fzf, lazygit) | Generated files in `.config/omarchy-studio/themesync/`, plus one managed block in `.bashrc` that sources them |
| Omarchy menu entry (`install-integration`) | `.config/omarchy/extensions/omarchy-menu.jsonc` on Omarchy 4; `.config/omarchy/extensions/menu.sh` before it |
| Update-survival hooks (`hooks install`) | `.config/omarchy/hooks/theme-set.d/` and `.config/omarchy/hooks/post-update.d/` |
| Nice Launcher | `.config/nova/nova.json` |
| Battery charge limits | `/sys/class/power_supply/BAT*/charge_control_*_threshold`, which resets at reboot. `--persist` (as root) installs a systemd oneshot that reapplies it at boot |
| AC/battery auto-switch (`power auto`) | `/etc/udev/rules.d/99-power-profile.rules`. Studio prints the rule and the `sudo` command and asks first; it never runs `sudo` silently |
| Apps & services | Nothing in your config: Studio runs `pacman -Rs` / `omarchy-webapp-remove`, and logs each removal to `.local/state/omarchy-studio/removed.toml` for `apps restore` |
| Custom config targets (`target set`) | `.config/omarchy-studio/config.toml`, `[targets]` |

## Studio's own files

| Path | What it is |
|---|---|
| `.config/omarchy-studio/config.toml` | Studio's settings: `[update]` checks, `[targets]` overrides, theme-sync choices |
| `.local/state/omarchy-studio/history/` | The snapshot store: a git repository of every change. `git -C` into it if you like |
| `.local/state/omarchy-studio/onboarded` | Marks that the first-run card has been shown |
| `.cache/omarchy-studio/` | Previews, the community-theme cache, scratch space for rice imports |

`omarchy-studio uninstall` removes the hooks, the menu entry and the
theme-sync wiring, then lists what it deliberately left in place (your
settings, the history, Studio's config) with the command to remove each.
