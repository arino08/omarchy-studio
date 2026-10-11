//! Shell completions and the man page.
//!
//! Each verb group parses its own leaf arguments (see the note atop
//! `main.rs`), so a clap-generated script would stop at the group name —
//! `theme <TAB>` would offer nothing. Instead the installed script is a thin
//! shim that asks the binary itself: `omarchy-studio __complete <words…>`
//! prints the candidates for the last word, one per line. That keeps every
//! verb table here, next to the code, and lets completion offer live values
//! — your theme slugs, the tweak catalog, Look & Feel keys — that no static
//! script could know.
//!
//! Printing nothing means "no idea": the shims fall back to file names,
//! which is right for the arguments that are paths (`rice import`, `theme
//! new --from-image`, `wallpaper add`).

use clap::Command;

const MODES: &[&str] = &["normal", "muted", "material"];
const BIAS: &[&str] = &["auto", "dark", "light"];

/// Candidates for the word after `prev` (the words already typed after the
/// program name), unfiltered.
fn candidates(prev: &[&str]) -> Vec<String> {
    // A flag that takes one of a fixed set of values wins wherever it is.
    let by_flag: Option<&[&str]> = match prev.last() {
        Some(&"--mode") => Some(MODES),
        Some(&"--bias") => Some(BIAS),
        Some(&"--align") => Some(&["start", "center", "end"]),
        Some(&"--section") | Some(&"--from-section") => Some(&["left", "center", "right"]),
        _ => None,
    };
    if let Some(values) = by_flag {
        return owned(values);
    }

    let s: &[&str] = match prev {
        [] => return groups(),
        ["doctor", ..] => &["--deps", "--quiet"],
        ["migrate"] => &["--dry-run"],
        ["theme"] => &[
            "list",
            "current",
            "coverage",
            "apply",
            "fork",
            "new",
            "extract",
            "community",
            "keybind",
            "export",
        ],
        ["theme", "apply" | "coverage" | "fork" | "export"] => return theme_slugs(),
        // Read verbs that answer `--json` (main.rs `json_out`).
        ["theme", "list" | "current"]
        | ["snapshot", "log" | "list"]
        | ["tweak" | "toggle" | "looknfeel" | "themesync", "list"]
        | ["looknfeel", "get", _] => &["--json"],
        ["theme", "export", _] => &["--out"],
        ["theme", "community"] => &["list", "search", "install"],
        ["theme", "keybind"] => &["install", "remove", "show"],
        ["theme", "new", _, ..] => &[
            "--from-image",
            "--from-current-wallpaper",
            "--mode",
            "--bias",
            "--apply",
        ],
        ["theme", "extract", _] => MODES,
        ["theme", "extract", _, _] => BIAS,
        ["wallpaper"] => &[
            "list",
            "current",
            "set",
            "next",
            "add",
            "remove",
            "wallhaven",
        ],
        ["wallpaper", "wallhaven"] => &["search"],
        ["wallpaper", "wallhaven", "search", _, ..] => {
            &["--color", "--ratio", "--top", "--page", "--download"]
        }
        ["keybind"] => &[
            "list",
            "check",
            "add",
            "disable",
            "remove",
            "reset",
            "cheatsheet",
        ],
        ["keybind", "list"] => &["--conflicts"],
        ["keybind", "cheatsheet"] => &["--out"],
        ["snapshot"] => &["log", "list", "show", "restore", "undo"],
        ["snapshot", "restore", _, ..] => &["--files"],
        ["rice"] => &["export", "show", "import"],
        ["rice", "export"] => &["--out"],
        ["rice", "import", _, ..] => &[
            "--only",
            "--themes",
            "--with-monitors",
            "--dry-run",
            "--yes",
        ],
        ["themesync"] => &["list", "enable", "disable", "apply"],
        ["themesync", "enable" | "disable"] => {
            use studio_core::modules::themesync;
            return themesync::catalog()
                .iter()
                .map(|t| t.id.to_string())
                .collect();
        }
        ["looknfeel"] => &["list", "get", "set"],
        ["looknfeel", "get" | "set"] => {
            use studio_core::modules::looknfeel::SETTINGS;
            return SETTINGS.iter().map(|s| s.key.to_string()).collect();
        }
        ["preset"] => &["list", "try", "apply"],
        ["preset", "try" | "apply"] => {
            use studio_core::modules::looknfeel::PRESETS;
            return PRESETS.iter().map(|p| p.name.to_string()).collect();
        }
        ["toggle"] => {
            use studio_core::modules::toggles::TOGGLES;
            let mut v = vec!["list".to_string()];
            v.extend(TOGGLES.iter().map(|t| t.id.to_string()));
            return v;
        }
        ["animations"] => &["list", "current", "apply"],
        ["animations", "apply"] => {
            use studio_core::modules::animations::PRESETS;
            return PRESETS.iter().map(|p| p.name.to_string()).collect();
        }
        ["waybar"] => &["modules", "add", "remove", "move", "set", "new", "style"],
        ["waybar", "new", _, ..] => &["--exec", "--interval", "--format", "--on-click", "--lane"],
        ["waybar", "style"] => &["show", "font-size", "radius", "reset"],
        ["notif"] => &["list", "get", "set", "rule", "dnd", "test"],
        ["notif", "rule"] => &["add", "remove"],
        ["notif", "dnd"] => &["on", "off", "status"],
        ["notif", "test"] => &["low", "normal", "critical"],
        ["osd"] => &["show", "test", "set"],
        ["osd", "set"] => &[
            "show-percentage",
            "max-volume",
            "top-margin",
            "radius",
            "font",
        ],
        ["osd", "set", "show-percentage"] => &["on", "off"],
        ["shell"] => &["idle", "bar", "plugin", "appearance"],
        ["shell", "idle"] => &["show", "set"],
        ["shell", "idle", "set"] => &["screensaver", "lock"],
        ["shell", "bar"] => &[
            "list",
            "catalog",
            "position",
            "transparent",
            "move",
            "put",
            "set",
            "defaults",
        ],
        ["shell", "bar", "position"] => &["top", "bottom", "left", "right"],
        ["shell", "bar", "transparent"] => &["true", "false", "toggle"],
        ["shell", "bar", "move" | "put", _, ..] | ["shell", "plugin", "enable", _, ..] => &[
            "--section",
            "--index",
            "--before",
            "--after",
            "--from-section",
            "--from-index",
        ],
        ["shell", "plugin"] => &["list", "enable", "disable"],
        ["shell", "appearance"] => &["show", "set"],
        ["shell", "appearance", "set"] => &[
            "font-size",
            "bar-size-horizontal",
            "bar-size-vertical",
            "bar-scale-with-font",
            "spacing-scale",
            "spacing-scale-with-font",
        ],
        ["idle"] => &["timeline", "set"],
        ["idle", "set"] => &["screensaver", "lock", "screen-off", "suspend"],
        ["lock"] => &["show", "avatar", "size", "blur"],
        ["lock", "avatar"] => &["add", "list"],
        ["monitor"] => &[
            "list",
            "identify",
            "check",
            "modes",
            "mode",
            "scale",
            "position",
            "place",
            "arrange",
            "normalize",
            "apply",
        ],
        ["monitor", "arrange"] => &["row", "column"],
        ["monitor", "place", _] => &["left-of", "right-of", "above", "below"],
        ["monitor", "position", _] => &["left", "right"],
        ["monitor", "mode", _] => &["preferred"],
        ["monitor", "mode" | "scale" | "position" | "place" | "arrange" | "normalize" | "apply", ..] => {
            &["--dry-run", "--align"]
        }
        ["apps"] => &["list", "remove", "restore", "leftovers"],
        ["apps", "list"] => &["--installed", "--all"],
        ["apps", "remove", ..] => &["--dry-run", "--yes"],
        ["tweak"] => {
            use studio_core::modules::tweaks;
            let mut v = vec!["list".to_string()];
            v.extend(tweaks::catalog().iter().map(|t| t.id().to_string()));
            return v;
        }
        ["tweak", id] if *id != "list" => &["on", "off"],
        ["power"] => &["profile", "auto"],
        ["power", "profile"] => &["list", "get", "set"],
        ["power", "profile", "set", _] => &["--persist"],
        ["power", "auto", ..] => &["--yes"],
        ["battery"] => &["status", "limit"],
        ["battery", "limit", _, _] => &["--persist"],
        ["target"] => &["list", "set", "reset"],
        ["niri"] => &["status", "install", "on", "off", "source", "set", "keybind"],
        ["niri", "set"] => &[
            "scale",
            "layout",
            "workspace_gap",
            "blur",
            "gesture_distance",
        ],
        ["nova"] => &["show", "set", "keybind", "install", "uninstall", "launch"],
        ["hooks"] => &["install", "remove", "status"],
        ["update"] => &["--check", "--json"],
        ["completions"] => &["bash", "zsh", "fish"],
        _ => &[],
    };
    owned(s)
}

fn owned(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| x.to_string()).collect()
}

/// Every group a person types (hidden ones — `hook`, `__complete` — left out).
fn groups() -> Vec<String> {
    crate::cli()
        .get_subcommands()
        .filter(|c| !c.is_hide_set())
        .map(|c| c.get_name().to_string())
        .collect()
}

/// Installed theme slugs; nothing at all when Omarchy isn't found (a
/// completion must never print an error into the user's prompt).
fn theme_slugs() -> Vec<String> {
    use studio_core::modules::themes::ThemeStore;
    use studio_core::omarchy::OmarchyPaths;
    let Ok(paths) = OmarchyPaths::discover() else {
        return Vec::new();
    };
    ThemeStore::from_paths(&paths)
        .list()
        .into_iter()
        .map(|t| t.slug)
        .collect()
}

/// `__complete <words…>`: the last word is the one being completed (possibly
/// empty); print the candidates that extend it.
pub fn complete(words: &[&str]) -> i32 {
    let (current, prev) = match words.split_last() {
        Some((cur, prev)) => (*cur, prev),
        None => ("", &[][..]),
    };
    for c in candidates(prev) {
        if c.starts_with(current) {
            println!("{c}");
        }
    }
    0
}

const BASH: &str = r#"# omarchy-studio bash completion — asks the binary for candidates.
_omarchy_studio() {
    local IFS=$'\n'
    COMPREPLY=($(omarchy-studio __complete "${COMP_WORDS[@]:1:COMP_CWORD}" 2>/dev/null))
}
complete -o default -o nosort -F _omarchy_studio omarchy-studio
"#;

const ZSH: &str = r#"#compdef omarchy-studio
# omarchy-studio zsh completion — asks the binary for candidates.
_omarchy_studio() {
    local -a candidates
    candidates=("${(@f)$(omarchy-studio __complete "${(@)words[2,CURRENT]}" 2>/dev/null)}")
    if (( ${#candidates} )) && [[ -n ${candidates[1]} ]]; then
        compadd -Q -a candidates
    else
        _files
    fi
}
if [ "$funcstack[1]" = "_omarchy-studio" ]; then
    _omarchy_studio "$@"
else
    compdef _omarchy_studio omarchy-studio
fi
"#;

const FISH: &str = r#"# omarchy-studio fish completion — asks the binary for candidates.
function __omarchy_studio_complete
    set -l words (commandline -opc)
    omarchy-studio __complete $words[2..-1] (commandline -ct) 2>/dev/null
end
complete -c omarchy-studio -k -a '(__omarchy_studio_complete)'
"#;

/// `completions <shell>`: print the shim for that shell.
pub fn completions(args: &[&str]) -> i32 {
    match args {
        ["bash"] => print!("{BASH}"),
        ["zsh"] => print!("{ZSH}"),
        ["fish"] => print!("{FISH}"),
        _ => {
            eprintln!("usage: omarchy-studio completions bash|zsh|fish");
            eprintln!();
            eprintln!("  bash:  omarchy-studio completions bash > ~/.local/share/bash-completion/completions/omarchy-studio");
            eprintln!(
                "  zsh:   omarchy-studio completions zsh > \"${{fpath[1]}}/_omarchy-studio\""
            );
            eprintln!("  fish:  omarchy-studio completions fish > ~/.config/fish/completions/omarchy-studio.fish");
            return 2;
        }
    }
    0
}

/// Escape text for a roff body line: backslashes, and a leading `.` or `'`
/// that roff would read as a request.
fn roff(text: &str) -> String {
    let mut t = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => t.push_str("\\e"),
            '-' => t.push_str("\\-"),
            // Non-ASCII (—, →, …) as groff's \[uXXXX] so a plain-ASCII
            // `nroff` renders it instead of warning and dropping bytes.
            c if !c.is_ascii() => t.push_str(&format!("\\[u{:04X}]", c as u32)),
            c => t.push(c),
        }
    }
    if t.starts_with('.') || t.starts_with('\'') {
        format!("\\&{t}")
    } else {
        t
    }
}

/// `man`: a man page rendered from the same clap definition `--help` uses,
/// so the two can't drift. `omarchy-studio man | man -l -` to read it.
pub fn man_page(cmd: &Command) -> String {
    let mut out = String::new();
    let version = cmd.get_version().unwrap_or_default();
    out.push_str(&format!(
        ".TH OMARCHY\\-STUDIO 1 \"\" \"omarchy-studio {version}\" \"User Commands\"\n"
    ));
    out.push_str(".SH NAME\nomarchy\\-studio \\- ");
    out.push_str(&roff(
        &cmd.get_about().map(|a| a.to_string()).unwrap_or_default(),
    ));
    out.push_str("\n.SH SYNOPSIS\n.B omarchy\\-studio\n.br\n.B omarchy\\-studio\n.I command\n[\n.I args\n\\&...]\n");
    out.push_str(".SH DESCRIPTION\n");
    let long = cmd
        .get_long_about()
        .map(|a| a.to_string())
        .unwrap_or_default();
    for para in long.split("\n\n") {
        out.push_str(".PP\n");
        out.push_str(&roff(&para.replace('\n', " ")));
        out.push('\n');
    }
    out.push_str(".SH COMMANDS\n");
    for sub in cmd.get_subcommands().filter(|c| !c.is_hide_set()) {
        out.push_str(&format!(".TP\n.B {}\n", roff(sub.get_name())));
        if let Some(about) = sub.get_about() {
            out.push_str(&roff(&about.to_string()));
            out.push('\n');
        }
        if let Some(usage) = sub.get_after_help() {
            out.push_str(".RS\n.nf\n");
            for line in usage.to_string().lines() {
                let line = line.trim_start_matches("usage:").trim();
                if !line.is_empty() {
                    out.push_str(&roff(line));
                    out.push('\n');
                }
            }
            out.push_str(".fi\n.RE\n");
        }
    }
    out.push_str(
        ".SH FILES\n\
         .TP\n.I ~/.config/omarchy\\-studio/config.toml\nStudio's own settings (update checks, config targets).\n\
         .TP\n.I ~/.local/state/omarchy\\-studio/history\nThe snapshot store: a git repository holding every change Studio made.\n\
         .SH SEE ALSO\n\
         .BR hyprctl (1),\n.BR git (1)\n.PP\nhttps://github.com/arino08/omarchy\\-studio\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_to(words: &[&str]) -> Vec<String> {
        let (cur, prev) = words.split_last().unwrap();
        candidates(prev)
            .into_iter()
            .filter(|c| c.starts_with(cur))
            .collect()
    }

    #[test]
    fn groups_complete_and_filter_by_prefix() {
        let all = complete_to(&[""]);
        assert!(all.contains(&"theme".to_string()));
        assert!(
            !all.contains(&"hook".to_string()),
            "hidden groups stay hidden"
        );
        assert!(!all.iter().any(|g| g.starts_with("__")));
        assert_eq!(complete_to(&["the"]), ["theme", "themesync"]);
    }

    #[test]
    fn every_group_with_verbs_completes_them() {
        // Groups that take no arguments at all.
        let bare = ["install-integration", "uninstall", "man"];
        for g in groups() {
            if bare.contains(&g.as_str()) {
                continue;
            }
            assert!(
                !candidates(&[g.as_str()]).is_empty(),
                "`{g} <TAB>` offers nothing — add it to complete::candidates"
            );
        }
    }

    #[test]
    fn catalog_values_and_value_flags_complete() {
        assert!(complete_to(&["tweak", ""]).contains(&"caps-escape".to_string()));
        assert_eq!(complete_to(&["tweak", "anything", ""]), ["on", "off"]);
        assert!(complete_to(&["looknfeel", "set", "decoration."])
            .iter()
            .all(|k| k.starts_with("decoration.")));
        assert_eq!(
            complete_to(&["theme", "new", "x", "--mode", ""]),
            ["normal", "muted", "material"]
        );
        // a path argument offers nothing, so the shell falls back to files
        assert!(complete_to(&["rice", "import", ""]).is_empty());
    }

    #[test]
    fn man_page_lists_every_visible_group_with_its_usage() {
        let page = man_page(&crate::cli());
        assert!(page.starts_with(".TH OMARCHY\\-STUDIO 1"));
        for g in groups() {
            assert!(
                page.contains(&format!(".B {}\n", roff(&g))),
                "{g} missing from the man page"
            );
        }
        assert!(page.contains("rice import"));
        assert!(!page.contains("Internal: run a Studio hook"));
    }
}
