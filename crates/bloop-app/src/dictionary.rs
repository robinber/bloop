//! The dictionary file `~/Library/Application Support/Bloop/dictionary.toml`.
//!
//! The settings hub reads the file at each dictation key press and each time the status menu opens.
//! A missing file gives an empty dictionary.
//! When the file does not exist at start, Bloop writes a commented empty dictionary.
//! The errors and the log never contain the file content, because a parser message can quote an entry.

use std::io;

use bloop_core::dictionary::{Dictionary, DictionaryEntry};
use serde::Deserialize;
use toml::Spanned;

use crate::files::FileError;

/// The size limit of `dictionary.toml`, in bytes.
pub(crate) const LIMIT: u64 = 256 * 1024;

/// The commented empty dictionary that Bloop writes when `dictionary.toml` does not exist.
pub(crate) const TEMPLATE: &str = r##"# Bloop dictionary: the names and terms that the dictation must spell correctly.
# Each entry gives one written form. Bloop gives the written forms to the speech provider.
# A change applies from the next dictation. To use an example, remove the "#" at the start of its lines.

# [[entry]]
# written = "Voxtral"

# [[entry]]
# heard = "obj c 2"           # Optional: the form that Bloop writes now. It has no effect for now.
# written = "objc2"
"##;

/// Why the dictionary file is invalid. It contains no file content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Invalid {
    /// The file is larger than [`LIMIT`].
    TooLarge,
    /// The file exists, but Bloop cannot read it as UTF-8 text.
    Unreadable(io::ErrorKind),
    /// The parser rejected the file: syntax, value type, missing key, or unknown key.
    /// `line` starts at 1. It is `None` when the parser gives no position.
    Parse { line: Option<usize> },
    /// The `written` value of an entry is empty or contains only whitespace.
    EmptyWritten { line: Option<usize> },
}

impl From<FileError> for Invalid {
    fn from(error: FileError) -> Self {
        match error {
            FileError::TooLarge => Self::TooLarge,
            FileError::Unreadable(kind) => Self::Unreadable(kind),
        }
    }
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

/// Returns the dictionary of a valid file. The error gives the line, never the entry or the parser message.
pub(crate) fn parse(text: &str) -> Result<Dictionary, Invalid> {
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
pub(crate) fn line(text: &str, offset: usize) -> Option<usize> {
    let before = text.as_bytes().get(..offset)?;
    Some(before.split(|&byte| byte == b'\n').count())
}

#[cfg(test)]
mod tests {
    use super::{Invalid, TEMPLATE, line, parse};

    const VALID: &str = r#"
[[entry]]
written = "objc2"

[[entry]]
heard = "comme Tabby"
written = "comme Taby"
"#;

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
    fn the_template_is_an_empty_dictionary() {
        assert_eq!(parse(TEMPLATE).unwrap().written_forms(), [] as [String; 0]);
    }

    #[test]
    fn the_template_examples_are_valid_entries_without_the_comment_marks() {
        let examples: Vec<&str> = TEMPLATE
            .lines()
            .filter_map(|line| line.strip_prefix("# "))
            .filter(|line| line.starts_with('[') || line.contains(" = "))
            .collect();

        assert_eq!(
            parse(&examples.join("\n")).unwrap().written_forms(),
            ["Voxtral", "objc2"]
        );
    }

    #[test]
    fn a_file_without_entries_gives_an_empty_dictionary() {
        assert_eq!(parse("").unwrap().written_forms(), [] as [String; 0]);
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
