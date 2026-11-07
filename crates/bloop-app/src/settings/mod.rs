//! The settings file `~/Library/Application Support/Bloop/config.toml`, and the prompts folder next to it.
//!
//! The settings hub reads the file at each dictation key press. A missing key has its default value.
//! When the file does not exist at start, Bloop writes the commented template. It never changes an existing file at start.
//! The "Correct text" menu item writes its state to `[refine] enabled`. The comments and the other keys stay.
//! The errors and the log never contain the file content, because a parser message can quote a value.

mod edit;
mod schema;
mod template;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::files::{self, FileError};
pub(crate) use schema::{HoldMs, PromptName, Settings, parse};

/// The settings folder in the home folder.
const FOLDER: &str = "Library/Application Support/Bloop";

/// The size limit of `config.toml`, in bytes.
pub(crate) const LIMIT: u64 = 64 * 1024;

/// The paths of the settings folder.
pub(crate) struct Paths {
    pub(crate) config: PathBuf,
    /// The folder of the prompt files of the user.
    pub(crate) prompts: PathBuf,
    pub(crate) dictionary: PathBuf,
}

impl Paths {
    /// Returns the paths in the home folder, or `None` when the home folder is unknown.
    pub(crate) fn in_home() -> Option<Self> {
        std::env::home_dir().map(|home| Self::new(&home.join(FOLDER)))
    }

    /// Returns the paths in the settings folder `folder`.
    pub(crate) fn new(folder: &Path) -> Self {
        Self {
            config: folder.join("config.toml"),
            prompts: folder.join("prompts"),
            dictionary: folder.join("dictionary.toml"),
        }
    }
}

/// Why the settings file is invalid. It contains no file content.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Invalid {
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
pub(crate) enum NotSaved {
    /// The current file is invalid. Bloop does not change it.
    Invalid(Invalid),
    /// The write of the new file failed.
    Write(io::ErrorKind),
}

/// Writes the template when `config.toml` does not exist, and creates the prompts folder.
/// A failure logs a warning with the error kind. Then Bloop continues.
pub(crate) fn prepare(paths: &Paths) {
    match files::write_if_missing(&paths.config, &template::template()) {
        Ok(true) => tracing::info!("the settings file did not exist, Bloop wrote the template"),
        Ok(false) => {}
        Err(error) => tracing::warn!(kind = ?error.kind(), "the settings template is not written"),
    }
    if let Err(error) = fs::create_dir_all(&paths.prompts) {
        tracing::warn!(kind = ?error.kind(), "the prompts folder is not created");
    }
}

/// Sets the correction state in the file at `path`, after it checks the current file.
/// When the file does not exist, it starts from the template, and it creates the folder.
pub(crate) fn write(path: &Path, on: bool) -> Result<(), NotSaved> {
    let text = files::read_bounded(path, LIMIT).map_err(|error| NotSaved::Invalid(error.into()))?;
    let new = match &text {
        Some(text) => {
            parse(text).map_err(NotSaved::Invalid)?;
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
    use std::path::Path;

    use super::{Invalid, LIMIT, NotSaved, Paths, Settings, parse, prepare, write};
    use crate::files::read_bounded;
    use crate::files::tests::scratch_dir;
    use crate::settings::template::template;

    /// Reads and parses the file at `path`, as the settings hub does. Returns `None` when the file does not exist.
    fn read(path: &Path) -> Result<Option<Settings>, Invalid> {
        read_bounded(path, LIMIT)?
            .map(|text| parse(&text))
            .transpose()
    }

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
    fn a_file_over_the_limit_is_invalid_and_the_write_does_not_change_it() {
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
    fn the_write_creates_the_folder_and_the_file() {
        let dir = scratch_dir("settings-new");
        let path = dir.join("Bloop").join("config.toml");

        write(&path, true).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            template().replace("enabled = false ", "enabled = true  ")
        );
        assert!(read(&path).unwrap().unwrap().refine.enabled);

        write(&path, false).unwrap();
        assert!(!read(&path).unwrap().unwrap().refine.enabled);
        assert!(!path.with_added_extension("tmp").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_write_does_not_change_an_invalid_file() {
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
