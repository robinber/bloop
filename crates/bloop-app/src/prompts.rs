//! The correction prompts: the built-in prompts and the prompt files of the user.
//! The log names a prompt and its source, never its text.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use crate::files::{self, FileError};
use crate::settings::PromptName;

/// The size limit of a prompt file, in bytes.
const LIMIT: u64 = 32 * 1024;

/// A prompt in the binary.
pub(crate) struct BuiltIn {
    /// The name of the prompt: its file name in `prompts/` without `.md`.
    pub(crate) name: &'static str,
    /// The instructions to the chat model.
    pub(crate) text: &'static str,
}

impl BuiltIn {
    pub(crate) fn to_prompt(&self) -> Prompt {
        Prompt {
            name: self.name.to_owned(),
            text: self.text.to_owned(),
            source: Source::BuiltIn,
        }
    }
}

/// The default prompt: punctuation, capital letters, hesitations, stutters, and spoken corrections.
pub(crate) const NATURAL_LIGHT: BuiltIn = BuiltIn {
    name: "natural-light",
    text: include_str!("../../../prompts/natural-light.md"),
};

/// The built-in prompts.
const BUILT_IN: [BuiltIn; 1] = [NATURAL_LIGHT];

/// Where a prompt comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    BuiltIn,
    /// A file of the prompts folder.
    File,
}

/// A correction prompt: the system message of `LlmRefine`.
///
/// `Debug` shows the name, the source, and the number of characters, never the text.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Prompt {
    pub(crate) name: String,
    /// The instructions to the chat model.
    pub(crate) text: String,
    pub(crate) source: Source,
}

impl fmt::Debug for Prompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prompt")
            .field("name", &self.name)
            .field("source", &self.source)
            .field("chars", &self.text.chars().count())
            .finish()
    }
}

/// Why the selected prompt is not available. It contains no file content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptError {
    /// No file of the prompts folder and no built-in prompt has the name.
    Missing,
    /// The prompt is empty or contains only whitespace.
    Empty,
    /// The file is larger than [`LIMIT`].
    TooLarge,
    /// The file exists, but Bloop cannot read it as UTF-8 text.
    Unreadable(io::ErrorKind),
}

/// Returns the prompt `name`: the file `<name>.md` of `folder` when it exists, else the built-in prompt with that name.
/// Thus a file replaces the built-in prompt with the same name.
///
/// # Errors
///
/// `Missing` when neither exists, and `Empty`, `TooLarge`, or `Unreadable` for a prompt that is not valid.
pub(crate) fn resolve(folder: &Path, name: &PromptName) -> Result<Prompt, PromptError> {
    let path = folder.join(format!("{}.md", name.as_str()));
    let prompt = match files::read_bounded(&path, LIMIT) {
        Ok(Some(text)) => Prompt {
            name: name.as_str().to_owned(),
            text,
            source: Source::File,
        },
        Ok(None) => BUILT_IN
            .iter()
            .find(|prompt| prompt.name == name.as_str())
            .ok_or(PromptError::Missing)?
            .to_prompt(),
        Err(FileError::TooLarge) => return Err(PromptError::TooLarge),
        Err(FileError::Unreadable(kind)) => return Err(PromptError::Unreadable(kind)),
    };
    if prompt.text.trim().is_empty() {
        return Err(PromptError::Empty);
    }
    Ok(prompt)
}

/// Returns the prompt names for the status menu: the built-in names and the names of the `.md` files of `folder`,
/// sorted and without duplicates. It skips a file name that is not UTF-8 or that is not a valid prompt name.
/// It does not read the files.
pub(crate) fn names(folder: &Path) -> Vec<PromptName> {
    let mut names: BTreeSet<PromptName> = BUILT_IN
        .iter()
        .filter_map(|prompt| PromptName::try_from(prompt.name.to_owned()).ok())
        .collect();
    match fs::read_dir(folder) {
        Ok(entries) => {
            names.extend(entries.filter_map(|entry| name_of(&entry.ok()?.file_name())));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => tracing::debug!(
            kind = ?error.kind(),
            "the prompts folder is not readable, the menu lists the built-in prompts only"
        ),
    }
    names.into_iter().collect()
}

/// Returns the prompt name of the file name `file`: its name without `.md`, when it is UTF-8 and a valid prompt name.
fn name_of(file: &OsStr) -> Option<PromptName> {
    let stem = file.to_str()?.strip_suffix(".md")?;
    PromptName::try_from(stem.to_owned()).ok()
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::ffi::OsStr;
    use std::fs;
    use std::io;
    use std::os::unix::ffi::OsStrExt as _;

    use super::{LIMIT, NATURAL_LIGHT, Prompt, PromptError, Source, name_of, names, resolve};
    use crate::files::tests::scratch_dir;
    use crate::settings::PromptName;

    fn name(value: &str) -> PromptName {
        PromptName::try_from(value.to_owned()).unwrap()
    }

    #[test]
    fn the_prompt_names_the_delimiters_of_the_correction_request() {
        assert!(NATURAL_LIGHT.text.contains("`<transcript>` line"));
        assert!(NATURAL_LIGHT.text.contains("`</transcript>` line"));
    }

    #[test]
    fn the_default_name_selects_the_built_in_prompt_without_a_file() {
        let dir = scratch_dir("prompt-built-in");

        let prompt = resolve(&dir, &PromptName::default()).unwrap();

        assert_eq!(prompt.name, "natural-light");
        assert_eq!(prompt.text, NATURAL_LIGHT.text);
        assert_eq!(prompt.source, Source::BuiltIn);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_of_the_folder_gives_the_prompt() {
        let dir = scratch_dir("prompt-file");
        fs::write(dir.join("dev.md"), "Corrige le texte.\n").unwrap();

        let prompt = resolve(&dir, &name("dev")).unwrap();

        assert_eq!(prompt.name, "dev");
        assert_eq!(prompt.text, "Corrige le texte.\n");
        assert_eq!(prompt.source, Source::File);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_replaces_the_built_in_prompt_with_the_same_name() {
        let dir = scratch_dir("prompt-replace");
        fs::write(dir.join("natural-light.md"), "Mon prompt.").unwrap();

        let prompt = resolve(&dir, &name("natural-light")).unwrap();

        assert_eq!(prompt.text, "Mon prompt.");
        assert_eq!(prompt.source, Source::File);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_name_without_a_file_or_a_built_in_prompt_is_missing() {
        let dir = scratch_dir("prompt-missing");

        assert_matches!(resolve(&dir, &name("dev")), Err(PromptError::Missing));
        assert_matches!(
            resolve(&dir.join("absent"), &name("dev")),
            Err(PromptError::Missing)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_empty_file_is_not_valid() {
        let dir = scratch_dir("prompt-empty");
        for text in ["", " \n\t\n"] {
            fs::write(dir.join("dev.md"), text).unwrap();

            assert_matches!(
                resolve(&dir, &name("dev")),
                Err(PromptError::Empty),
                "{text:?}"
            );
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_over_32_kib_is_too_large() {
        let dir = scratch_dir("prompt-large");
        let limit = usize::try_from(LIMIT).unwrap();
        fs::write(dir.join("dev.md"), "a".repeat(limit)).unwrap();
        assert_matches!(resolve(&dir, &name("dev")), Ok(Prompt { .. }));

        fs::write(dir.join("dev.md"), "a".repeat(limit + 1)).unwrap();

        assert_matches!(resolve(&dir, &name("dev")), Err(PromptError::TooLarge));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_is_not_utf_8_is_not_valid() {
        let dir = scratch_dir("prompt-utf8");
        fs::write(dir.join("dev.md"), b"Corrige \xff").unwrap();

        assert_matches!(
            resolve(&dir, &name("dev")),
            Err(PromptError::Unreadable(io::ErrorKind::InvalidData))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    fn listed(folder: &std::path::Path) -> Vec<String> {
        names(folder)
            .iter()
            .map(|name| name.as_str().to_owned())
            .collect()
    }

    #[test]
    fn the_names_are_the_built_in_names_and_the_md_files_sorted_without_duplicates() {
        let dir = scratch_dir("prompt-names");
        for file in [
            "dev.md",
            "Mail.md",
            "a_b-2.md",
            "natural-light.md",
            "notes.txt",
            "dev.md.bak",
            "bad name.md",
            ".md",
            "._dev.md",
        ] {
            fs::write(dir.join(file), "Corrige.\n").unwrap();
        }

        assert_eq!(listed(&dir), ["Mail", "a_b-2", "dev", "natural-light"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn without_the_folder_the_names_are_the_built_in_names() {
        let dir = scratch_dir("prompt-names-missing");

        assert_eq!(listed(&dir.join("absent")), ["natural-light"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_name_that_is_not_utf_8_or_not_a_valid_name_gives_no_name() {
        assert_eq!(
            name_of(OsStr::new("dev.md")).map(|name| name.as_str().to_owned()),
            Some("dev".to_owned())
        );
        for file in [
            OsStr::from_bytes(b"d\xffv.md"),
            OsStr::new("dev"),
            OsStr::new("dev.MD"),
            OsStr::new("../dev.md"),
            OsStr::new(".md"),
        ] {
            assert_eq!(name_of(file), None, "{file:?}");
        }
        let long = format!("{}.md", "a".repeat(65));
        assert_eq!(name_of(OsStr::new(&long)), None);
    }

    #[test]
    fn the_debug_of_a_prompt_shows_no_text() {
        let prompt = NATURAL_LIGHT.to_prompt();

        let debug = format!("{prompt:?}");

        assert!(debug.starts_with("Prompt { name: \"natural-light\", source: BuiltIn, chars: "));
        assert!(!debug.contains("transcript"));
    }
}
