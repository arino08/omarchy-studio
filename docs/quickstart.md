# Quickstart

Five minutes from nothing to a change you can undo.

## 1. Install

```bash
curl -sL https://raw.githubusercontent.com/arino08/omarchy-studio/main/install.sh | sh
```

The installer downloads the release binary, checks it against the release's
published SHA-256, and puts it on your `PATH` (`~/.local/bin` on a fresh
machine). From the AUR: `yay -S omarchy-studio`.

## 2. Check the machine

```bash
omarchy-studio doctor
```

Doctor reports your Omarchy and Hyprland versions, which config dialect
Hyprland is reading (`.conf` before Omarchy 4, `.lua` on it), the optional
tools Studio found, and anything that needs attention. Studio warns about an
Omarchy version it hasn't been tested against, but still runs.

## 3. Open the TUI

```bash
omarchy-studio
```

The first launch opens a welcome card that offers three things. Each one
can be skipped and redone later:

- **Change history**: the snapshot store that makes everything undoable.
- **Omarchy integration**: a Studio entry in the Omarchy menu, plus the hooks
  that keep your changes alive through theme switches and `omarchy-update`.
- **Terminal theme sync**: fzf and lazygit follow your theme.

## 4. Get around

| Key | Does |
|---|---|
| <kbd>Tab</kbd> / <kbd>Shift</kbd>+<kbd>Tab</kbd>, <kbd>Ctrl</kbd>+<kbd>j</kbd>/<kbd>k</kbd> | next / previous screen (or click the rail) |
| <kbd>↑</kbd> / <kbd>↓</kbd> | move within a screen |
| <kbd>←</kbd> / <kbd>→</kbd> | adjust the selected value (Look & Feel also takes <kbd>-</kbd> / <kbd>+</kbd>) |
| <kbd>Enter</kbd> | apply / open |
| <kbd>s</kbd> | save pending edits (snapshotted first) |
| <kbd>/</kbd> | command palette: search every screen and action |
| <kbd>?</kbd> | help for the screen you're on |
| <kbd>q</kbd> | quit |

## 5. Change something, then take it back

On **Look & Feel**, select *Inner gaps* (the space between windows) and press
<kbd>→</kbd> a few times. The desktop previews each step as you go. Press
<kbd>s</kbd> to save.

Now go to **Snapshots**. The change is at the top of the timeline, with its
diff beside it. Select the entry *before* it and press <kbd>r</kbd>: the
whole tree is restored to that point. The restore is itself a snapshot, so
you can undo the undo.

Switching themes is the one thing that isn't a snapshot. It's Omarchy's own
`omarchy-theme-set`, so to go back you apply the previous theme again. On
**Themes**, the right pane previews a theme's wallpaper and palette before
you press <kbd>Enter</kbd>.

From a shell, the same thing is:

```bash
omarchy-studio snapshot log
omarchy-studio snapshot undo
```

## Where next

- [Tutorial](tutorial.md): a full rice from a stock install, start to finish.
- [What file did that touch?](what-file.md): every file each screen writes.
- `omarchy-studio --help`, `omarchy-studio <command> --help`, and
  `omarchy-studio man | man -l -` for the CLI.
- Tab completion: `omarchy-studio completions bash|zsh|fish`. See the README.
