//! Mistral provider.

pub(crate) mod chat;
pub(crate) mod transcribe;

/// The `SecretStore` account of the Mistral API key.
pub const ACCOUNT: &str = "mistral";

/// The settings of the Voxtral realtime transcription.
#[derive(Debug)]
pub struct TranscribeConfig {
    /// The model identifier, for example `voxtral-mini-transcribe-realtime-3-0`.
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

/// The settings of the Mistral chat completions.
#[derive(Debug)]
pub struct ChatConfig {
    /// The model identifier, for example `mistral-medium-latest`.
    pub model: String,
    /// The `reasoning_effort` value, for example `none`. `None` omits the field. The provider checks the value.
    pub reasoning_effort: Option<String>,
    /// The sampling temperature.
    pub temperature: f64,
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            model: "mistral-medium-latest".to_owned(),
            reasoning_effort: Some("none".to_owned()),
            temperature: 0.0,
        }
    }
}
