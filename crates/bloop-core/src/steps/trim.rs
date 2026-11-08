//! Removal of the line breaks at the start and at the end of the text.

use std::future;

use crate::error::StepError;
use crate::ports::ai::{BoxFuture, StepContext, TextStep};

/// Removes `\n` and `\r` at the start and at the end of the text. It keeps the spaces and the other characters.
#[derive(Debug, Clone, Copy, Default)]
pub struct Trim;

impl TextStep for Trim {
    fn name(&self) -> &'static str {
        "trim"
    }

    fn apply<'a>(
        &'a self,
        text: &'a str,
        _context: &'a StepContext,
    ) -> BoxFuture<'a, Result<String, StepError>> {
        let trimmed = text.trim_matches(['\n', '\r']).to_owned();
        Box::pin(future::ready(Ok(trimmed)))
    }
}

#[cfg(test)]
mod tests {
    use super::Trim;
    use crate::fakes::{context, resolve};
    use crate::ports::ai::TextStep;

    fn trim(text: &str) -> String {
        resolve(Trim.apply(text, &context())).unwrap()
    }

    #[test]
    fn line_breaks_at_the_start_and_at_the_end_go() {
        assert_eq!(trim("\n\r\nBonjour à tous.\r\n\n"), "Bonjour à tous.");
        assert_eq!(trim("\n\n"), "");
    }

    #[test]
    fn spaces_and_inner_line_breaks_stay() {
        for text in [
            " Bonjour ",
            "\tBonjour\t",
            "\u{a0}Bonjour\u{a0}",
            "Bonjour\n\nà tous",
            "",
        ] {
            assert_eq!(trim(text), text, "{text:?}");
        }
        assert_eq!(trim("\n Bonjour \n"), " Bonjour ");
    }

    #[test]
    fn the_step_name_is_trim() {
        assert_eq!(Trim.name(), "trim");
    }
}
