//! Theme coverage (spec 04 §3, roadmap 1.0.3): does the active theme actually
//! cover every app Omarchy themes?
//!
//! Omarchy renders most app configs from the palette — `default/themed/*.tpl`
//! turns `colors.toml` into alacritty, waybar, mako and friends, so a theme
//! that ships nothing but colours still themes them. A handful of files have
//! **no template** and must be shipped verbatim by the theme, because no
//! colour file can imply them: `neovim.lua` names an editor colorscheme
//! *plugin*, `icons.theme` names an icon set.
//!
//! When one of those is missing, Omarchy's symlink into the theme dangles and
//! the app fails — `nvim` errors on every launch with a "cannot open
//! .../theme.lua" that says nothing about themes. Most community themes are
//! colours-only, so this is the common case, not the exotic one. Reporting it
//! is the whole point of this module.

use std::path::{Path, PathBuf};

use crate::omarchy::OmarchyPaths;

/// One file a complete theme is expected to provide.
pub struct Expected {
    /// File name inside the theme directory.
    pub file: &'static str,
    /// Who consumes it — named the way a person would say it.
    pub app: &'static str,
    /// What the user actually sees when it's missing.
    pub consequence: &'static str,
    /// Without this the theme is not usable at all.
    pub required: bool,
}

/// The files Omarchy's own themes ship, and what each one is for.
///
/// Derived from what all of Omarchy's stock themes have in common; anything
/// with a `.tpl` is deliberately absent here, since a template means a theme
/// never has to ship it.
pub fn catalog() -> &'static [Expected] {
    &[
        Expected {
            file: "colors.toml",
            app: "everything",
            consequence: "no palette — Omarchy can't render any themed config",
            required: true,
        },
        Expected {
            file: "neovim.lua",
            app: "Neovim",
            consequence: "nvim errors on every launch (Omarchy symlinks this file)",
            required: false,
        },
        Expected {
            file: "vscode.json",
            app: "VS Code",
            consequence: "the editor keeps its previous colours",
            required: false,
        },
        Expected {
            file: "icons.theme",
            app: "icons",
            consequence: "icons stay on the previous theme's set",
            required: false,
        },
        Expected {
            file: "backgrounds",
            app: "wallpapers",
            consequence: "no wallpapers ship with the theme",
            required: false,
        },
        Expected {
            file: "preview.png",
            app: "theme picker",
            consequence: "no thumbnail in the theme browser",
            required: false,
        },
        Expected {
            file: "unlock.png",
            app: "lock screen",
            consequence: "hyprlock falls back to its default art",
            required: false,
        },
    ]
}

/// How one expected file is being satisfied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The theme ships the file itself. Beats a template when both exist.
    Shipped,
    /// The theme doesn't ship it, but Omarchy renders it from the palette.
    Templated,
    /// Nothing provides it — this is what breaks.
    Missing,
}

pub struct Row {
    pub file: &'static str,
    pub app: &'static str,
    pub consequence: &'static str,
    pub required: bool,
    pub status: Status,
}

impl Row {
    /// A gap the user will actually notice.
    pub fn is_breakage(&self) -> bool {
        self.status == Status::Missing
    }
}

/// True when Omarchy (or the user) has a template that renders `file`, so a
/// theme doesn't need to ship it.
fn templated(paths: &OmarchyPaths, file: &str) -> bool {
    let tpl = format!("{file}.tpl");
    [paths.user_templates(), paths.default_templates()]
        .iter()
        .any(|dir| dir.join(&tpl).exists())
}

/// Report how each expected file is satisfied for the theme in `theme_dir`.
pub fn report(paths: &OmarchyPaths, theme_dir: &Path) -> Vec<Row> {
    catalog()
        .iter()
        .map(|e| {
            // `exists()` follows symlinks, which is what we want: a dangling
            // link into a deleted theme is as missing as no file at all.
            let status = if theme_dir.join(e.file).exists() {
                Status::Shipped
            } else if templated(paths, e.file) {
                Status::Templated
            } else {
                Status::Missing
            };
            Row {
                file: e.file,
                app: e.app,
                consequence: e.consequence,
                required: e.required,
                status,
            }
        })
        .collect()
}

/// Resolve a theme slug to its directory, honouring Omarchy's overlay order
/// (the user's own themes win over the stock ones of the same name).
pub fn theme_dir(paths: &OmarchyPaths, slug: &str) -> Option<PathBuf> {
    paths
        .theme_dirs()
        .into_iter()
        .rev()
        .map(|d| d.join(slug))
        .find(|d| d.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh unique temp dir (repo convention — no tempfile dev-dep).
    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "omarchy-studio-coverage-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A fixture Omarchy with `templates` present as `<name>.tpl`.
    fn paths(root: &Path, templates: &[&str]) -> OmarchyPaths {
        let system = root.join("sys/omarchy");
        let config = root.join("config/omarchy");
        std::fs::create_dir_all(system.join("default/themed")).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        for t in templates {
            std::fs::write(system.join("default/themed").join(format!("{t}.tpl")), "x").unwrap();
        }
        let state = root.join("state/omarchy");
        std::fs::create_dir_all(&state).unwrap();
        OmarchyPaths {
            system,
            config,
            state,
        }
    }

    fn theme(root: &Path, slug: &str, files: &[&str]) -> PathBuf {
        let dir = root.join("config/omarchy/themes").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        for f in files {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        dir
    }

    fn row<'a>(rows: &'a [Row], file: &str) -> &'a Row {
        rows.iter().find(|r| r.file == file).expect("row exists")
    }

    #[test]
    fn a_shipped_file_is_shipped_even_when_a_template_also_exists() {
        let tmp = tmpdir("t");
        let p = paths(&tmp, &["neovim.lua"]);
        let dir = theme(&tmp, "full", &["neovim.lua"]);
        // Verbatim beats the template (spec 04 §3).
        assert_eq!(row(&report(&p, &dir), "neovim.lua").status, Status::Shipped);
    }

    #[test]
    fn a_missing_but_templated_file_is_not_a_breakage() {
        let tmp = tmpdir("t");
        let p = paths(&tmp, &["neovim.lua"]);
        let dir = theme(&tmp, "colors-only", &["colors.toml"]);
        let rows = report(&p, &dir);
        assert_eq!(row(&rows, "neovim.lua").status, Status::Templated);
        assert!(!row(&rows, "neovim.lua").is_breakage());
    }

    #[test]
    fn a_missing_untemplated_file_is_a_breakage() {
        let tmp = tmpdir("t");
        // No templates at all — exactly a stock Omarchy, which has no
        // neovim.lua.tpl, which is why this is the real-world case.
        let p = paths(&tmp, &[]);
        let dir = theme(&tmp, "colors-only", &["colors.toml"]);
        let rows = report(&p, &dir);
        assert_eq!(row(&rows, "neovim.lua").status, Status::Missing);
        assert!(row(&rows, "neovim.lua").is_breakage());
        assert_eq!(row(&rows, "colors.toml").status, Status::Shipped);
    }

    #[test]
    fn a_colours_only_community_theme_reports_every_verbatim_file() {
        let tmp = tmpdir("t");
        let p = paths(&tmp, &[]);
        let dir = theme(&tmp, "community", &["colors.toml", "preview.png"]);
        let broken: Vec<_> = report(&p, &dir)
            .into_iter()
            .filter(Row::is_breakage)
            .map(|r| r.file)
            .collect();
        assert!(broken.contains(&"neovim.lua"), "{broken:?}");
        assert!(broken.contains(&"icons.theme"), "{broken:?}");
        assert!(!broken.contains(&"preview.png"), "{broken:?}");
    }

    #[test]
    fn a_missing_palette_is_the_only_required_gap() {
        let tmp = tmpdir("t");
        let p = paths(&tmp, &[]);
        let dir = theme(&tmp, "empty", &[]);
        let rows = report(&p, &dir);
        let required: Vec<_> = rows
            .iter()
            .filter(|r| r.required && r.is_breakage())
            .map(|r| r.file)
            .collect();
        assert_eq!(required, vec!["colors.toml"]);
    }

    #[test]
    fn a_user_template_also_counts_as_covered() {
        let tmp = tmpdir("t");
        let p = paths(&tmp, &[]);
        std::fs::create_dir_all(p.user_templates()).unwrap();
        std::fs::write(p.user_templates().join("neovim.lua.tpl"), "x").unwrap();
        let dir = theme(&tmp, "colors-only", &["colors.toml"]);
        assert_eq!(
            row(&report(&p, &dir), "neovim.lua").status,
            Status::Templated
        );
    }

    #[test]
    fn theme_dir_prefers_the_users_overlay_over_the_stock_theme() {
        let tmp = tmpdir("t");
        let p = paths(&tmp, &[]);
        std::fs::create_dir_all(p.system.join("themes/nord")).unwrap();
        let user = theme(&tmp, "nord", &["colors.toml"]);
        assert_eq!(theme_dir(&p, "nord").unwrap(), user);
        assert!(theme_dir(&p, "nope").is_none());
    }
}
