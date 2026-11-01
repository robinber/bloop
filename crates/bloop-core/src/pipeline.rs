//! Text pipeline: turns the complete transcript of a job into its result event.

use crate::domain::{FinalText, Generation, Transcript};
use crate::session::Event;

/// Returns `NothingHeard` for a transcript that contains only whitespace,
/// and `TextReady` with the raw transcript otherwise.
#[must_use]
pub fn run(generation: Generation, transcript: Transcript) -> Event {
    if transcript.as_str().trim().is_empty() {
        Event::NothingHeard(generation)
    } else {
        Event::TextReady(generation, FinalText::from(transcript))
    }
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::domain::{Generation, Transcript};
    use crate::session::Event;

    #[test]
    fn a_transcript_without_words_is_nothing_heard() {
        for text in ["", " ", "\n\t "] {
            let event = run(Generation(1), Transcript::new(text.to_owned()));

            assert!(
                matches!(event, Event::NothingHeard(Generation(1))),
                "{text:?}: {event:?}"
            );
        }
    }

    #[test]
    fn the_final_text_is_the_raw_transcript() {
        let event = run(Generation(1), Transcript::new("Bonjour à tous.".to_owned()));

        let Event::TextReady(Generation(1), text) = event else {
            panic!("{event:?}");
        };
        assert_eq!(text.as_str(), "Bonjour à tous.");
    }
}
