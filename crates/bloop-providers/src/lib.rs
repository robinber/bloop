//! AI provider adapters of Bloop, with one module for each vendor.
//!
//! `bloop-app` selects a provider with [`TranscriberConfig`] and builds it with [`build_transcriber`].

#![forbid(unsafe_code)]

#[cfg(feature = "mistral")]
pub mod mistral;

use bloop_core::ports::ai::Transcriber;
use secrecy::SecretString;

/// The transcription provider and its settings.
#[derive(Debug)]
pub enum TranscriberConfig {
    /// Voxtral realtime transcription.
    #[cfg(feature = "mistral")]
    Mistral(mistral::TranscribeConfig),
}

impl TranscriberConfig {
    /// The `SecretStore` account of the API key of the provider.
    #[must_use]
    pub fn account(&self) -> &'static str {
        match *self {
            #[cfg(feature = "mistral")]
            Self::Mistral(_) => mistral::ACCOUNT,
        }
    }
}

#[cfg(feature = "mistral")]
impl Default for TranscriberConfig {
    fn default() -> Self {
        Self::Mistral(mistral::TranscribeConfig::default())
    }
}

/// Builds the transcriber that `config` selects. `key` is the API key of its provider.
#[must_use]
#[cfg_attr(
    not(feature = "mistral"),
    expect(
        unused_variables,
        clippy::needless_pass_by_value,
        reason = "without a provider feature, no config exists"
    )
)]
pub fn build_transcriber(config: TranscriberConfig, key: SecretString) -> Box<dyn Transcriber> {
    match config {
        #[cfg(feature = "mistral")]
        TranscriberConfig::Mistral(config) => {
            Box::new(mistral::transcribe::Transcriber::new(key, config))
        }
    }
}

#[cfg(all(test, feature = "mistral"))]
mod tests {
    use super::TranscriberConfig;

    const KEY: &str = "test-key";

    #[test]
    fn the_default_transcriber_is_voxtral_with_the_current_settings() {
        let config = TranscriberConfig::default();

        assert_eq!(config.account(), "mistral");
        let TranscriberConfig::Mistral(mistral) = &config;
        assert_eq!(mistral.model, "voxtral-mini-transcribe-realtime-3-0");
        assert_eq!(mistral.target_streaming_delay_ms, 480);
    }

    #[test]
    fn the_debug_of_the_configs_shows_no_key() {
        let transcriber = format!("{:?}", TranscriberConfig::default());

        assert!(transcriber.starts_with("Mistral(TranscribeConfig {"));
        assert!(!transcriber.contains(KEY));
    }
}
