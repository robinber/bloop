//! French typography after the correction: normal spaces around punctuation and straight apostrophes.

use std::future;

use crate::error::StepError;
use crate::ports::ai::{BoxFuture, TextStep};

/// Replaces a no-break space (U+00A0) or a narrow no-break space (U+202F) with a normal space
/// when it comes immediately before `?`, `!`, `:`, `;`, or `»`, or immediately after `«`.
/// Replaces `’` (U+2019) with `'`.
///
/// The step changes no other character. It does not add a space, and it does not change line breaks.
#[derive(Debug, Clone, Copy, Default)]
pub struct Typography;

impl TextStep for Typography {
    fn name(&self) -> &'static str {
        "typography"
    }

    fn apply<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<String, StepError>> {
        Box::pin(future::ready(Ok(normalize(text))))
    }
}

fn normalize(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut previous = None;
    let mut chars = text.chars().peekable();
    while let Some(current) = chars.next() {
        let replacement = match current {
            '\u{2019}' => '\'',
            '\u{a0}' | '\u{202f}'
                if previous == Some('«')
                    || chars
                        .peek()
                        .is_some_and(|next| matches!(next, '?' | '!' | ':' | ';' | '»')) =>
            {
                ' '
            }
            other => other,
        };
        normalized.push(replacement);
        previous = Some(current);
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::Typography;
    use crate::fakes::resolve;
    use crate::ports::ai::TextStep;

    fn typography(text: &str) -> String {
        resolve(Typography.apply(text)).unwrap()
    }

    #[test]
    fn a_no_break_space_before_the_high_punctuation_becomes_a_normal_space() {
        for space in ['\u{a0}', '\u{202f}'] {
            for mark in ['?', '!', ':', ';', '»'] {
                let text = format!("Bonjour{space}{mark}");

                assert_eq!(typography(&text), format!("Bonjour {mark}"), "{text:?}");
            }
        }
    }

    #[test]
    fn a_no_break_space_after_an_opening_guillemet_becomes_a_normal_space() {
        for space in ['\u{a0}', '\u{202f}'] {
            let text = format!("«{space}Bonjour");

            assert_eq!(typography(&text), "« Bonjour", "{text:?}");
        }
    }

    #[test]
    fn mixed_spaces_in_one_text_all_become_normal_spaces() {
        let text = "Il dit\u{a0}: «\u{202f}Vraiment\u{a0}?\u{202f}» Oui\u{202f}!";

        assert_eq!(typography(text), "Il dit : « Vraiment ? » Oui !");
    }

    #[test]
    fn other_no_break_spaces_stay() {
        for text in [
            "10\u{a0}000 euros",
            "M.\u{a0}Dupont",
            "3\u{202f}h",
            "fin\u{a0}",
            "\u{a0}début",
            "Bonjour\u{a0}.",
            "Bonjour\u{a0}«",
            "»\u{a0}Bonjour",
        ] {
            assert_eq!(typography(text), text, "{text:?}");
        }
    }

    #[test]
    fn only_the_space_next_to_the_mark_changes() {
        assert_eq!(typography("Bonjour\u{a0}\u{a0}?"), "Bonjour\u{a0} ?");
        assert_eq!(typography("«\u{202f}\u{202f}Bonjour"), "« \u{202f}Bonjour");
    }

    #[test]
    fn the_step_inserts_no_space() {
        for text in [
            "Bonjour?",
            "Bonjour!",
            "Note:",
            "Fin;",
            "«Bonjour»",
            "Bonjour ?",
        ] {
            assert_eq!(typography(text), text);
        }
    }

    #[test]
    fn the_typographic_apostrophe_becomes_straight() {
        assert_eq!(typography("l’heure d’été"), "l'heure d'été");
        assert_eq!(typography("‘cité’"), "‘cité'");
    }

    #[test]
    fn line_breaks_stay() {
        for text in ["\nBonjour\n", "Bonjour\n?", "«\nBonjour", "a\r\n\r\nb"] {
            assert_eq!(typography(text), text, "{text:?}");
        }
        assert_eq!(
            typography("Ligne\u{a0}:\nsuite\u{a0}!\n"),
            "Ligne :\nsuite !\n"
        );
    }

    #[test]
    fn the_step_name_is_typography() {
        assert_eq!(Typography.name(), "typography");
    }
}
