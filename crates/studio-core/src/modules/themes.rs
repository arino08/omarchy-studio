//! Theme model & operations (spec 04 §1–2). Applying always shells to
//! `omarchy-theme-set`; Studio never reimplements the apply pipeline.

use std::path::{Path, PathBuf};

use crate::error::{Result, StudioError};
use crate::modules::color::Color;
use crate::omarchy::OmarchyPaths;

/// The canonical `colors.toml` keys (AGENTS.md: accent, background,
/// foreground, color0-15; plus cursor/selection pair).
pub const PALETTE_KEYS: [&str; 22] = [
    "accent",
    "cursor",
    "foreground",
    "background",
    "selection_foreground",
    "selection_background",
    "color0",
    "color1",
    "color2",
    "color3",
    "color4",
    "color5",
    "color6",
    "color7",
    "color8",
    "color9",
    "color10",
    "color11",
    "color12",
    "color13",
    "color14",
    "color15",
];

/// Lossless `colors.toml` document: comments, ordering, and unknown keys all
/// survive edits (spec 03 dialect table — toml_edit).
pub struct Palette {
    doc: toml_edit::DocumentMut,
}

impl Palette {
    pub fn parse(content: &str) -> Result<Self> {
        let doc =
            content
                .parse::<toml_edit::DocumentMut>()
                .map_err(|e| StudioError::ParseFailed {
                    file: PathBuf::from("colors.toml"),
                    line: e.span().map(|s| content[..s.start].lines().count()),
                    hint: format!("not valid TOML: {}", e.message()),
                })?;
        Ok(Self { doc })
    }

    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::parse(&content)
    }

    pub fn get(&self, key: &str) -> Option<Color> {
        self.doc
            .get(key)
            .and_then(|i| i.as_str())
            .and_then(|s| Color::parse(s).ok())
    }

    /// In-place update preserving the line's comments and spacing.
    pub fn set(&mut self, key: &str, color: &Color) {
        if let Some(v) = self.doc.get_mut(key).and_then(|i| i.as_value_mut()) {
            let decor = v.decor().clone();
            *v = toml_edit::Value::from(color.hex());
            *v.decor_mut() = decor;
        } else {
            self.doc[key] = toml_edit::value(color.hex());
        }
    }

    /// Canonical keys that are missing or unparsable — the palette editor's
    /// validation strip.
    pub fn problems(&self) -> Vec<String> {
        PALETTE_KEYS
            .iter()
            .filter(|k| self.get(k).is_none())
            .map(|k| k.to_string())
            .collect()
    }

    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        self.doc.to_string()
    }
}

/// Where a theme lives (spec 04 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeOrigin {
    /// `$OMARCHY_PATH/themes/<slug>` — read-only; editing forks it.
    System,
    /// `~/.config/omarchy/themes/<slug>` with a unique slug.
    User,
    /// User dir shadowing a system slug — per-file overlay semantics
    /// (user files win file-by-file, exactly like `omarchy-theme-set`).
    Overlay,
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub slug: String,
    /// Title-cased display name (`osaka-jade` → `Osaka Jade`).
    pub pretty: String,
    pub origin: ThemeOrigin,
    pub system_dir: Option<PathBuf>,
    pub user_dir: Option<PathBuf>,
}

impl Theme {
    /// Merged-view lookup: user file wins, system fills in (overlay model).
    pub fn file(&self, name: &str) -> Option<PathBuf> {
        for dir in [&self.user_dir, &self.system_dir].into_iter().flatten() {
            let p = dir.join(name);
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    /// A theme is light iff `light.mode` exists anywhere in the merged view.
    pub fn light(&self) -> bool {
        self.file("light.mode").is_some()
    }

    pub fn palette(&self) -> Result<Palette> {
        let path = self.file("colors.toml").ok_or_else(|| StudioError::ParseFailed {
            file: PathBuf::from(format!("{}/colors.toml", self.slug)),
            line: None,
            hint: "theme has no colors.toml — supply one, or an alacritty.toml Omarchy can convert".into(),
        })?;
        Palette::load(&path)
    }

    /// Where palette edits go. System themes are read-only, so editing one
    /// writes a `colors.toml` into a user overlay dir of the same slug.
    pub fn writable_colors_path(&self, user_themes_dir: &Path) -> PathBuf {
        match &self.user_dir {
            Some(d) => d.join("colors.toml"),
            None => user_themes_dir.join(&self.slug).join("colors.toml"),
        }
    }
}

pub struct ThemeStore {
    pub system_dir: PathBuf,
    pub user_dir: PathBuf,
}

impl ThemeStore {
    pub fn from_paths(paths: &OmarchyPaths) -> Self {
        Self {
            system_dir: paths.system.join("themes"),
            user_dir: paths.config.join("themes"),
        }
    }

    /// Union of system + user themes, deduped by slug, sorted. Same
    /// semantics as `omarchy-theme-list` + the Walker picker. Dot-prefixed
    /// dirs (the live-preview scratch, [`SCRATCH_SLUG`]) are hidden.
    pub fn list(&self) -> Vec<Theme> {
        let mut slugs: Vec<String> = Vec::new();
        for dir in [&self.system_dir, &self.user_dir] {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for e in entries.flatten() {
                    if e.path().is_dir() {
                        let slug = e.file_name().to_string_lossy().to_string();
                        if !slug.starts_with('.') && !slugs.contains(&slug) {
                            slugs.push(slug);
                        }
                    }
                }
            }
        }
        slugs.sort();
        slugs.into_iter().filter_map(|s| self.get(&s)).collect()
    }

    pub fn get(&self, slug: &str) -> Option<Theme> {
        let system = self.system_dir.join(slug);
        let user = self.user_dir.join(slug);
        let (system_dir, user_dir) = (
            system.is_dir().then_some(system),
            user.is_dir().then_some(user),
        );
        let origin = match (&system_dir, &user_dir) {
            (Some(_), Some(_)) => ThemeOrigin::Overlay,
            (Some(_), None) => ThemeOrigin::System,
            (None, Some(_)) => ThemeOrigin::User,
            (None, None) => return None,
        };
        Some(Theme {
            pretty: pretty_name(slug),
            slug: slug.to_string(),
            origin,
            system_dir,
            user_dir,
        })
    }

    /// Fork `src` into a new standalone user theme: the *merged* view is
    /// copied (system first, user overlay on top — mirrors the staging step
    /// of `omarchy-theme-set`).
    pub fn fork(&self, src: &Theme, new_slug: &str) -> Result<Theme> {
        let slug = slugify(new_slug);
        let dest = self.user_dir.join(&slug);
        if dest.exists() {
            return Err(StudioError::External {
                cmd: format!("fork {} → {}", src.slug, slug),
                detail: format!("a theme named `{slug}` already exists — pick another name"),
            });
        }
        for dir in [&src.system_dir, &src.user_dir].into_iter().flatten() {
            copy_dir_all(dir, &dest)?;
        }
        self.get(&slug).ok_or_else(|| StudioError::External {
            cmd: "fork".into(),
            detail: "fork copied nothing — source theme is empty".into(),
        })
    }

    /// Share one theme: `out` becomes a `.tar.gz` holding `<slug>/` with the
    /// theme's *merged* view (system first, user overlay on top, like
    /// [`fork`](Self::fork)), so an overlay theme travels complete. Dot
    /// entries (a community theme's whole `.git`) stay behind and links are
    /// followed, so the archive holds plain files only. Extract it into
    /// `~/.config/omarchy/themes/` to install.
    pub fn export(
        &self,
        theme: &Theme,
        out: &Path,
        runner: &dyn crate::cmd::CommandRunner,
    ) -> Result<()> {
        let parent = out
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let stage = parent.join(format!(".theme-export-{}-{nanos}", std::process::id()));
        let result = (|| {
            for dir in [&theme.system_dir, &theme.user_dir].into_iter().flatten() {
                copy_plain(dir, &stage.join(&theme.slug))?;
            }
            let tar = crate::cmd::Cmd::new("tar")
                .arg("-czf")
                .arg(out.display().to_string())
                .arg("-C")
                .arg(stage.display().to_string())
                .arg(&theme.slug);
            let res = runner.run(&tar)?;
            if !res.ok() {
                return Err(StudioError::External {
                    cmd: "tar -czf".into(),
                    detail: res.stderr.trim().to_string(),
                });
            }
            Ok(())
        })();
        let _ = std::fs::remove_dir_all(&stage);
        result
    }

    fn scratch_dir(&self) -> PathBuf {
        self.user_dir.join(SCRATCH_SLUG)
    }

    /// Live-preview `palette` on the desktop without disturbing the real theme
    /// (spec 04 §2). Writes a minimal scratch theme (just `colors.toml`, plus a
    /// `light.mode` marker when `light`) and applies it with the wallpaper
    /// pinned. Templates regenerate every app's colors from `colors.toml`, so
    /// this is enough for an accurate color preview and stays cheap enough to
    /// run on each edit.
    pub fn preview(
        &self,
        palette: &Palette,
        light: bool,
        runner: &dyn crate::cmd::CommandRunner,
    ) -> Result<()> {
        let dir = self.scratch_dir();
        std::fs::create_dir_all(&dir)?;
        crate::configfs::atomic_write(&dir.join("colors.toml"), &palette.to_string())?;
        let light_marker = dir.join("light.mode");
        if light {
            if !light_marker.exists() {
                std::fs::write(&light_marker, "# omarchy-studio live preview\n")?;
            }
        } else if light_marker.exists() {
            std::fs::remove_file(&light_marker)?;
        }
        let out = runner.run(&crate::omarchy::cmds::theme_set_keep_bg(SCRATCH_SLUG))?;
        if !out.ok() {
            return Err(StudioError::External {
                cmd: format!("omarchy-theme-set {SCRATCH_SLUG}"),
                detail: out.stderr.trim().to_string(),
            });
        }
        Ok(())
    }

    /// Remove the scratch preview theme. Call when leaving the editor.
    pub fn cleanup_preview(&self) {
        let _ = std::fs::remove_dir_all(self.scratch_dir());
    }
}

/// Hidden slug used for the live-preview scratch theme.
pub const SCRATCH_SLUG: &str = ".studio-preview";

/// `"Tokyo Night"` → `tokyo-night` (same normalization as omarchy-theme-set).
pub fn slugify(name: &str) -> String {
    name.trim().to_lowercase().replace(char::is_whitespace, "-")
}

fn pretty_name(slug: &str) -> String {
    slug.split('-')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Recursive copy for sharing: skips dot entries, follows links (so the
/// copy holds plain files only), later sources overwrite earlier ones.
fn copy_plain(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        if from.is_dir() {
            copy_plain(&from, &to)?;
        } else if from.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::{CommandRunner, StubRunner};
    use crate::omarchy::cmds;

    const SAMPLE: &str = "\
# Tokyo Night palette
accent = \"#7aa2f7\" # the blue
background = \"#1a1b26\"
foreground = \"#c0caf5\"
";

    #[test]
    fn palette_round_trips_byte_identical() {
        let p = Palette::parse(SAMPLE).unwrap();
        assert_eq!(p.to_string(), SAMPLE);
    }

    #[test]
    fn palette_set_preserves_comments_and_layout() {
        let mut p = Palette::parse(SAMPLE).unwrap();
        p.set("accent", &Color::parse("#ff0000").unwrap());
        let out = p.to_string();
        assert!(out.contains("# Tokyo Night palette"), "header comment kept");
        assert!(
            out.contains("accent = \"#ff0000\" # the blue"),
            "inline comment kept: {out}"
        );
        assert_eq!(p.get("accent").unwrap().hex(), "#ff0000");
    }

    #[test]
    fn palette_reports_missing_keys_not_crashes() {
        let p = Palette::parse(SAMPLE).unwrap();
        let problems = p.problems();
        assert!(problems.contains(&"color0".to_string()));
        assert!(!problems.contains(&"accent".to_string()));
    }

    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let dir = std::env::temp_dir().join(format!(
            "omarchy-studio-themes-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// system: tokyo-night (dark), snow (light). user: custom, tokyo-night overlay.
    fn fake_store(tag: &str) -> ThemeStore {
        let root = scratch(tag);
        let store = ThemeStore {
            system_dir: root.join("system/themes"),
            user_dir: root.join("user/themes"),
        };
        let tn = store.system_dir.join("tokyo-night");
        std::fs::create_dir_all(&tn).unwrap();
        std::fs::write(tn.join("colors.toml"), SAMPLE).unwrap();
        std::fs::write(tn.join("icons.theme"), "Yaru-blue\n").unwrap();

        let snow = store.system_dir.join("snow");
        std::fs::create_dir_all(&snow).unwrap();
        std::fs::write(snow.join("colors.toml"), "accent = \"#111111\"\n").unwrap();
        std::fs::write(snow.join("light.mode"), "").unwrap();

        let custom = store.user_dir.join("custom");
        std::fs::create_dir_all(&custom).unwrap();
        std::fs::write(custom.join("colors.toml"), "accent = \"#222222\"\n").unwrap();

        let overlay = store.user_dir.join("tokyo-night");
        std::fs::create_dir_all(&overlay).unwrap();
        std::fs::write(overlay.join("colors.toml"), "accent = \"#00ff99\"\n").unwrap();

        store
    }

    #[test]
    fn lists_union_with_correct_origins() {
        let store = fake_store("list");
        let themes = store.list();
        let by_slug: Vec<(&str, ThemeOrigin)> =
            themes.iter().map(|t| (t.slug.as_str(), t.origin)).collect();
        assert_eq!(
            by_slug,
            vec![
                ("custom", ThemeOrigin::User),
                ("snow", ThemeOrigin::System),
                ("tokyo-night", ThemeOrigin::Overlay),
            ]
        );
        assert_eq!(themes[2].pretty, "Tokyo Night");
        assert!(themes[1].light());
        assert!(!themes[2].light());
    }

    #[test]
    fn merged_view_user_file_wins() {
        let store = fake_store("merge");
        let tn = store.get("tokyo-night").unwrap();
        // colors.toml comes from the overlay…
        assert_eq!(
            tn.palette().unwrap().get("accent").unwrap().hex(),
            "#00ff99"
        );
        // …while icons.theme falls through to the system theme.
        assert!(tn
            .file("icons.theme")
            .unwrap()
            .starts_with(&store.system_dir));
    }

    #[test]
    fn fork_copies_merged_view_and_refuses_collisions() {
        let store = fake_store("fork");
        let tn = store.get("tokyo-night").unwrap();
        let forked = store.fork(&tn, "My Night").unwrap();

        assert_eq!(forked.slug, "my-night");
        assert_eq!(forked.origin, ThemeOrigin::User);
        // overlay accent won the merge; system icons.theme came along
        assert_eq!(
            forked.palette().unwrap().get("accent").unwrap().hex(),
            "#00ff99"
        );
        assert!(forked.file("icons.theme").is_some());

        let err = store.fork(&tn, "custom").unwrap_err();
        match err {
            StudioError::External { detail, .. } => assert!(detail.contains("already exists")),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn export_archives_the_merged_view_without_dotfiles() {
        use crate::cmd::RealRunner;
        let store = fake_store("export");
        let tn = store.get("tokyo-night").unwrap();
        std::fs::create_dir_all(tn.user_dir.as_ref().unwrap().join(".git")).unwrap();
        std::fs::write(tn.user_dir.as_ref().unwrap().join(".git/HEAD"), "ref").unwrap();
        let out = store
            .user_dir
            .parent()
            .unwrap()
            .join("out/tokyo-night.tar.gz");
        store.export(&tn, &out, &RealRunner).unwrap();

        let listing = RealRunner
            .run(
                &crate::cmd::Cmd::new("tar")
                    .arg("-tzf")
                    .arg(out.display().to_string()),
            )
            .unwrap()
            .stdout;
        assert!(listing.contains("tokyo-night/colors.toml"), "{listing}");
        assert!(
            listing.contains("tokyo-night/icons.theme"),
            "system file came along"
        );
        assert!(!listing.contains(".git"), "dot entries stay behind");

        // the overlay's colors won the merge
        let x = out.parent().unwrap().join("x");
        std::fs::create_dir_all(&x).unwrap();
        RealRunner
            .run(
                &crate::cmd::Cmd::new("tar")
                    .arg("-xzf")
                    .arg(out.display().to_string())
                    .arg("-C")
                    .arg(x.display().to_string()),
            )
            .unwrap();
        let colors = std::fs::read_to_string(x.join("tokyo-night/colors.toml")).unwrap();
        assert!(colors.contains("#00ff99"), "{colors}");
        // no staging litter beside the archive
        let litter: Vec<_> = std::fs::read_dir(out.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".theme-export"))
            .collect();
        assert!(litter.is_empty());
    }

    #[test]
    fn system_theme_edits_route_to_user_overlay() {
        let store = fake_store("route");
        let snow = store.get("snow").unwrap();
        assert_eq!(
            snow.writable_colors_path(&store.user_dir),
            store.user_dir.join("snow/colors.toml")
        );
        let tn = store.get("tokyo-night").unwrap();
        assert_eq!(
            tn.writable_colors_path(&store.user_dir),
            store.user_dir.join("tokyo-night/colors.toml")
        );
    }

    #[test]
    fn apply_shells_to_omarchy_theme_set() {
        let stub = StubRunner::default().with_ok("omarchy-theme-set tokyo-night", "");
        stub.run(&cmds::theme_set("tokyo-night")).unwrap();
        assert_eq!(stub.calls(), vec!["omarchy-theme-set tokyo-night"]);
    }

    #[test]
    fn preview_writes_scratch_and_applies_it() {
        let store = fake_store("preview");
        let mut p = store.get("tokyo-night").unwrap().palette().unwrap();
        p.set("accent", &Color::parse("#abcdef").unwrap());

        let stub = StubRunner::default().with_ok(format!("omarchy-theme-set {SCRATCH_SLUG}"), "");
        store.preview(&p, false, &stub).unwrap();

        // scratch colors.toml carries the edited value…
        let scratch = store.user_dir.join(SCRATCH_SLUG).join("colors.toml");
        assert!(std::fs::read_to_string(&scratch)
            .unwrap()
            .contains("#abcdef"));
        // …and we applied it with the wallpaper pinned.
        assert_eq!(
            stub.calls(),
            vec![format!("omarchy-theme-set {SCRATCH_SLUG}")]
        );
        // scratch theme is hidden from the browser…
        assert!(!store.list().iter().any(|t| t.slug == SCRATCH_SLUG));
        // …and cleanup removes it.
        store.cleanup_preview();
        assert!(!store.user_dir.join(SCRATCH_SLUG).exists());
    }

    #[test]
    fn preview_toggles_light_marker() {
        let store = fake_store("light-preview");
        let p = store.get("snow").unwrap().palette().unwrap();
        let stub = StubRunner::default().with_ok(format!("omarchy-theme-set {SCRATCH_SLUG}"), "");

        store.preview(&p, true, &stub).unwrap();
        assert!(store
            .user_dir
            .join(SCRATCH_SLUG)
            .join("light.mode")
            .exists());
        store.preview(&p, false, &stub).unwrap();
        assert!(!store
            .user_dir
            .join(SCRATCH_SLUG)
            .join("light.mode")
            .exists());
    }
}
