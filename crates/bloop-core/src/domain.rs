//! Domain values shared by the state machine and the adapters.

use std::fmt;

/// Identifies one session. The state machine ignores events for an unknown generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generation(pub(crate) u64);

/// The text that Bloop pastes: the corrected text or the raw transcript.
///
/// `Debug` shows only the number of characters, never the text.
#[derive(Clone, PartialEq, Eq)]
pub struct FinalText(String);

impl FinalText {
    /// Wraps the text that a job produced.
    #[must_use]
    pub fn new(text: String) -> Self {
        Self(text)
    }

    /// Returns the text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for FinalText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FinalText")
            .field("chars", &self.0.chars().count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::FinalText;

    #[test]
    fn debug_shows_the_length_but_not_the_text() {
        let text = FinalText::new("mot de passe secret".to_owned());

        let debug = format!("{text:?}");

        assert_eq!(debug, "FinalText { chars: 19 }");
        assert!(!debug.contains("secret"));
    }
}
