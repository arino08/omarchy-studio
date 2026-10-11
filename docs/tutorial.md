# Tutorial: a stock Omarchy to a finished rice

This walks through a full setup on a stock Omarchy install: a theme built
from a wallpaper, window shape and motion, keys, the bar, and finally a
bundle you can carry to another machine. Each step shows the TUI way and
the CLI way; they do the same thing and write the same files.

Nothing here is one-way. Every step except switching themes is a snapshot,
and `omarchy-studio snapshot undo` reverses the last one. When in doubt,
look at the **Snapshots** screen.

## 0. Set up once

```bash
omarchy-studio doctor               # versions, dialect, anything missing
omarchy-studio hooks install        # keep your changes through theme switches and omarchy-update
omarchy-studio install-integration  # add Studio to the Omarchy menu
```

The first TUI launch offers the last two for you. If Doctor says Hyprland
reads `.lua`, you're on Omarchy 4. Studio writes the Lua files there, and a
few steps below differ (see step 5).

## 1. Start from a theme

Open `omarchy-studio` and stay on **Themes**. Moving through the list
previews each theme's wallpaper and palette in the right pane. Press
<kbd>c</kbd> to browse the community directory (more than a hundred themes).
It previews a theme's wallpaper and palette *before* anything is installed,
and <kbd>Enter</kbd> installs and applies it.

```bash
omarchy-studio theme community search nord
omarchy-studio theme community install <name>
```

Applying a theme is Omarchy's own `omarchy-theme-set`, the one step that
isn't a snapshot. To go back, apply the old theme again.

## 2. Make it yours: a wallpaper, then a theme from it

On **Wallpaper**, press <kbd>w</kbd> to search wallhaven without leaving the
TUI: filter by ratio or colour, then <kbd>Enter</kbd> to download and set.
With the wallpaper you want selected, press <kbd>t</kbd>. The theme wizard
extracts a full palette from the image, shows it live, and writes a standard
Omarchy theme folder with a preview image.

```bash
omarchy-studio wallpaper wallhaven search "misty forest" --ratio 16x9
omarchy-studio wallpaper wallhaven search "misty forest" --ratio 16x9 --download 3
omarchy-studio theme new my-rice --from-current-wallpaper --apply
```

`--mode muted` or `--mode material` and `--bias dark|light` change how the
palette is extracted (`theme new my-rice --from-image <path> --mode muted`).
To fine-tune single colours, press <kbd>e</kbd> on the theme in **Themes**:
the palette editor previews every edit on the desktop as you go.

## 3. Shape the windows

**Look & Feel** has 56 Hyprland settings in tabs (<kbd>Tab</kbd> to switch):
windows, layout, behaviour, decoration, blur, shadow, input, touchpad. Each
<kbd>←</kbd>/<kbd>→</kbd> previews live, and <kbd>s</kbd> saves. Press
<kbd>p</kbd> for presets, which are a good starting point:

```bash
omarchy-studio preset list
omarchy-studio preset try cozy        # live, not saved
omarchy-studio preset apply cozy
omarchy-studio looknfeel set decoration.rounding 12
omarchy-studio looknfeel set general.gaps_out 14
```

`looknfeel list` shows every key, and marks the ones you've changed from
Omarchy's default with `*`.

## 4. Motion

```bash
omarchy-studio animations list
omarchy-studio animations apply smooth
```

Or use the **Animations** screen.

## 5. The bar

**Omarchy 3 (Waybar).** On the **Waybar** screen, <kbd>Shift</kbd>+<kbd>↑</kbd>/<kbd>↓</kbd>
reorders a module within its lane, <kbd>←</kbd>/<kbd>→</kbd> moves it to the
next lane, <kbd>a</kbd> adds one from the catalog (or builds a custom module
from a command), and <kbd>s</kbd> saves. Saving restarts Waybar under a
watchdog: if the new config kills the bar, the old one is put back
automatically.

```bash
omarchy-studio waybar modules
omarchy-studio waybar move clock center
omarchy-studio waybar style font-size 13
```

**Omarchy 4 (the Quickshell shell).** The same rail slot edits the shell's
bar through Omarchy's own `omarchy bar` command:

```bash
omarchy-studio shell bar list
omarchy-studio shell bar move clock --section center
omarchy-studio shell bar position top
omarchy-studio shell appearance set font-size 13
```

## 6. Keys

On **Keybinds**, select a binding and press <kbd>r</kbd>, then press the new
chord: Studio moves the action to the chord you pressed. <kbd>x</kbd>
disables one. Check for clashes afterwards with `keybind list --conflicts`. Your changes go into a managed block in your own bindings
file, never Omarchy's.

```bash
omarchy-studio keybind list --conflicts
omarchy-studio keybind add SUPER+ALT+B exec firefox
omarchy-studio theme keybind install   # SUPER+SHIFT+T: theme from the current wallpaper
omarchy-studio keybind cheatsheet --out ~/keymap.html
```

## 7. Displays (if you have more than one)

**Monitors** shows a proportional map of your displays. <kbd>m</kbd> and
<kbd>r</kbd> cycle through the resolutions and refresh rates the panel
actually advertises, <kbd>+</kbd>/<kbd>-</kbd> change the scale, and
<kbd>p</kbd> places the selected screen against another (left of, above,
centred). Studio warns about overlaps and unreachable screens before you
save.

```bash
omarchy-studio monitor list
omarchy-studio monitor place eDP-1 below DP-1 --align center --dry-run
omarchy-studio monitor place eDP-1 below DP-1 --align center
```

## 8. Look back, keep it, carry it

```bash
omarchy-studio snapshot log                 # everything you just did
omarchy-studio snapshot show <id>           # what one step changed
omarchy-studio rice export --out ~/my-rice.tar.gz
omarchy-studio theme export my-rice         # just the theme, to share
```

On the other machine (after installing Studio):

```bash
omarchy-studio rice show my-rice.tar.gz
omarchy-studio rice import my-rice.tar.gz --themes --dry-run
omarchy-studio rice import my-rice.tar.gz --themes
```

Import replays every file through the same checked pipeline as any other
change: a snapshot first, then the write, a Hyprland reload and a check for
config errors, with an automatic rollback if the check fails. Monitor
layouts stay behind unless you ask for them (`--with-monitors`), because
they describe the old machine's screens. If the bundle would add anything
that can run a program (an `exec-once`, a Waybar `on-click`, shell lines in
`~/.bashrc`), Studio lists those lines and asks before importing.

## If something goes wrong

- `omarchy-studio snapshot undo` reverses the last change. The **Snapshots**
  screen restores any earlier point.
- `omarchy-studio doctor` explains most problems in one screen, including a
  theme that's missing a file your editor needs.
- [What file did that touch?](what-file.md) lists every file each screen
  writes.
