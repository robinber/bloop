//! The correction prompts in the binary. The log names a prompt, never its text.

/// A correction prompt: the system message of `LlmRefine`.
pub(crate) struct Prompt {
    /// The name of the prompt in the log: its file name without `.md`.
    pub(crate) name: &'static str,
    /// The instructions to the chat model.
    pub(crate) text: &'static str,
}

/// The default prompt: punctuation, capital letters, hesitations, stutters, and spoken corrections.
pub(crate) const NATURAL_LIGHT: Prompt = Prompt {
    name: "natural-light",
    text: include_str!("../../../prompts/natural-light.md"),
};

#[cfg(test)]
mod tests {
    use super::NATURAL_LIGHT;

    #[test]
    fn the_prompt_names_the_delimiters_of_the_correction_request() {
        assert!(NATURAL_LIGHT.text.contains("`<transcript>` line"));
        assert!(NATURAL_LIGHT.text.contains("`</transcript>` line"));
    }
}
