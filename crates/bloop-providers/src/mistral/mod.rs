//! Mistral provider.

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
