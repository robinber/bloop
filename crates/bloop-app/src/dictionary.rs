//! The dictionary file `~/Library/Application Support/Bloop/dictionary.toml`.
//!
//! Bloop reads the file one time at start. A missing file gives an empty dictionary.
//! The errors and the log never contain the file content, because a parser message can quote an entry.

use std::fs;
use std::io;
use std::path::Path;

use bloop_core::dictionary::{Dictionary, DictionaryEntry};
use bloop_core::error::AppError;
use serde::Deserialize;
use toml::Spanned;

/// The path of the file in the home folder.
const FILE: &str = "Library/Application Support/Bloop/dictionary.toml";

/// Why the dictionary file is invalid. It contains no file content.
#[derive(Debug, PartialEq, Eq)]
enum Invalid {
    /// The home folder is unknown.
    NoHome,
    /// The file exists, but Bloop cannot read it as UTF-8 text.
    Unreadable(io::ErrorKind),
    /// The parser rejected the file: syntax, value type, missing key, or unknown key.
    /// `line` starts at 1. It is `None` when the parser gives no position.
    Parse { line: Option<usize> },
    /// The `written` value of an entry is empty or contains only whitespace.
    EmptyWritten { line: Option<usize> },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    entry: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    heard: Option<String>,
    written: Spanned<String>,
}

/// Reads the dictionary file and logs the number of entries or the error, never the entries.
///
/// # Errors
///
/// `InvalidSettings` when the file exists but is not a valid dictionary.
pub(crate) fn load() -> Result<Dictionary, AppError> {
    let result = std::env::home_dir()
        .ok_or(Invalid::NoHome)
        .and_then(|home| read(&home.join(FILE)));
    match result {
        Ok(Some(dictionary)) => {
            tracing::info!(?dictionary, "the dictionary is ready");
            Ok(dictionary)
        }
        Ok(None) => {
            tracing::info!("the dictionary file does not exist, the dictionary is empty");
            Ok(Dictionary::new(Vec::new()))
        }
        Err(error) => {
            tracing::error!(
                ?error,
                "the dictionary file is invalid, each dictation fails until the file is fixed and Bloop starts again"
            );
            Err(AppError::InvalidSettings)
        }
    }
}

/// Reads and parses the file at `path`. Returns `None` when the file does not exist.
fn read(path: &Path) -> Result<Option<Dictionary>, Invalid> {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Invalid::Unreadable(error.kind())),
    }
}

fn parse(text: &str) -> Result<Dictionary, Invalid> {
    let file: File = toml::from_str(text).map_err(|error| Invalid::Parse {
        line: error.span().and_then(|span| line(text, span.start)),
    })?;
    file.entry
        .into_iter()
        .map(|entry| {
            let start = entry.written.span().start;
            let written = entry.written.into_inner();
            if written.trim().is_empty() {
                return Err(Invalid::EmptyWritten {
                    line: line(text, start),
                });
            }
            Ok(DictionaryEntry {
                heard: entry.heard,
                written,
            })
        })
        .collect::<Result<_, _>>()
        .map(Dictionary::new)
}

/// Returns the line, from 1, that contains the byte at `offset` of `text`.
fn line(text: &str, offset: usize) -> Option<usize> {
    let before = text.as_bytes().get(..offset)?;
    Some(before.split(|&byte| byte == b'\n').count())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{Invalid, line, parse, read};

    const VALID: &str = r#"
[[entry]]
written = "objc2"

[[entry]]
heard = "comme Tabby"
written = "comme Taby"
"#;

    fn scratch_dir(test: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "bloop-dictionary-{test}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir(&dir).unwrap();
        dir
    }

    fn parse_error(text: &str) -> Invalid {
        parse(text).map(|_| ()).unwrap_err()
    }

    #[test]
    fn a_valid_file_gives_its_written_forms_in_order() {
        let dictionary = parse(VALID).unwrap();

        assert_eq!(dictionary.written_forms(), ["objc2", "comme Taby"]);
        assert_eq!(format!("{dictionary:?}"), "Dictionary { entries: 2 }");
    }

    #[test]
    fn an_entry_without_a_heard_form_is_valid() {
        let dictionary = parse("[[entry]]\nwritten = \"Voxtral\"\n").unwrap();

        assert_eq!(dictionary.written_forms(), ["Voxtral"]);
    }

    #[test]
    fn a_file_without_entries_gives_an_empty_dictionary() {
        assert_eq!(parse("").unwrap().written_forms(), [] as [String; 0]);
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let dir = scratch_dir("missing");

        assert!(read(&dir.join("dictionary.toml")).unwrap().is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_existing_file_is_parsed() {
        let dir = scratch_dir("existing");
        let path = dir.join("dictionary.toml");
        fs::write(&path, VALID).unwrap();

        let dictionary = read(&path).unwrap().unwrap();

        assert_eq!(dictionary.written_forms(), ["objc2", "comme Taby"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_cannot_be_read_is_an_error() {
        let dir = scratch_dir("unreadable");

        assert!(matches!(
            read(&dir).map(|_| ()),
            Err(Invalid::Unreadable(_))
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_syntax_error_gives_its_line() {
        let text = "[[entry]]\nwritten = \"objc2\"\n\n[[entry]]\nwritten = \"comme Taby\n";

        assert_eq!(parse_error(text), Invalid::Parse { line: Some(5) });
    }

    #[test]
    fn an_unknown_key_is_an_error_without_the_file_content() {
        let text = "[[entry]]\nwritten = \"objc2\"\nspoken = \"secret\"\n";

        let error = parse_error(text);

        assert_eq!(error, Invalid::Parse { line: Some(3) });
        assert_eq!(format!("{error:?}"), "Parse { line: Some(3) }");
    }

    #[test]
    fn an_unknown_table_is_an_error() {
        let text = "[[entry]]\nwritten = \"objc2\"\n\n[[entries]]\nwritten = \"Voxtral\"\n";

        assert_eq!(parse_error(text), Invalid::Parse { line: Some(4) });
    }

    #[test]
    fn a_written_form_of_another_type_is_an_error() {
        let text = "[[entry]]\nwritten = 2\n";

        assert_eq!(parse_error(text), Invalid::Parse { line: Some(2) });
    }

    #[test]
    fn an_entry_without_a_written_form_is_an_error() {
        let text = "[[entry]]\nwritten = \"objc2\"\n\n[[entry]]\nheard = \"comme Tabby\"\n";

        assert_eq!(parse_error(text), Invalid::Parse { line: Some(4) });
    }

    #[test]
    fn an_empty_written_form_is_an_error_with_its_line() {
        for written in ["\"\"", "\" \t \""] {
            let text =
                format!("[[entry]]\nwritten = \"objc2\"\n\n[[entry]]\nwritten = {written}\n");

            assert_eq!(
                parse_error(&text),
                Invalid::EmptyWritten { line: Some(5) },
                "{written}"
            );
        }
    }

    #[test]
    fn the_line_counts_the_line_breaks_before_the_offset() {
        let text = "a\nbc\nd";

        assert_eq!(line(text, 0), Some(1));
        assert_eq!(line(text, 1), Some(1));
        assert_eq!(line(text, 2), Some(2));
        assert_eq!(line(text, 5), Some(3));
        assert_eq!(line(text, 6), Some(3));
        assert_eq!(line(text, 7), None);
    }
}
