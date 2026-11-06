//! The settings file `~/Library/Application Support/Bloop/config.toml`.
//!
//! Bloop reads the file one time at start. A missing file gives the default settings.
//! The "Correct text" menu item writes its state to `[refine] enabled`. The comments and the other keys stay.
//! The errors and the log never contain the file content, because a parser message can quote a value.

use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use bloop_core::error::AppError;
use serde::Deserialize;
use toml::Spanned;
use toml_edit::{DocumentMut, Item, TomlError, Value};

use crate::dictionary::line;

/// The path of the file in the home folder.
const FILE: &str = "Library/Application Support/Bloop/config.toml";

/// The only `schema` value that this version of Bloop reads.
const SCHEMA: u64 = 1;

/// The text that a new file starts from.
const NEW_FILE: &str = "schema = 1\n";

/// Why the settings file is invalid. It contains no file content.
#[derive(Debug, PartialEq, Eq)]
enum Invalid {
    /// The home folder is unknown.
    NoHome,
    /// The file exists, but Bloop cannot read it as UTF-8 text.
    Unreadable(io::ErrorKind),
    /// The parser rejected the file: syntax, value type, or unknown key.
    /// `line` starts at 1. It is `None` when the parser gives no position.
    Parse { line: Option<usize> },
    /// The `schema` value is not [`SCHEMA`].
    Schema { line: Option<usize> },
}

/// Why Bloop did not save the correction state. It contains no file content.
#[derive(Debug, PartialEq, Eq)]
enum NotSaved {
    /// The current file is invalid. Bloop does not change it.
    Invalid(Invalid),
    /// The write of the new file failed.
    Write(io::ErrorKind),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    schema: Option<Spanned<u64>>,
    #[serde(default)]
    refine: Refine,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Refine {
    #[serde(default)]
    enabled: bool,
}

/// Reads the settings file and logs the correction state or the error, never the file content.
/// Returns the start state of the "Correct text" toggle.
///
/// # Errors
///
/// `InvalidSettings` when the file exists but is not valid.
pub(crate) fn load() -> Result<bool, AppError> {
    match path().and_then(|path| read(&path)) {
        Ok(Some(on)) => {
            tracing::info!(correction = on, "the settings are ready");
            Ok(on)
        }
        Ok(None) => {
            tracing::info!("the settings file does not exist, the correction is off");
            Ok(false)
        }
        Err(error) => {
            tracing::error!(
                ?error,
                "the settings file is invalid, each dictation fails until the file is fixed and Bloop starts again"
            );
            Err(AppError::InvalidSettings)
        }
    }
}

/// Writes the correction state to the settings file, and logs a warning if it fails.
/// It does not change an invalid file. It never panics, because it runs in an `AppKit` callback.
pub(crate) fn store(on: bool) {
    if let Err(error) = path()
        .map_err(NotSaved::Invalid)
        .and_then(|path| write(&path, on))
    {
        tracing::warn!(?error, "the correction state is not saved");
    }
}

fn path() -> Result<PathBuf, Invalid> {
    std::env::home_dir()
        .map(|home| home.join(FILE))
        .ok_or(Invalid::NoHome)
}

/// Reads and parses the file at `path`. Returns `None` when the file does not exist.
fn read(path: &Path) -> Result<Option<bool>, Invalid> {
    read_text(path)?.map(|text| parse(&text)).transpose()
}

fn read_text(path: &Path) -> Result<Option<String>, Invalid> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Invalid::Unreadable(error.kind())),
    }
}

/// Returns the correction state of a valid file.
fn parse(text: &str) -> Result<bool, Invalid> {
    let settings: Settings = toml::from_str(text).map_err(|error| Invalid::Parse {
        line: error.span().and_then(|span| line(text, span.start)),
    })?;
    if let Some(schema) = settings.schema
        && *schema.get_ref() != SCHEMA
    {
        return Err(Invalid::Schema {
            line: line(text, schema.span().start),
        });
    }
    Ok(settings.refine.enabled)
}

/// Sets the correction state in the file at `path`, after it checks the current file.
/// Creates the folder and the file when they do not exist.
fn write(path: &Path, on: bool) -> Result<(), NotSaved> {
    let text = read_text(path).map_err(NotSaved::Invalid)?;
    if let Some(text) = &text {
        parse(text).map_err(NotSaved::Invalid)?;
    }
    let new =
        with_correction(text.as_deref().unwrap_or(NEW_FILE), on).map_err(NotSaved::Invalid)?;
    replace(path, &new).map_err(|error| NotSaved::Write(error.kind()))
}

/// Returns `text` with `[refine] enabled` set to `on`. The other text does not change.
fn with_correction(text: &str, on: bool) -> Result<String, Invalid> {
    let mut document: DocumentMut = text.parse().map_err(|error: TomlError| Invalid::Parse {
        line: error.span().and_then(|span| line(text, span.start)),
    })?;
    let refine = document
        .entry("refine")
        .or_insert(toml_edit::table())
        .as_table_like_mut()
        .ok_or(Invalid::Parse { line: None })?;
    let mut new = Value::from(on);
    // `insert` on an existing key resets the key format, which removes the comments above it.
    match refine.get_mut("enabled") {
        Some(item) => {
            if let Some(old) = item.as_value() {
                *new.decor_mut() = old.decor().clone();
            }
            *item = Item::Value(new);
        }
        None => {
            refine.insert("enabled", Item::Value(new));
        }
    }
    Ok(document.to_string())
}

/// Writes `text` to a temporary file in the folder of `path`, then renames it to `path`.
/// Thus a failed write never leaves a partial settings file.
fn replace(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temporary = path.with_added_extension("tmp");
    let mut file = File::create(&temporary)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{Invalid, NEW_FILE, NotSaved, parse, read, with_correction, write};

    fn scratch_dir(test: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "bloop-settings-{test}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let dir = scratch_dir("missing");

        assert_eq!(read(&dir.join("config.toml")), Ok(None));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_empty_file_turns_the_correction_off() {
        assert_eq!(parse(""), Ok(false));
    }

    #[test]
    fn the_refine_table_gives_the_correction_state() {
        assert_eq!(parse("[refine]\nenabled = true\n"), Ok(true));
        assert_eq!(parse("[refine]\nenabled = false\n"), Ok(false));
        assert_eq!(parse("[refine]\n"), Ok(false));
    }

    #[test]
    fn schema_1_is_valid() {
        assert_eq!(parse("schema = 1\n"), Ok(false));
        assert_eq!(parse("schema = 1\n\n[refine]\nenabled = true\n"), Ok(true));
    }

    #[test]
    fn another_schema_is_an_error_with_its_line() {
        assert_eq!(
            parse("# Bloop\nschema = 2\n"),
            Err(Invalid::Schema { line: Some(2) })
        );
        assert_eq!(
            parse("schema = 0\n"),
            Err(Invalid::Schema { line: Some(1) })
        );
    }

    #[test]
    fn an_unknown_key_is_an_error_without_the_file_content() {
        let error = parse("schema = 1\nsecret = \"dictée\"\n").unwrap_err();

        assert_eq!(error, Invalid::Parse { line: Some(2) });
        assert_eq!(format!("{error:?}"), "Parse { line: Some(2) }");
    }

    #[test]
    fn an_unknown_refine_key_is_an_error() {
        assert_eq!(
            parse("[refine]\nenabled = true\nprompt = \"natural-light\"\n"),
            Err(Invalid::Parse { line: Some(3) })
        );
    }

    #[test]
    fn a_value_of_another_type_is_an_error() {
        assert_eq!(
            parse("schema = 1\n\n[refine]\nenabled = \"yes\"\n"),
            Err(Invalid::Parse { line: Some(4) })
        );
        assert_eq!(
            parse("schema = \"1\"\n"),
            Err(Invalid::Parse { line: Some(1) })
        );
    }

    #[test]
    fn a_new_file_gets_the_schema_and_the_refine_table() {
        assert_eq!(
            with_correction(NEW_FILE, true).unwrap(),
            "schema = 1\n\n[refine]\nenabled = true\n"
        );
    }

    #[test]
    fn the_change_replaces_the_value() {
        assert_eq!(
            with_correction("schema = 1\n\n[refine]\nenabled = true\n", false).unwrap(),
            "schema = 1\n\n[refine]\nenabled = false\n"
        );
    }

    #[test]
    fn the_change_keeps_the_comments_and_the_spacing() {
        let text = "# Bloop settings\nschema = 1\n\n\n# The correction\n[refine]  # menu\n  # Set from the menu\n  enabled   =   false   # off for now\n\n# End\n";

        assert_eq!(
            with_correction(text, true).unwrap(),
            text.replace("=   false", "=   true")
        );
    }

    #[test]
    fn a_file_without_the_refine_table_gets_a_standard_table() {
        assert_eq!(
            with_correction("# Bloop settings\nschema = 1\n", true).unwrap(),
            "# Bloop settings\nschema = 1\n\n[refine]\nenabled = true\n"
        );
    }

    #[test]
    fn store_creates_the_folder_and_the_file() {
        let dir = scratch_dir("new");
        let path = dir.join("Bloop").join("config.toml");

        write(&path, true).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "schema = 1\n\n[refine]\nenabled = true\n"
        );
        assert_eq!(read(&path), Ok(Some(true)));

        write(&path, false).unwrap();
        assert_eq!(read(&path), Ok(Some(false)));
        assert!(!path.with_added_extension("tmp").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn store_does_not_change_an_invalid_file() {
        let dir = scratch_dir("invalid");
        let path = dir.join("config.toml");
        let text = b"# Bloop\n[refine]\nenabled = \"yes\"\n";
        fs::write(&path, text).unwrap();

        assert_eq!(
            write(&path, true),
            Err(NotSaved::Invalid(Invalid::Parse { line: Some(3) }))
        );
        assert_eq!(fs::read(&path).unwrap(), text);
        fs::remove_dir_all(&dir).unwrap();
    }
}
