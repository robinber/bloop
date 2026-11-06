//! The test set file: the language, the dictionary pairs, and the samples of the correction.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use bloop_core::dictionary::DictionaryEntry;
use serde::Deserialize;
use toml::Spanned;

/// The categories of the samples. A sample with another category is an error.
pub(crate) const CATEGORIES: [&str; 12] = [
    "question",
    "negation",
    "number",
    "name",
    "technical",
    "spoken-correction",
    "hesitation",
    "instruction",
    "long",
    "punctuation",
    "dictionary-fits",
    "dictionary-does-not-fit",
];

/// A valid test set, with the samples in file order.
pub(crate) struct TestSet {
    /// The language of the dictations, for example `fr`.
    pub(crate) language: String,
    /// The dictionary pairs. Each entry has a heard form.
    pub(crate) pairs: Vec<DictionaryEntry>,
    pub(crate) samples: Vec<Sample>,
}

/// One raw transcript of the set.
pub(crate) struct Sample {
    /// A name that no other sample of the set has.
    pub(crate) id: String,
    /// One of [`CATEGORIES`].
    pub(crate) category: &'static str,
    /// The raw transcript. It contains more than whitespace.
    pub(crate) raw: String,
    /// What a reviewer checks.
    pub(crate) note: Option<String>,
    /// The sample is reserved for the check after a prompt change.
    pub(crate) validation: bool,
}

/// The part of the set that a run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Split {
    /// The samples that are not reserved for validation.
    Dev,
    /// The samples reserved for validation.
    Validation,
}

impl Split {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Validation => "validation",
        }
    }
}

impl Sample {
    pub(crate) fn split(&self) -> Split {
        if self.validation {
            Split::Validation
        } else {
            Split::Dev
        }
    }
}

impl TestSet {
    /// Returns the samples of `split`, in file order.
    pub(crate) fn samples(&self, split: Split) -> Vec<&Sample> {
        self.samples
            .iter()
            .filter(|sample| sample.split() == split)
            .collect()
    }
}

/// Why the test set is invalid. `line` starts at 1. It is `None` when the parser gives no position.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Invalid {
    /// The parser rejected the file: syntax, value type, missing key, or unknown key.
    Parse {
        line: Option<usize>,
        message: String,
    },
    /// A text value is empty or contains only whitespace.
    Blank {
        key: &'static str,
        line: Option<usize>,
    },
    UnknownCategory {
        line: Option<usize>,
    },
    DuplicateId {
        line: Option<usize>,
    },
    NoSample,
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let line = match self {
            Self::Parse { line, .. }
            | Self::Blank { line, .. }
            | Self::UnknownCategory { line }
            | Self::DuplicateId { line } => *line,
            Self::NoSample => None,
        };
        if let Some(line) = line {
            write!(f, "line {line}: ")?;
        }
        match self {
            Self::Parse { message, .. } => f.write_str(message),
            Self::Blank { key, .. } => write!(f, "the `{key}` value is empty"),
            Self::UnknownCategory { .. } => {
                write!(f, "unknown category, use one of: {}", CATEGORIES.join(", "))
            }
            Self::DuplicateId { .. } => f.write_str("another sample has the same id"),
            Self::NoSample => f.write_str("the set has no sample"),
        }
    }
}

impl Error for Invalid {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    language: Spanned<String>,
    #[serde(default)]
    dictionary: Vec<Pair>,
    #[serde(default)]
    sample: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    written: Spanned<String>,
    heard: Spanned<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    id: Spanned<String>,
    category: Spanned<String>,
    raw: Spanned<String>,
    note: Option<String>,
    #[serde(default)]
    validation: bool,
}

/// Parses and checks the text of a test set file.
pub(crate) fn parse(text: &str) -> Result<TestSet, Invalid> {
    let file: File = toml::from_str(text).map_err(|error| Invalid::Parse {
        line: error.span().and_then(|span| line(text, span.start)),
        message: error.message().to_owned(),
    })?;
    let filled = |key, value: Spanned<String>| {
        let line = line(text, value.span().start);
        let value = value.into_inner();
        if value.trim().is_empty() {
            return Err(Invalid::Blank { key, line });
        }
        Ok(value)
    };
    let language = filled("language", file.language)?;
    let pairs = file
        .dictionary
        .into_iter()
        .map(|pair| {
            Ok(DictionaryEntry {
                written: filled("written", pair.written)?,
                heard: Some(filled("heard", pair.heard)?),
            })
        })
        .collect::<Result<_, Invalid>>()?;
    let mut ids = HashSet::new();
    let mut samples = Vec::with_capacity(file.sample.len());
    for entry in file.sample {
        let id_line = line(text, entry.id.span().start);
        let category_line = line(text, entry.category.span().start);
        let id = filled("id", entry.id)?;
        let category = filled("category", entry.category)?;
        let raw = filled("raw", entry.raw)?;
        let category = CATEGORIES
            .into_iter()
            .find(|known| *known == category)
            .ok_or(Invalid::UnknownCategory {
                line: category_line,
            })?;
        if !ids.insert(id.clone()) {
            return Err(Invalid::DuplicateId { line: id_line });
        }
        samples.push(Sample {
            id,
            category,
            raw,
            note: entry.note,
            validation: entry.validation,
        });
    }
    if samples.is_empty() {
        return Err(Invalid::NoSample);
    }
    Ok(TestSet {
        language,
        pairs,
        samples,
    })
}

/// Returns the line, from 1, that contains the byte at `offset` of `text`.
fn line(text: &str, offset: usize) -> Option<usize> {
    let before = text.as_bytes().get(..offset)?;
    Some(before.split(|&byte| byte == b'\n').count())
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::collections::HashSet;

    use super::{CATEGORIES, Invalid, Split, TestSet, parse};

    const VALID: &str = r#"language = "fr"

[[dictionary]]
written = "mi amor"
heard = "Mia Moore"

[[sample]]
id = "question-01"
category = "question"
raw = "tu viens ce soir"
note = "Keep the question."

[[sample]]
id = "number-01"
category = "number"
raw = "à 2 heures"
validation = true

[[sample]]
id = "name-01"
category = "name"
raw = "dis à Camille"
"#;

    const REAL: &str = include_str!("../../../../../prompts/test-set-fr.toml");

    fn error(text: &str) -> Invalid {
        parse(text).map(|_| ()).unwrap_err()
    }

    fn ids(set: &TestSet, split: Split) -> Vec<&str> {
        set.samples(split)
            .into_iter()
            .map(|sample| sample.id.as_str())
            .collect()
    }

    #[test]
    fn a_valid_file_gives_its_language_pairs_and_samples_in_order() {
        let set = parse(VALID).unwrap();

        assert_eq!(set.language, "fr");
        assert_eq!(set.pairs.len(), 1);
        assert_eq!(set.pairs[0].written, "mi amor");
        assert_eq!(set.pairs[0].heard.as_deref(), Some("Mia Moore"));
        let [question, number, name] = set.samples.as_slice() else {
            panic!("three samples");
        };
        assert_eq!(question.id, "question-01");
        assert_eq!(question.category, "question");
        assert_eq!(question.raw, "tu viens ce soir");
        assert_eq!(question.note.as_deref(), Some("Keep the question."));
        assert!(!question.validation);
        assert!(number.validation);
        assert_eq!(name.note, None);
    }

    #[test]
    fn the_dev_split_and_the_validation_split_are_separate() {
        let set = parse(VALID).unwrap();

        assert_eq!(ids(&set, Split::Dev), ["question-01", "name-01"]);
        assert_eq!(ids(&set, Split::Validation), ["number-01"]);
    }

    #[test]
    fn a_syntax_error_gives_its_line() {
        let text = VALID.replace("raw = \"à 2 heures\"", "raw = \"à 2 heures");

        assert_matches!(error(&text), Invalid::Parse { line: Some(16), .. });
    }

    #[test]
    fn a_sample_without_a_required_key_is_an_error_with_its_line() {
        for (key, removed) in [
            ("id", "id = \"number-01\"\n"),
            ("category", "category = \"number\"\n"),
            ("raw", "raw = \"à 2 heures\"\n"),
        ] {
            let invalid = error(&VALID.replace(removed, ""));

            assert_matches!(invalid, Invalid::Parse { line: Some(13), .. }, "{key}");
            assert!(invalid.to_string().contains(key), "{key}: {invalid}");
        }
    }

    #[test]
    fn a_file_without_a_language_is_an_error() {
        let text = VALID.replace("language = \"fr\"\n", "");

        assert_matches!(error(&text), Invalid::Parse { .. });
    }

    #[test]
    fn an_unknown_key_is_an_error_with_its_line() {
        let text = VALID.replace("validation = true", "validated = true");

        assert_matches!(error(&text), Invalid::Parse { line: Some(17), .. });
    }

    #[test]
    fn a_duplicate_id_is_an_error_with_the_line_of_the_second_id() {
        let text = VALID.replace("name-01", "question-01");

        assert_eq!(error(&text), Invalid::DuplicateId { line: Some(20) });
    }

    #[test]
    fn an_unknown_category_is_an_error_with_its_line() {
        let text = VALID.replace("category = \"name\"", "category = \"names\"");

        assert_eq!(error(&text), Invalid::UnknownCategory { line: Some(21) });
    }

    #[test]
    fn a_blank_text_value_is_an_error_with_its_line() {
        for (from, to, key, line) in [
            ("raw = \"dis à Camille\"", "raw = \" \\n \"", "raw", 22),
            ("id = \"name-01\"", "id = \"\"", "id", 20),
            ("heard = \"Mia Moore\"", "heard = \"\\t\"", "heard", 5),
            ("language = \"fr\"", "language = \"\"", "language", 1),
        ] {
            let text = VALID.replace(from, to);

            assert_eq!(
                error(&text),
                Invalid::Blank {
                    key,
                    line: Some(line)
                },
                "{key}"
            );
        }
    }

    #[test]
    fn a_set_without_samples_is_an_error() {
        assert_eq!(error("language = \"fr\"\n"), Invalid::NoSample);
    }

    #[test]
    fn the_error_message_gives_the_line() {
        let text = VALID.replace("name-01", "question-01");

        assert_eq!(
            error(&text).to_string(),
            "line 20: another sample has the same id"
        );
    }

    #[test]
    fn the_french_test_set_is_valid_and_covers_each_category() {
        let set = parse(REAL).unwrap();

        assert_eq!(set.language, "fr");
        assert!(!set.pairs.is_empty());
        let mut ids = HashSet::new();
        assert!(set.samples.iter().all(|sample| ids.insert(&sample.id)));
        for category in CATEGORIES {
            assert!(
                set.samples.iter().any(|sample| sample.category == category),
                "{category}"
            );
        }
        assert!(!set.samples(Split::Validation).is_empty());
        assert!(!set.samples(Split::Dev).is_empty());
        assert!(
            set.samples
                .iter()
                .all(|sample| !sample.raw.trim().is_empty())
        );
    }

    #[test]
    fn the_french_test_set_has_the_spoken_correction_of_the_ratio_review() {
        let set = parse(REAL).unwrap();

        assert!(
            set.samples
                .iter()
                .any(|sample| sample.raw == "à 2 heures, non, 3 heures")
        );
    }
}
