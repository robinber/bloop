//! Mistral provider.

pub(crate) mod chat;
pub(crate) mod transcribe;

/// The `SecretStore` account of the Mistral API key.
pub const ACCOUNT: &str = "mistral";

/// The `provider` value of the Mistral tables in the settings file.
pub(crate) const PROVIDER: &str = "mistral";

/// The settings of the Voxtral realtime transcription.
/// A missing key in the settings file has its default value.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TranscribeConfig {
    /// The model identifier, for example `voxtral-mini-transcribe-realtime-3-0`.
    /// It goes into the query of the WebSocket URL, so `model_id` checks its characters.
    #[serde(deserialize_with = "model_id")]
    pub model: String,
    /// The balance between delay and accuracy, in milliseconds. The provider checks the value.
    pub target_streaming_delay_ms: u32,
}

impl Default for TranscribeConfig {
    fn default() -> Self {
        Self {
            model: "voxtral-mini-transcribe-realtime-3-0".to_owned(),
            target_streaming_delay_ms: 480,
        }
    }
}

/// Reads a model identifier that a URL query can carry unchanged: 1 to 255 characters from `A-Z`, `a-z`, `0-9`,
/// `.`, `_`, `:`, and `-`.
fn model_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let model = <String as serde::Deserialize>::deserialize(deserializer)?;
    let allowed =
        |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-');
    if (1..=255).contains(&model.len()) && model.bytes().all(allowed) {
        Ok(model)
    } else {
        Err(serde::de::Error::custom(
            "the model is not 1 to 255 characters from A-Z a-z 0-9 . _ : -",
        ))
    }
}

/// The settings of the Mistral chat completions.
/// A missing key in the settings file has its default value.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChatConfig {
    /// The model identifier, for example `mistral-medium-latest`.
    pub model: String,
    /// The models that the user can select.
    pub models: Vec<String>,
    /// The `reasoning_effort` value, for example `none`. `None` omits the field. The provider checks the value.
    pub reasoning_effort: Option<String>,
    /// The sampling temperature. The settings file cannot change it.
    #[serde(skip)]
    pub temperature: f64,
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            model: "mistral-medium-latest".to_owned(),
            models: vec![
                "mistral-medium-latest".to_owned(),
                "mistral-small-latest".to_owned(),
            ],
            reasoning_effort: Some("none".to_owned()),
            temperature: 0.0,
        }
    }
}

/// The commented `[transcriber]` table of the settings template. It gives the default values.
pub(crate) const TRANSCRIBER_TEMPLATE: &str = r#"[transcriber]
provider = "mistral"                            # The speech provider.
model = "voxtral-mini-transcribe-realtime-3-0"  # The realtime transcription model.
target_streaming_delay_ms = 480                 # The balance between delay and accuracy, in milliseconds.
"#;

/// The commented `[chat]` table of the settings template. It gives the default values.
pub(crate) const CHAT_TEMPLATE: &str = r#"[chat]
provider = "mistral"                                        # The correction provider.
model = "mistral-medium-latest"                             # The correction model.
models = ["mistral-medium-latest", "mistral-small-latest"]  # The correction models to select from.
reasoning_effort = "none"                                   # The reasoning effort of the model.
"#;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::TranscribeConfig;

    fn model(value: &str) -> Option<String> {
        serde_json::from_value::<TranscribeConfig>(json!({ "model": value }))
            .ok()
            .map(|config| config.model)
    }

    #[test]
    fn the_transcription_model_accepts_1_to_255_letters_digits_dots_underscores_colons_and_hyphens()
    {
        let longest = "a".repeat(255);
        for value in [
            "voxtral-mini-transcribe-realtime-3-0",
            "a",
            "ft:voxtral_mini.2",
            longest.as_str(),
        ] {
            assert_eq!(model(value).as_deref(), Some(value));
        }
    }

    #[test]
    fn a_transcription_model_that_would_change_the_url_is_rejected() {
        let too_long = "a".repeat(256);
        for value in [
            "",
            "voxtral mini",
            "voxtral&key=x",
            "voxtral#x",
            "voxtral?x",
            "voxtral/x",
            "voxtral%20x",
            "voxtral=x",
            "é",
            too_long.as_str(),
        ] {
            assert_eq!(model(value), None, "{value}");
        }
    }
}
