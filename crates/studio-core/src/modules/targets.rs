//! Custom config targets (roadmap 0.8.2, community ask).
//!
//! Every module resolves its file(s) from fixed Omarchy-convention paths. Users
//! with a relocated or hand-rolled config — a Waybar launched with
//! `waybar -c ~/.config/waybar/mybar.jsonc`, several bars, or configs kept
//! outside the Omarchy tree — need Studio to edit *their* file, not the
//! convention default. The editing machinery (JSONC editor, `ManagedBlock`,
//! the hyprlang CST) is already path-agnostic; only resolution was hardcoded.
//!
//! Resolution has three tiers, highest precedence first:
//!
//! 1. **Override** — an explicit path in the `[targets]` table of Studio's own
//!    `config.toml`, set via the `target` CLI verb and validated on write.
//! 2. **Detected** — read from a running process (Waybar's `-c`/`-s` flags).
//! 3. **Default** — the Omarchy-convention path (unchanged for stock setups —
//!    zero config for the common case).
//!
//! The snapshot store keys on absolute paths, so a custom target gets the same
//! pre/post snapshots, drift detection and rollback as a default one; managed
//! blocks are relocated by marker, not by path assumption.

use std::path::{Path, PathBuf};

use crate::error::{Result, StudioError};
use crate::expand_tilde;
use crate::omarchy::OmarchyPaths;

/// Where a resolved target path came from (precedence order, highest first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// An explicit `[targets]` entry in Studio's config.
    Override,
    /// Read from a running process (Waybar `-c`/`-s`).
    Detected,
    /// The Omarchy-convention default.
    Default,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Override => "override",
            Source::Detected => "detected",
            Source::Default => "default",
        }
    }
}

/// A resolved config target: which file a module reads/writes, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub path: PathBuf,
    pub source: Source,
}

/// Pick the winning path by precedence: override > detected > default.
pub fn resolve(over: Option<PathBuf>, detected: Option<PathBuf>, default: PathBuf) -> Resolved {
    if let Some(path) = over {
        Resolved {
            path,
            source: Source::Override,
        }
    } else if let Some(path) = detected {
        Resolved {
            path,
            source: Source::Detected,
        }
    } else {
        Resolved {
            path: default,
            source: Source::Default,
        }
    }
}

// ------------------------------------------------------------------ registry

/// A config target Studio knows how to resolve for the user.
pub struct Spec {
    /// `module.file` key used in `[targets]` and CLI verbs.
    pub key: &'static str,
    /// Human label for listings.
    pub label: &'static str,
    /// Whether Studio can auto-detect this from a running process.
    pub detectable: bool,
}

/// The targets Studio resolves today. Hypr files stay convention-bound (their
/// sourcing path is fixed by Hyprland itself), so only Waybar is relocatable
/// for now; new file writers in later milestones register their keys here.
pub const KNOWN: &[Spec] = &[
    Spec {
        key: "waybar.config",
        label: "Waybar config",
        detectable: true,
    },
    Spec {
        key: "waybar.style",
        label: "Waybar style",
        detectable: true,
    },
];

/// Omarchy-convention default path for a known target key.
pub fn default_for(key: &str, paths: &OmarchyPaths) -> Option<PathBuf> {
    match key {
        "waybar.config" => Some(super::waybar::config_path(paths)),
        "waybar.style" => Some(super::waybar::style_path(paths)),
        _ => None,
    }
}

fn detect_key(key: &str, instances: &[WaybarInstance]) -> Option<PathBuf> {
    match key {
        "waybar.config" => instances.iter().find_map(|w| w.config.clone()),
        "waybar.style" => instances.iter().find_map(|w| w.style.clone()),
        _ => None,
    }
}

/// Resolve every known target end to end, scanning running processes once.
/// Powers `target list` and the Doctor row.
pub fn resolve_all(paths: &OmarchyPaths, overrides: &Overrides) -> Vec<(&'static Spec, Resolved)> {
    let instances = waybar_instances();
    KNOWN
        .iter()
        .filter_map(|spec| {
            let default = default_for(spec.key, paths)?;
            let detected = if spec.detectable {
                detect_key(spec.key, &instances)
            } else {
                None
            };
            Some((spec, resolve(overrides.get(spec.key), detected, default)))
        })
        .collect()
}

/// Resolve Waybar's config path: override > running `-c` > Omarchy default.
pub fn resolve_waybar_config(
    default: PathBuf,
    overrides: &Overrides,
    instances: &[WaybarInstance],
) -> Resolved {
    resolve(
        overrides.get("waybar.config"),
        instances.iter().find_map(|w| w.config.clone()),
        default,
    )
}

/// Resolve Waybar's style path: override > running `-s` > Omarchy default.
pub fn resolve_waybar_style(
    default: PathBuf,
    overrides: &Overrides,
    instances: &[WaybarInstance],
) -> Resolved {
    resolve(
        overrides.get("waybar.style"),
        instances.iter().find_map(|w| w.style.clone()),
        default,
    )
}

// ------------------------------------------------------------------ overrides

const TABLE: &str = "targets";

/// User path overrides, backed by the `[targets]` table of Studio's
/// `config.toml`. Keys are `module.file` (e.g. `waybar.config`), stored as
/// nested tables: `[targets.waybar]` with a `config` entry.
pub struct Overrides {
    path: PathBuf,
    doc: toml_edit::DocumentMut,
}

impl Overrides {
    /// Load from Studio's `config.toml`. Absent or unparsable → empty (never an
    /// error — a garbled config must not block resolution).
    pub fn load(config_toml: &Path) -> Self {
        let doc = std::fs::read_to_string(config_toml)
            .ok()
            .and_then(|t| t.parse::<toml_edit::DocumentMut>().ok())
            .unwrap_or_default();
        Self {
            path: config_toml.to_path_buf(),
            doc,
        }
    }

    /// The override path for `key`, tilde-expanded, or `None` if unset.
    pub fn get(&self, key: &str) -> Option<PathBuf> {
        let (module, file) = key.split_once('.')?;
        let raw = self.doc.get(TABLE)?.get(module)?.get(file)?.as_str()?;
        Some(expand_tilde(raw))
    }

    /// Set `key`'s override to `path` in memory. Call [`Overrides::save`] to
    /// persist. Does not validate — see [`validate`].
    pub fn set(&mut self, key: &str, path: &str) -> Result<()> {
        let (module, file) = split_key(key)?;
        let targets = self
            .doc
            .entry(TABLE)
            .or_insert(toml_edit::table())
            .as_table_mut()
            .ok_or_else(|| bad_table(TABLE))?;
        // Render only `[targets.waybar]`, not a bare `[targets]` header above it.
        targets.set_implicit(true);
        let m = targets
            .entry(module)
            .or_insert(toml_edit::table())
            .as_table_mut()
            .ok_or_else(|| bad_table(module))?;
        m[file] = toml_edit::value(path);
        Ok(())
    }

    /// Remove `key`'s override, pruning an emptied module table. Returns whether
    /// anything was removed.
    pub fn reset(&mut self, key: &str) -> Result<bool> {
        let (module, file) = split_key(key)?;
        let Some(targets) = self.doc.get_mut(TABLE).and_then(|t| t.as_table_mut()) else {
            return Ok(false);
        };
        let removed = targets
            .get_mut(module)
            .and_then(|m| m.as_table_mut())
            .map(|m| m.remove(file).is_some())
            .unwrap_or(false);
        if targets
            .get(module)
            .and_then(|m| m.as_table())
            .is_some_and(toml_edit::Table::is_empty)
        {
            targets.remove(module);
        }
        if targets.is_empty() {
            self.doc.remove(TABLE);
        }
        Ok(removed)
    }

    /// Every override currently set, as `(key, tilde-expanded path)`.
    pub fn entries(&self) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        if let Some(t) = self.doc.get(TABLE).and_then(|t| t.as_table()) {
            for (module, item) in t.iter() {
                if let Some(m) = item.as_table() {
                    for (file, v) in m.iter() {
                        if let Some(s) = v.as_str() {
                            out.push((format!("{module}.{file}"), expand_tilde(s)));
                        }
                    }
                }
            }
        }
        out
    }

    /// Persist to `config.toml`, creating the parent directory if needed.
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, self.doc.to_string())?;
        Ok(())
    }
}

fn split_key(key: &str) -> Result<(&str, &str)> {
    key.split_once('.').ok_or_else(|| StudioError::External {
        cmd: "target".into(),
        detail: format!("target key must be module.file, got `{key}`"),
    })
}

fn bad_table(name: &str) -> StudioError {
    StudioError::External {
        cmd: "target".into(),
        detail: format!("`{name}` in config.toml is not a table — fix it by hand"),
    }
}

/// Validate a candidate override path for `key` before persisting it. The file
/// must exist and, for formats we can read, parse — so a typo fails loudly now
/// instead of at edit time.
pub fn validate(key: &str, path: &Path) -> Result<()> {
    if !path.is_file() {
        return Err(StudioError::External {
            cmd: format!("target set {key}"),
            detail: format!(
                "no file at {} — create it first, or check the path",
                path.display()
            ),
        });
    }
    if key == "waybar.config" {
        let src = std::fs::read_to_string(path)?;
        crate::configfs::jsonc::JsoncDoc::parse(&src).map_err(|hint| StudioError::ParseFailed {
            file: path.to_path_buf(),
            line: None,
            hint,
        })?;
    }
    Ok(())
}

// ------------------------------------------------------------------ detection

/// A running Waybar process and the config/style paths it was launched with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaybarInstance {
    pub pid: u32,
    pub config: Option<PathBuf>,
    pub style: Option<PathBuf>,
}

/// Scan `/proc` for running Waybar processes and the `-c`/`-s` paths they were
/// launched with. Empty when Waybar isn't running or `/proc` is unreadable.
pub fn waybar_instances() -> Vec<WaybarInstance> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let comm = std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
        if comm.trim() != "waybar" {
            continue;
        }
        let raw = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
        let args: Vec<String> = raw
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        let (config, style) = parse_waybar_cmdline(&args);
        out.push(WaybarInstance { pid, config, style });
    }
    out
}

/// Extract `-c`/`--config` and `-s`/`--style` paths from a Waybar argv.
/// Handles the separate (`-c PATH`), joined (`--config=PATH`, `-cPATH`) forms;
/// tilde-expands the result.
pub(crate) fn parse_waybar_cmdline(args: &[String]) -> (Option<PathBuf>, Option<PathBuf>) {
    let mut config = None;
    let mut style = None;
    let mut i = 0;
    while i < args.len() {
        if let Some(v) = flag_arg(args, &mut i, "-c", "--config") {
            config = Some(expand_tilde(&v));
        } else if let Some(v) = flag_arg(args, &mut i, "-s", "--style") {
            style = Some(expand_tilde(&v));
        }
        i += 1;
    }
    (config, style)
}

/// If `args[i]` is `short`/`long`, return its value (advancing `i` past a
/// separate value token). Recognises `--long=VALUE` and `-cVALUE` too.
fn flag_arg(args: &[String], i: &mut usize, short: &str, long: &str) -> Option<String> {
    let a = &args[*i];
    if let Some(rest) = a.strip_prefix(long).and_then(|r| r.strip_prefix('=')) {
        return Some(rest.to_string());
    }
    if a == long || a == short {
        if let Some(next) = args.get(*i + 1) {
            *i += 1;
            return Some(next.clone());
        }
        return None;
    }
    if let Some(rest) = a.strip_prefix(short) {
        let rest = rest.strip_prefix('=').unwrap_or(rest);
        if !rest.is_empty() {
            return Some(rest.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> String {
        v.to_string()
    }

    /// A fresh unique temp dir (repo convention — no tempfile dev-dep).
    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "omarchy-studio-targets-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolve_precedence_override_beats_detected_beats_default() {
        let over = resolve(
            Some(PathBuf::from("/over")),
            Some(PathBuf::from("/det")),
            PathBuf::from("/def"),
        );
        assert_eq!(over.source, Source::Override);
        assert_eq!(over.path, PathBuf::from("/over"));

        let det = resolve(None, Some(PathBuf::from("/det")), PathBuf::from("/def"));
        assert_eq!(det.source, Source::Detected);
        assert_eq!(det.path, PathBuf::from("/det"));

        let def = resolve(None, None, PathBuf::from("/def"));
        assert_eq!(def.source, Source::Default);
        assert_eq!(def.path, PathBuf::from("/def"));
    }

    #[test]
    fn cmdline_separate_flags() {
        let args = [
            s("waybar"),
            s("-c"),
            s("/a/config.jsonc"),
            s("-s"),
            s("/a/style.css"),
        ];
        let (c, st) = parse_waybar_cmdline(&args);
        assert_eq!(c, Some(PathBuf::from("/a/config.jsonc")));
        assert_eq!(st, Some(PathBuf::from("/a/style.css")));
    }

    #[test]
    fn cmdline_long_and_joined_forms() {
        let args = [
            s("waybar"),
            s("--config=/b/bar.jsonc"),
            s("--style"),
            s("/b/bar.css"),
        ];
        let (c, st) = parse_waybar_cmdline(&args);
        assert_eq!(c, Some(PathBuf::from("/b/bar.jsonc")));
        assert_eq!(st, Some(PathBuf::from("/b/bar.css")));
    }

    #[test]
    fn cmdline_absent_flags_yield_none() {
        let args = [s("waybar"), s("-b"), s("DP-1")];
        let (c, st) = parse_waybar_cmdline(&args);
        assert!(c.is_none());
        assert!(st.is_none());
    }

    #[test]
    fn cmdline_tilde_expands() {
        std::env::set_var("HOME", "/home/tester");
        let args = [s("waybar"), s("-c"), s("~/.config/waybar/x.jsonc")];
        let (c, _) = parse_waybar_cmdline(&args);
        assert_eq!(
            c,
            Some(PathBuf::from("/home/tester/.config/waybar/x.jsonc"))
        );
    }

    #[test]
    fn overrides_roundtrip_through_toml() {
        let dir = tmpdir("roundtrip");
        let cfg = dir.join("config.toml");

        let mut o = Overrides::load(&cfg);
        std::env::set_var("HOME", "/home/tester");
        o.set("waybar.config", "~/.config/waybar/mybar.jsonc")
            .unwrap();
        o.set("waybar.style", "/abs/style.css").unwrap();
        o.save().unwrap();

        let reloaded = Overrides::load(&cfg);
        assert_eq!(
            reloaded.get("waybar.config"),
            Some(PathBuf::from("/home/tester/.config/waybar/mybar.jsonc"))
        );
        assert_eq!(
            reloaded.get("waybar.style"),
            Some(PathBuf::from("/abs/style.css"))
        );
        assert_eq!(reloaded.entries().len(), 2);
    }

    #[test]
    fn reset_prunes_emptied_tables() {
        let dir = tmpdir("reset");
        let cfg = dir.join("config.toml");
        let mut o = Overrides::load(&cfg);
        o.set("waybar.config", "/a.jsonc").unwrap();
        assert!(o.reset("waybar.config").unwrap());
        assert!(o.get("waybar.config").is_none());
        assert!(o.entries().is_empty());
        // second reset is a no-op, not an error
        assert!(!o.reset("waybar.config").unwrap());
    }

    #[test]
    fn set_preserves_unrelated_config() {
        let dir = tmpdir("preserve");
        let cfg = dir.join("config.toml");
        std::fs::write(&cfg, "[update]\ncheck = false\n").unwrap();
        let mut o = Overrides::load(&cfg);
        o.set("waybar.config", "/a.jsonc").unwrap();
        o.save().unwrap();
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert!(text.contains("[update]"));
        assert!(text.contains("check = false"));
        assert!(text.contains("[targets.waybar]"));
    }

    #[test]
    fn split_key_rejects_flat_key() {
        assert!(split_key("waybar").is_err());
        assert_eq!(split_key("waybar.config").unwrap(), ("waybar", "config"));
    }

    #[test]
    fn validate_rejects_missing_file() {
        let err = validate("waybar.config", Path::new("/no/such/file.jsonc"));
        assert!(err.is_err());
    }

    #[test]
    fn validate_rejects_bad_jsonc() {
        let dir = tmpdir("badjsonc");
        let p = dir.join("bad.jsonc");
        std::fs::write(&p, "{ this is not json ").unwrap();
        assert!(validate("waybar.config", &p).is_err());
    }

    #[test]
    fn validate_accepts_good_jsonc() {
        let dir = tmpdir("okjsonc");
        let p = dir.join("ok.jsonc");
        std::fs::write(&p, "{\n  // a comment\n  \"layer\": \"top\"\n}\n").unwrap();
        assert!(validate("waybar.config", &p).is_ok());
    }
}
