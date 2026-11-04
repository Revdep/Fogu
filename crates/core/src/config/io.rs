//! Config file I/O: XDG path, atomic in-place writes, load with graceful fallback.

use std::path::{Path, PathBuf};

use super::Config;
use crate::background::{Background, Fit};

/// The config file's name inside [`config_dir`].
pub(super) const FILE_NAME: &str = "config.toml";

/// Something went wrong loading or saving the config file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A filesystem operation failed.
    #[error("{path}: {source}")]
    Io {
        /// The path being read/written/renamed.
        path: PathBuf,
        /// The underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// The file's content was not valid TOML, or did not match the `Config` schema. `message`
    /// is `toml`'s own `Display`, which includes the line/column when available.
    #[error("{path}: {message}")]
    Parse {
        /// The file that failed to parse.
        path: PathBuf,
        /// `toml::de::Error`'s message (line/column included for syntax errors).
        message: String,
    },
    /// The profile to edit is not in the config file.
    #[error("{path}: no profile with id {id:?}")]
    NoSuchProfile {
        /// The config file that was searched.
        path: PathBuf,
        /// The profile id that was not found.
        id: String,
    },
}

impl ConfigError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// The platform config directory (`$XDG_CONFIG_HOME/fogu`, `%APPDATA%\fogu`), or
/// `override_dir` verbatim when given (the `--config-dir` CLI flag).
#[must_use]
pub fn config_dir(override_dir: Option<&Path>) -> PathBuf {
    override_dir.map_or_else(crate::platform::config_dir, Path::to_path_buf)
}

/// Loads `config.toml` from `dir`. A missing file is not an error (returns `Config::default()`);
/// any other failure (unreadable file, bad TOML, schema mismatch) also falls back to defaults but
/// is returned alongside so the caller can log it and show a toast.
#[must_use]
pub fn load(dir: &Path) -> (Config, Option<ConfigError>) {
    let path = dir.join(FILE_NAME);
    match std::fs::read_to_string(&path) {
        Ok(text) => match parse(&text, &path) {
            Ok(config) => (config, None),
            Err(err) => (Config::default(), Some(err)),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => (Config::default(), None),
        Err(err) => (Config::default(), Some(ConfigError::io(&path, err))),
    }
}

fn parse(text: &str, path: &Path) -> Result<Config, ConfigError> {
    let to_parse_error = |message: &dyn std::fmt::Display| ConfigError::Parse {
        path: path.to_path_buf(),
        message: message.to_string(),
    };
    let raw: toml::Table = toml::from_str(text).map_err(|err| to_parse_error(&err))?;
    let raw = migrate(raw);
    toml::Value::Table(raw)
        .try_into()
        .map_err(|err: toml::de::Error| to_parse_error(&err))
}

/// Upgrades an older raw TOML table to the current schema, operating before typed
/// deserialisation. A no-op
/// chain today: `version = 1` is the only version that has ever existed. Add a `version == N =>
/// { ...; table.insert("version", (N + 1).into()); }` arm here for the next breaking change.
fn migrate(table: toml::Table) -> toml::Table {
    table
}

/// Sets (`Some`) or removes (`None`) the `background` key of the `[[profiles]]` entry with `id`
/// in `dir/config.toml`, leaving every other byte of the file (comments, order, spacing)
/// untouched. The file must already exist, parse as TOML and contain the profile: nothing is
/// ever created or rewritten from the typed `Config`. Written atomically (temp file, `fsync`,
/// rename), mode `0600`, keeping the previous file as `config.toml.bak`.
pub fn set_profile_background(
    dir: &Path,
    id: &str,
    background: Option<&Background>,
) -> Result<(), ConfigError> {
    let path = dir.join(FILE_NAME);
    let text = std::fs::read_to_string(&path).map_err(|err| ConfigError::io(&path, err))?;
    let parse_error = |message: String| ConfigError::Parse {
        path: path.clone(),
        message,
    };
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| parse_error(e.to_string()))?;
    let profile = doc
        .get_mut("profiles")
        .and_then(toml_edit::Item::as_array_of_tables_mut)
        .and_then(|profiles| {
            profiles
                .iter_mut()
                .find(|p| p.get("id").and_then(toml_edit::Item::as_str) == Some(id))
        })
        .ok_or_else(|| ConfigError::NoSuchProfile {
            path: path.clone(),
            id: id.to_owned(),
        })?;
    match background {
        None => {
            profile.remove("background");
        }
        Some(background) => {
            let mut value = background_value(background);
            // Keep a trailing `# comment` on the line being replaced.
            if let Some(old) = profile
                .get("background")
                .and_then(toml_edit::Item::as_value)
            {
                *value.decor_mut() = old.decor().clone();
            }
            profile.insert("background", toml_edit::Item::Value(value));
        }
    }
    write_atomic(dir, doc.to_string().as_bytes())
}

/// Replaces `dir/config.toml` with `text` as typed (Settings › config.toml), only when it loads:
/// a file that would fall back to the defaults is refused, nothing written. Same atomic write,
/// `.bak` and symlink handling as every other edit.
pub fn write_text(dir: &Path, text: &str) -> Result<Config, ConfigError> {
    let config = parse(text, &dir.join(FILE_NAME))?;
    write_atomic(dir, text.as_bytes())?;
    Ok(config)
}

/// `background` as the inline table the docs show: `{ color = "#rrggbb" }` or
/// `{ image = { path = "…", opacity = 0.6, fit = "contain" } }` (`blur` only when on).
fn background_value(background: &Background) -> toml_edit::Value {
    let mut outer = toml_edit::InlineTable::new();
    match background {
        Background::Color(color) => {
            outer.insert("color", color.to_string().into());
        }
        Background::Image {
            path,
            opacity,
            fit,
            blur,
        } => {
            let mut image = toml_edit::InlineTable::new();
            image.insert("path", path.to_string_lossy().as_ref().into());
            // Through the shortest decimal form: 0.6_f32 as f64 would print 0.6000000238418579.
            let opacity = opacity.to_string().parse::<f64>().unwrap_or(1.0);
            image.insert("opacity", opacity.into());
            let fit = match fit {
                Fit::Cover => "cover",
                Fit::Contain => "contain",
                Fit::Tile => "tile",
            };
            image.insert("fit", fit.into());
            if *blur {
                image.insert("blur", true.into());
            }
            outer.insert("image", toml_edit::Value::InlineTable(image));
        }
    }
    toml_edit::Value::InlineTable(outer)
}

/// Replaces `dir/config.toml` without a moment where it is missing: the old content is first
/// written to `config.toml.bak`, then the new one over the file a symlinked `config.toml` points
/// at (so a dotfiles link stays a link). Both through [`crate::fsutil::write_private_atomic`].
pub(super) fn write_atomic(dir: &Path, contents: &[u8]) -> Result<(), ConfigError> {
    let dest = dir.join(FILE_NAME);
    match std::fs::read(&dest) {
        Ok(old) => {
            let bak = dir.join(format!("{FILE_NAME}.bak"));
            crate::fsutil::write_private_atomic(&bak, &old)
                .map_err(|err| ConfigError::io(&bak, err))?;
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(ConfigError::io(&dest, err)),
    }
    let target = dest.canonicalize().unwrap_or(dest);
    crate::fsutil::write_private_atomic(&target, contents)
        .map_err(|err| ConfigError::io(&target, err))
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn write_text_refuses_a_file_that_would_not_load() {
        let dir = std::env::temp_dir().join(format!("fogu-write-text-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        std::fs::write(&path, "# mine\n").unwrap();
        for bad in ["[appearance\n", "[appearance]\nfont_size = \"big\"\n"] {
            assert!(write_text(&dir, bad).is_err(), "{bad}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine\n");
        }
        let good = "# typed\n[appearance]\nfont_size = 14.0\n";
        assert!((write_text(&dir, good).unwrap().appearance.font_size - 14.0).abs() < f32::EPSILON);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), good);
        assert_eq!(
            std::fs::read_to_string(dir.join("config.toml.bak")).unwrap(),
            "# mine\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn config_dir_honours_the_override() {
        let dir = config_dir(Some(Path::new("/tmp/example")));
        assert_eq!(dir, PathBuf::from("/tmp/example"));
    }

    #[test]
    fn config_dir_without_override_is_under_home() {
        let dir = config_dir(None);
        assert!(dir.ends_with("fogu"), "{}", dir.display());
    }

    #[test]
    fn missing_file_loads_defaults_without_an_error() {
        let tmp = tempdir();
        let (config, err) = load(tmp.path());
        assert!(err.is_none());
        assert_eq!(config, Config::default());
    }

    #[test]
    fn syntax_error_falls_back_to_defaults_and_reports_line_col() {
        let tmp = tempdir();
        std::fs::write(tmp.path().join(FILE_NAME), "not valid toml [[[").unwrap();
        let (config, err) = load(tmp.path());
        assert_eq!(config, Config::default());
        let err = err.expect("a parse error");
        let message = err.to_string();
        assert!(message.contains("line"), "{message}");
    }

    const FIXTURE: &str = "\
# my hand-written config
version = 1

[appearance]
font_size = 14   # big enough
color_scheme = \"Dracula\"


# --- work ---
[[profiles]]
name = \"Work\"   # shown in the menu
id = \"work\"
type = \"local\"

# --- play ---
[[profiles]]
type = \"local\"
id = \"play\"
name = \"Play\"
background = { color = \"#112233\" }   # keep me
";

    fn fixture(tmp: &TempDir) -> PathBuf {
        let path = tmp.path().join(FILE_NAME);
        std::fs::write(&path, FIXTURE).unwrap();
        path
    }

    fn differing_lines(before: &str, after: &str) -> (Vec<String>, Vec<String>) {
        let b: Vec<&str> = before.lines().collect();
        let a: Vec<&str> = after.lines().collect();
        let prefix = b.iter().zip(&a).take_while(|(x, y)| x == y).count();
        let suffix = b[prefix..]
            .iter()
            .rev()
            .zip(a[prefix..].iter().rev())
            .take_while(|(x, y)| x == y)
            .count();
        let owned = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect();
        (
            owned(&b[prefix..b.len() - suffix]),
            owned(&a[prefix..a.len() - suffix]),
        )
    }

    #[test]
    fn set_background_adds_only_that_key_and_keeps_a_0600_backup() {
        let tmp = tempdir();
        let path = fixture(&tmp);
        let bg = Background::Image {
            path: PathBuf::from("~/wall.jpg"),
            opacity: 0.6,
            fit: Fit::Contain,
            blur: false,
        };
        set_profile_background(tmp.path(), "work", Some(&bg)).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        let (removed, added) = differing_lines(FIXTURE, &after);
        assert!(removed.is_empty(), "{removed:?}");
        assert_eq!(
            added,
            [
                "background = { image = { path = \"~/wall.jpg\", opacity = 0.6, fit = \"contain\" } }"
            ]
        );
        // Everything else is byte-identical: removing the added line gives the original back.
        let restored = after.replace(&format!("{}\n", added[0]), "");
        assert_eq!(restored, FIXTURE);

        let (config, err) = load(tmp.path());
        assert!(err.is_none());
        assert_eq!(config.profiles[0].background, Some(bg));
        let bak = tmp.path().join(format!("{FILE_NAME}.bak"));
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), FIXTURE);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            // The file and its backup are written through `fsutil`, private on Unix.
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), crate::fsutil::FILE_MODE);
            assert_eq!(mode(&bak), crate::fsutil::FILE_MODE);
        }
    }

    #[test]
    fn set_background_replaces_in_place_and_keeps_the_trailing_comment() {
        let tmp = tempdir();
        let path = fixture(&tmp);
        let bg = Background::Color(crate::RgbColor::from_hex(0x00_aa_bb));
        set_profile_background(tmp.path(), "play", Some(&bg)).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        let (removed, added) = differing_lines(FIXTURE, &after);
        assert_eq!(
            removed,
            ["background = { color = \"#112233\" }   # keep me"]
        );
        assert_eq!(added, ["background = { color = \"#00aabb\" }   # keep me"]);
    }

    #[test]
    fn remove_background_deletes_only_that_line() {
        let tmp = tempdir();
        let path = fixture(&tmp);
        set_profile_background(tmp.path(), "play", None).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        let (removed, added) = differing_lines(FIXTURE, &after);
        assert_eq!(
            removed,
            ["background = { color = \"#112233\" }   # keep me"]
        );
        assert!(added.is_empty(), "{added:?}");
    }

    #[test]
    fn unknown_profile_or_unparsable_or_missing_file_is_an_error_and_untouched() {
        let tmp = tempdir();
        let path = fixture(&tmp);
        let bg = Background::Color(crate::RgbColor::from_hex(1));
        let err = set_profile_background(tmp.path(), "gone", Some(&bg)).unwrap_err();
        assert!(matches!(err, ConfigError::NoSuchProfile { .. }), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), FIXTURE);
        assert!(!tmp.path().join(format!("{FILE_NAME}.bak")).exists());

        std::fs::write(&path, "not valid toml [[[").unwrap();
        let err = set_profile_background(tmp.path(), "work", Some(&bg)).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "not valid toml [[["
        );
        assert!(!tmp.path().join(format!("{FILE_NAME}.bak")).exists());

        std::fs::remove_file(&path).unwrap();
        let err = set_profile_background(tmp.path(), "work", Some(&bg)).unwrap_err();
        assert!(matches!(err, ConfigError::Io { .. }), "{err}");
        assert!(!path.exists());
    }

    /// A crash leaves a stale temp file behind; a later save must neither write through it (a
    /// symlink planted under that name) nor fail on it.
    #[cfg(unix)]
    #[test]
    fn save_never_writes_through_a_planted_temp_file() {
        let tmp = tempdir();
        let path = fixture(&tmp);
        let victim = tmp.path().join("victim");
        std::fs::write(&victim, "keep").unwrap();
        std::os::unix::fs::symlink(&victim, tmp.path().join(format!("{FILE_NAME}.tmp"))).unwrap();
        set_profile_background(tmp.path(), "play", None).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
        assert_ne!(std::fs::read_to_string(&path).unwrap(), FIXTURE);
    }

    /// A `config.toml` symlinked from a dotfiles repo stays a symlink; the repo file is updated.
    #[cfg(unix)]
    #[test]
    fn save_keeps_a_symlinked_config_in_place() {
        let tmp = tempdir();
        let dotfiles = tmp.path().join("dotfiles");
        std::fs::create_dir(&dotfiles).unwrap();
        let real = dotfiles.join(FILE_NAME);
        std::fs::write(&real, FIXTURE).unwrap();
        let dir = tmp.path().join("conf");
        std::fs::create_dir(&dir).unwrap();
        let link = dir.join(FILE_NAME);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        set_profile_background(&dir, "play", None).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!std::fs::read_to_string(&real).unwrap().contains("#112233"));
        let bak = dir.join(format!("{FILE_NAME}.bak"));
        assert_eq!(std::fs::read_to_string(bak).unwrap(), FIXTURE);
    }

    /// A tiny unique-per-test temp dir under the system temp dir, removed on drop.
    struct TempDir(PathBuf);
    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir() -> TempDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fogu-core-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}
