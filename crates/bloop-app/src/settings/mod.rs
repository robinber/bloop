//! The settings file `~/Library/Application Support/Bloop/config.toml`, and the prompts folder next to it.
//!
//! Bloop reads the file one time at start. A missing key has its default value.
//! When the file does not exist at start, Bloop writes the commented template. It never changes an existing file at start.
//! The "Correct text" menu item writes its state to `[refine] enabled`. The comments and the other keys stay.
//! The errors and the log never contain the file content, because a parser message can quote a value.

mod edit;
mod schema;
mod template;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bloop_core::error::AppError;

use crate::files::{self, FileError};
pub(crate) use schema::{PromptName, Settings};

/// The settings folder in the home folder.
const FOLDER: &str = "Library/Application Support/Bloop";

/// The size limit of `config.toml`, in bytes.
const LIMIT: u64 = 64 * 1024;

/// The paths of the settings folder.
pub(crate) struct Paths {
    config: PathBuf,
    /// The folder of the prompt files of the user.
    pub(crate) prompts: PathBuf,
}

impl Paths {
    /// Returns the paths in the home folder, or `None` when the home folder is unknown.
    fn in_home() -> Option<Self> {
        std::env::home_dir().map(|home| Self::new(&home.join(FOLDER)))
    }

    fn new(folder: &Path) -> Self {
        Self {
            config: folder.join("config.toml"),
            prompts: folder.join("prompts"),
        }
    }
}

/// Why the settings file is invalid. It contains no file content.
#[derive(Debug, PartialEq, Eq)]
enum Invalid {
    /// The home folder is unknown.
    NoHome,
    /// The file is larger than [`LIMIT`].
    TooLarge,
    /// The file exists, but Bloop cannot read it as UTF-8 text.
    Unreadable(io::ErrorKind),
    /// The parser rejected the file: syntax, value type, value out of its limits, or unknown key.
    /// `line` starts at 1. It is `None` when the parser gives no position.
    Parse { line: Option<usize> },
    /// The `schema` value is not 1.
    Schema { line: Option<usize> },
}

impl From<FileError> for Invalid {
    fn from(error: FileError) -> Self {
        match error {
            FileError::TooLarge => Self::TooLarge,
            FileError::Unreadable(kind) => Self::Unreadable(kind),
        }
    }
}

/// Why Bloop did not save the correction state. It contains no file content.
#[derive(Debug, PartialEq, Eq)]
enum NotSaved {
    /// The current file is invalid. Bloop does not change it.
    Invalid(Invalid),
    /// The write of the new file failed.
    Write(io::ErrorKind),
}

/// Reads the settings at start. Before that, it writes the template when `config.toml` does not exist,
/// and it creates the prompts folder. It logs the nonsecret values or the error, never the file content.
///
/// Returns the settings, and the paths of the settings folder when the file is valid.
/// For an invalid file, the settings have the default values and the result is `InvalidSettings`.
pub(crate) fn load() -> (Settings, Result<Paths, AppError>) {
    let result = Paths::in_home().ok_or(Invalid::NoHome).and_then(|paths| {
        prepare(&paths);
        read(&paths.config).map(|settings| (settings.unwrap_or_default(), paths))
    });
    match result {
        Ok((settings, paths)) => {
            tracing::info!(
                hold_ms = settings.dictation.hold_ms.get(),
                language = settings.dictation.language.as_str(),
                transcriber = ?settings.transcriber,
                chat = ?settings.chat,
                prompt = settings.refine.prompt.as_str(),
                correction = settings.refine.enabled,
                "the settings are ready"
            );
            (settings, Ok(paths))
        }
        Err(error) => {
            tracing::error!(
                ?error,
                "the settings file is invalid, each dictation fails until the file is fixed and Bloop starts again"
            );
            (Settings::default(), Err(AppError::InvalidSettings))
        }
    }
}

/// Writes the correction state to the settings file, and logs a warning if it fails.
/// It does not change an invalid file. It never panics, because it runs in an `AppKit` callback.
pub(crate) fn store(on: bool) {
    let result = Paths::in_home()
        .ok_or(NotSaved::Invalid(Invalid::NoHome))
        .and_then(|paths| write(&paths.config, on));
    if let Err(error) = result {
        tracing::warn!(?error, "the correction state is not saved");
    }
}

/// Writes the template when `config.toml` does not exist, and creates the prompts folder.
/// A failure logs a warning with the error kind. Then Bloop continues.
fn prepare(paths: &Paths) {
    match files::write_if_missing(&paths.config, &template::template()) {
        Ok(true) => tracing::info!("the settings file did not exist, Bloop wrote the template"),
        Ok(false) => {}
        Err(error) => tracing::warn!(kind = ?error.kind(), "the settings template is not written"),
    }
    if let Err(error) = fs::create_dir_all(&paths.prompts) {
        tracing::warn!(kind = ?error.kind(), "the prompts folder is not created");
    }
}

/// Reads and parses the file at `path`. Returns `None` when the file does not exist.
fn read(path: &Path) -> Result<Option<Settings>, Invalid> {
    files::read_bounded(path, LIMIT)?
        .map(|text| schema::parse(&text))
        .transpose()
}

/// Sets the correction state in the file at `path`, after it checks the current file.
/// When the file does not exist, it starts from the template, and it creates the folder.
fn write(path: &Path, on: bool) -> Result<(), NotSaved> {
    let text = files::read_bounded(path, LIMIT).map_err(|error| NotSaved::Invalid(error.into()))?;
    let new = match &text {
        Some(text) => {
            schema::parse(text).map_err(NotSaved::Invalid)?;
            edit::with_correction(text, on)
        }
        None => edit::with_correction(&template::template(), on),
    }
    .map_err(NotSaved::Invalid)?;
    files::replace(path, &new).map_err(|error| NotSaved::Write(error.kind()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Invalid, LIMIT, NotSaved, Paths, Settings, prepare, read, write};
    use crate::files::tests::scratch_dir;
    use crate::settings::template::template;

    #[test]
    fn a_missing_file_is_not_an_error() {
        let dir = scratch_dir("settings-missing");

        assert_eq!(read(&dir.join("config.toml")), Ok(None));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_first_start_writes_the_template_and_creates_the_prompts_folder() {
        let dir = scratch_dir("settings-first");
        let paths = Paths::new(&dir.join("Bloop"));

        prepare(&paths);

        assert_eq!(fs::read_to_string(&paths.config).unwrap(), template());
        assert!(paths.prompts.is_dir());
        assert_eq!(read(&paths.config), Ok(Some(Settings::default())));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_later_start_keeps_the_existing_file_byte_for_byte() {
        let dir = scratch_dir("settings-later");
        let paths = Paths::new(&dir);
        let text = b"# mine\r\n[refine]\r\nenabled = true\r\n";
        fs::write(&paths.config, text).unwrap();

        prepare(&paths);

        assert_eq!(fs::read(&paths.config).unwrap(), text);
        let settings = read(&paths.config).unwrap().unwrap();
        assert!(settings.refine.enabled);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_over_the_limit_is_invalid_and_store_does_not_change_it() {
        let dir = scratch_dir("settings-large");
        let path = dir.join("config.toml");
        let text = format!(
            "schema = 1\n{}",
            "#".repeat(usize::try_from(LIMIT).unwrap())
        );
        fs::write(&path, &text).unwrap();

        assert_eq!(read(&path), Err(Invalid::TooLarge));
        assert_eq!(
            write(&path, true),
            Err(NotSaved::Invalid(Invalid::TooLarge))
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn store_creates_the_folder_and_the_file() {
        let dir = scratch_dir("settings-new");
        let path = dir.join("Bloop").join("config.toml");

        write(&path, true).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            template().replace("enabled = false", "enabled = true")
        );
        assert!(read(&path).unwrap().unwrap().refine.enabled);

        write(&path, false).unwrap();
        assert!(!read(&path).unwrap().unwrap().refine.enabled);
        assert!(!path.with_added_extension("tmp").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn store_does_not_change_an_invalid_file() {
        let dir = scratch_dir("settings-invalid");
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
