//! The dictionary of the user: the written forms of names and terms.

use std::collections::HashSet;
use std::fmt;

/// One entry of the dictionary.
///
/// `Debug` shows only whether the entry has a heard form, never its text.
pub struct DictionaryEntry {
    /// The form that the speech model writes instead, if the user knows it.
    pub heard: Option<String>,
    /// The form that the text must contain.
    pub written: String,
}

impl fmt::Debug for DictionaryEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DictionaryEntry")
            .field("heard", &self.heard.is_some())
            .finish_non_exhaustive()
    }
}

/// The entries of the dictionary, in file order.
///
/// `Debug` shows only the number of entries, never their text.
pub struct Dictionary {
    entries: Vec<DictionaryEntry>,
}

impl Dictionary {
    /// Keeps the entries in the given order.
    #[must_use]
    pub fn new(entries: Vec<DictionaryEntry>) -> Self {
        Self { entries }
    }

    /// Returns the written forms for `TranscriptionOptions::vocabulary`, in entry order.
    /// A written form that occurs again is not repeated.
    #[must_use]
    pub fn written_forms(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.entries
            .iter()
            .map(|entry| entry.written.as_str())
            .filter(|written| seen.insert(*written))
            .map(str::to_owned)
            .collect()
    }
}

impl fmt::Debug for Dictionary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dictionary")
            .field("entries", &self.entries.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{Dictionary, DictionaryEntry};

    fn entry(heard: Option<&str>, written: &str) -> DictionaryEntry {
        DictionaryEntry {
            heard: heard.map(str::to_owned),
            written: written.to_owned(),
        }
    }

    #[test]
    fn written_forms_keep_the_entry_order_without_repeats() {
        let dictionary = Dictionary::new(vec![
            entry(None, "objc2"),
            entry(Some("rust fmt"), "rustfmt"),
            entry(Some("obj c 2"), "objc2"),
            entry(None, "Voxtral"),
            entry(Some("rust format"), "rustfmt"),
        ]);

        assert_eq!(dictionary.written_forms(), ["objc2", "rustfmt", "Voxtral"]);
    }

    #[test]
    fn an_empty_dictionary_has_no_written_forms() {
        assert_eq!(
            Dictionary::new(Vec::new()).written_forms(),
            [] as [String; 0]
        );
    }

    #[test]
    fn debug_shows_the_number_of_entries_but_not_their_text() {
        let dictionary = Dictionary::new(vec![
            entry(None, "objc2"),
            entry(Some("rust fmt"), "rustfmt"),
        ]);

        let debug = format!("{dictionary:?}");

        assert_eq!(debug, "Dictionary { entries: 2 }");
    }

    #[test]
    fn the_debug_of_an_entry_shows_no_text() {
        let with_heard = entry(Some("private heard"), "private written");
        let without_heard = entry(None, "private written");

        assert_eq!(
            format!("{with_heard:?}"),
            "DictionaryEntry { heard: true, .. }"
        );
        assert_eq!(
            format!("{without_heard:?}"),
            "DictionaryEntry { heard: false, .. }"
        );
    }
}
