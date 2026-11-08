//! The correction prompts: the built-in prompts, the prompt files of the user, and the applications of each prompt.
//! The log names a prompt and its source, never its text.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use crate::files::{self, FileError};
use crate::settings::{AppId, PromptName, Settings};

/// The size limit of a prompt file, in bytes.
const LIMIT: u64 = 32 * 1024;

/// A prompt in the binary.
pub(crate) struct BuiltIn {
    /// The name of the prompt: its file name in `prompts/` without `.md`.
    pub(crate) name: &'static str,
    /// The instructions to the chat model.
    pub(crate) text: &'static str,
    /// The bundle identifiers of the default list of the prompt. Only the installed applications are default.
    pub(crate) apps: &'static [&'static str],
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
    apps: &[
        "com.apple.MobileSMS",
        "net.whatsapp.WhatsApp",
        "ru.keepcoder.Telegram",
        "org.telegram.desktop",
        "org.whispersystems.signal-desktop",
    ],
};

/// The prompt for technical dictation: the corrections of `natural-light`, and the written form of technical terms.
const CODE: BuiltIn = BuiltIn {
    name: "code",
    text: include_str!("../../../prompts/code.md"),
    apps: &[
        "com.apple.Terminal",
        "com.mitchellh.ghostty",
        "com.googlecode.iterm2",
        "dev.warp.Warp-Stable",
        "com.microsoft.VSCode",
        "com.todesktop.230313mzl4w4u92",
        "dev.zed.Zed",
        "com.apple.dt.Xcode",
        "com.anthropic.claudefordesktop",
        "com.openai.codex",
    ],
};

/// The built-in prompts.
const BUILT_IN: [BuiltIn; 2] = [NATURAL_LIGHT, CODE];

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

/// The applications of each prompt.
pub(crate) type AppLists = BTreeMap<PromptName, Vec<AppId>>;

/// Returns the default lists: one list for each built-in prompt, with the applications of its built-in list for which
/// `installed` gives `true`. A built-in prompt without an installed application has an empty list.
pub(crate) fn default_lists(installed: impl Fn(&str) -> bool) -> AppLists {
    BUILT_IN
        .iter()
        .filter_map(|prompt| {
            let name = PromptName::try_from(prompt.name.to_owned()).ok()?;
            let list = prompt
                .apps
                .iter()
                .filter(|id| installed(id))
                .filter_map(|&id| AppId::try_from(id.to_owned()).ok())
                .collect();
            Some((name, list))
        })
        .collect()
}

/// Returns the applications of each prompt: the lists of `[refine.apps]` when the file has the table, else `defaults`.
/// With the table, a prompt without a key has no application, and `defaults` do not apply.
/// No application is in two lists: the parser rejects it in the table, and no built-in list shares an application.
pub(crate) fn app_lists(settings: &Settings, defaults: &AppLists) -> AppLists {
    match &settings.refine.apps {
        Some(apps) => apps
            .iter()
            .map(|(prompt, list)| (prompt.clone(), list.to_vec()))
            .collect(),
        None => defaults.clone(),
    }
}

/// Returns the prompt whose list contains the application `app`, or `None` when no list contains it.
pub(crate) fn route<'a>(lists: &'a AppLists, app: Option<&str>) -> Option<&'a PromptName> {
    let app = app?;
    lists
        .iter()
        .find(|(_, list)| list.iter().any(|id| id.as_str() == app))
        .map(|(prompt, _)| prompt)
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

    use super::{
        AppLists, BUILT_IN, CODE, LIMIT, NATURAL_LIGHT, Prompt, PromptError, Source, app_lists,
        default_lists, name_of, names, resolve, route,
    };
    use crate::files::tests::scratch_dir;
    use crate::settings::{AppId, PromptName, Settings, parse};

    fn name(value: &str) -> PromptName {
        PromptName::try_from(value.to_owned()).unwrap()
    }

    #[test]
    fn the_prompt_names_the_delimiters_of_the_correction_request() {
        assert!(NATURAL_LIGHT.text.contains("`<transcript>` line"));
        assert!(NATURAL_LIGHT.text.contains("`</transcript>` line"));
    }

    #[test]
    fn each_built_in_prompt_has_a_valid_unique_name_and_a_text_with_the_delimiters() {
        let mut seen = Vec::new();
        for prompt in &BUILT_IN {
            assert!(
                PromptName::try_from(prompt.name.to_owned()).is_ok(),
                "{}",
                prompt.name
            );
            assert!(!seen.contains(&prompt.name), "{}", prompt.name);
            seen.push(prompt.name);
            assert!(!prompt.text.trim().is_empty(), "{}", prompt.name);
            assert!(
                prompt.text.contains("`<transcript>` line"),
                "{}",
                prompt.name
            );
            assert!(
                prompt.text.contains("`</transcript>` line"),
                "{}",
                prompt.name
            );
        }
        assert_eq!(seen, ["natural-light", "code"]);
    }

    #[test]
    fn each_built_in_name_selects_its_built_in_prompt_without_a_file() {
        let dir = scratch_dir("prompt-built-ins");
        for built_in in &BUILT_IN {
            let prompt = resolve(&dir, &name(built_in.name)).unwrap();

            assert_eq!(prompt, built_in.to_prompt(), "{}", built_in.name);
            assert_eq!(prompt.source, Source::BuiltIn);
        }
        fs::remove_dir_all(&dir).unwrap();
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

        assert_eq!(
            listed(&dir),
            ["Mail", "a_b-2", "code", "dev", "natural-light"]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn without_the_folder_the_names_are_the_built_in_names() {
        let dir = scratch_dir("prompt-names-missing");

        assert_eq!(listed(&dir.join("absent")), ["code", "natural-light"]);
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

    fn ids(list: &[AppId]) -> Vec<&str> {
        list.iter().map(AppId::as_str).collect()
    }

    /// The default lists when every built-in application is installed.
    fn all_installed() -> AppLists {
        default_lists(|_| true)
    }

    #[test]
    fn each_built_in_application_is_a_valid_bundle_identifier_in_one_list() {
        let mut seen = Vec::new();
        for prompt in &BUILT_IN {
            for &app in prompt.apps {
                assert!(AppId::try_from(app.to_owned()).is_ok(), "{app}");
                assert!(!seen.contains(&app), "{app}");
                seen.push(app);
            }
        }
        assert_eq!(seen.len(), 15);
        assert!(CODE.apps.contains(&"com.openai.codex"));
        assert!(CODE.apps.contains(&"com.anthropic.claudefordesktop"));
    }

    #[test]
    fn the_default_lists_keep_only_the_installed_applications_and_each_built_in_prompt() {
        let lists = default_lists(|id| id == "com.mitchellh.ghostty" || id == "com.apple.Notes");

        assert_eq!(lists.len(), BUILT_IN.len());
        assert_eq!(ids(&lists[&name("code")]), ["com.mitchellh.ghostty"]);
        assert_eq!(ids(&lists[&name("natural-light")]), [] as [&str; 0]);

        let all = all_installed();
        for prompt in &BUILT_IN {
            assert_eq!(
                ids(&all[&name(prompt.name)]),
                prompt.apps,
                "{}",
                prompt.name
            );
        }
    }

    #[test]
    fn without_the_apps_table_the_lists_are_the_defaults() {
        let defaults = default_lists(|id| id == "com.apple.Terminal");

        assert_eq!(app_lists(&Settings::default(), &defaults), defaults);
        assert_eq!(
            app_lists(&parse("[refine]\nenabled = true\n").unwrap(), &defaults),
            defaults
        );
    }

    #[test]
    fn with_the_apps_table_the_lists_are_exactly_the_table() {
        let settings = parse(
            "[refine.apps]\nmail = [\"com.mitchellh.ghostty\", \"com.apple.mail\"]\ncode = []\n",
        )
        .unwrap();

        let lists = app_lists(&settings, &all_installed());

        assert_eq!(lists.len(), 2);
        assert_eq!(
            ids(&lists[&name("mail")]),
            ["com.mitchellh.ghostty", "com.apple.mail"]
        );
        assert_eq!(ids(&lists[&name("code")]), [] as [&str; 0]);
        assert!(!lists.contains_key(&name("natural-light")));
        assert_eq!(route(&lists, Some("com.apple.Terminal")), None);
        assert_eq!(route(&lists, Some("net.whatsapp.WhatsApp")), None);

        let empty = parse("[refine.apps]\n").unwrap();
        assert_eq!(app_lists(&empty, &all_installed()), AppLists::new());
    }

    #[test]
    fn the_route_is_the_prompt_whose_list_contains_the_application() {
        let settings = parse(
            "[refine.apps]\nnatural-light = [\"com.mitchellh.ghostty\"]\nmail = [\"com.apple.mail\"]\ncode = [\"com.apple.Terminal\"]\n",
        )
        .unwrap();
        let lists = app_lists(&settings, &all_installed());

        for (app, prompt) in [
            (Some("com.mitchellh.ghostty"), Some("natural-light")),
            (Some("com.apple.Terminal"), Some("code")),
            (Some("com.apple.mail"), Some("mail")),
            (Some("com.apple.MobileSMS"), None),
            (Some("com.apple.terminal"), None),
            (Some(""), None),
            (None, None),
        ] {
            assert_eq!(
                route(&lists, app).map(PromptName::as_str),
                prompt,
                "{app:?}"
            );
        }
    }
}
