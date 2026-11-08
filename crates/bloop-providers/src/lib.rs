//! AI provider adapters of Bloop, with one module for each vendor.
//!
//! `bloop-app` selects a provider with [`TranscriberConfig`] and [`ChatConfig`],
//! and builds it with [`build_transcriber`] and [`build_chat_model`].

#![forbid(unsafe_code)]

#[cfg(feature = "mistral")]
mod http;
#[cfg(feature = "mistral")]
pub mod mistral;

#[cfg(all(test, feature = "mistral"))]
mod test_log;

use std::sync::Arc;
use std::time::Duration;

use bloop_core::error::ProviderError;
use bloop_core::ports::ai::{ChatModel, Transcriber};
use secrecy::SecretString;

/// The transcription provider and its settings.
///
/// In the settings file, the `provider` key selects the variant, in snake case.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
    feature = "mistral",
    derive(serde::Deserialize),
    serde(tag = "provider", rename_all = "snake_case")
)]
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
impl TranscriberConfig {
    /// The commented `[transcriber]` table of the default provider, with its default values, for the settings template.
    #[must_use]
    pub fn template() -> &'static str {
        mistral::TRANSCRIBER_TEMPLATE
    }
}

#[cfg(feature = "mistral")]
impl Default for TranscriberConfig {
    fn default() -> Self {
        Self::Mistral(mistral::TranscribeConfig::default())
    }
}

/// The chat provider and its settings.
///
/// In the settings file, the `provider` key selects the variant, in snake case.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
    feature = "mistral",
    derive(serde::Deserialize),
    serde(tag = "provider", rename_all = "snake_case")
)]
pub enum ChatConfig {
    /// Mistral chat completions.
    #[cfg(feature = "mistral")]
    Mistral(mistral::ChatConfig),
}

impl ChatConfig {
    /// The `SecretStore` account of the API key of the provider.
    #[must_use]
    pub fn account(&self) -> &'static str {
        match *self {
            #[cfg(feature = "mistral")]
            Self::Mistral(_) => mistral::ACCOUNT,
        }
    }

    /// The `provider` value of the `[chat]` table that selects this provider in the settings file.
    #[must_use]
    pub fn provider(&self) -> &'static str {
        match *self {
            #[cfg(feature = "mistral")]
            Self::Mistral(_) => mistral::PROVIDER,
        }
    }

    /// The model identifier.
    #[must_use]
    pub fn model(&self) -> &str {
        match *self {
            #[cfg(feature = "mistral")]
            Self::Mistral(ref config) => &config.model,
        }
    }

    /// The models that the user can select in the status menu.
    #[must_use]
    pub fn models(&self) -> &[String] {
        match *self {
            #[cfg(feature = "mistral")]
            Self::Mistral(ref config) => &config.models,
        }
    }

    /// Replaces the model identifier and keeps the other settings. The provider API rejects an unknown model at the first request.
    #[must_use]
    #[cfg_attr(
        not(feature = "mistral"),
        expect(
            unused_variables,
            clippy::needless_pass_by_value,
            reason = "without a provider feature, no config exists"
        )
    )]
    pub fn with_model(self, model: String) -> Self {
        match self {
            #[cfg(feature = "mistral")]
            Self::Mistral(config) => Self::Mistral(mistral::ChatConfig { model, ..config }),
        }
    }
}

#[cfg(feature = "mistral")]
impl ChatConfig {
    /// The commented `[chat]` table of the default provider, with its default values, for the settings template.
    #[must_use]
    pub fn template() -> &'static str {
        mistral::CHAT_TEMPLATE
    }
}

#[cfg(feature = "mistral")]
impl Default for ChatConfig {
    fn default() -> Self {
        Self::Mistral(mistral::ChatConfig::default())
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

/// Builds the chat model that `config` selects. `key` is the API key of its provider.
///
/// The model applies `limit` to each request. An expired limit returns `ProviderError::Timeout`.
///
/// # Errors
///
/// `ProviderError::Network` when the HTTP client does not start, for example because the TLS setup failed.
#[cfg_attr(
    not(feature = "mistral"),
    expect(
        unused_variables,
        clippy::needless_pass_by_value,
        reason = "without a provider feature, no config exists"
    )
)]
pub fn build_chat_model(
    config: ChatConfig,
    key: SecretString,
    limit: Duration,
) -> Result<Arc<dyn ChatModel>, ProviderError> {
    match config {
        #[cfg(feature = "mistral")]
        ChatConfig::Mistral(config) => Ok(Arc::new(mistral::chat::Chat::new(
            http::client(limit)?,
            key,
            config,
        ))),
    }
}

#[cfg(all(test, feature = "mistral"))]
mod tests {
    use std::time::Duration;

    use super::{ChatConfig, TranscriberConfig, build_chat_model};

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
    fn the_default_chat_model_is_mistral_medium_without_reasoning() {
        let config = ChatConfig::default();

        assert_eq!(config.account(), "mistral");
        let ChatConfig::Mistral(mistral) = &config;
        assert_eq!(mistral.model, "mistral-medium-latest");
        assert_eq!(mistral.reasoning_effort.as_deref(), Some("none"));
        assert_eq!(mistral.temperature, 0.0);
    }

    #[test]
    fn the_provider_value_selects_the_same_chat_provider() {
        let config = ChatConfig::default();

        let read: ChatConfig =
            serde_json::from_value(serde_json::json!({ "provider": config.provider() })).unwrap();

        assert_eq!(config.provider(), "mistral");
        assert_eq!(read, config);
    }

    #[test]
    fn models_gives_the_models_to_select_from() {
        assert_eq!(
            ChatConfig::default().models(),
            ["mistral-medium-latest", "mistral-small-latest"]
        );
    }

    #[test]
    fn with_model_replaces_the_model_and_keeps_the_other_chat_settings() {
        let config = ChatConfig::default();
        assert_eq!(config.model(), "mistral-medium-latest");

        let config = config.with_model("ministral-8b-2512".to_owned());

        assert_eq!(config.model(), "ministral-8b-2512");
        assert_eq!(config.account(), "mistral");
        let ChatConfig::Mistral(mistral) = &config;
        assert_eq!(mistral.reasoning_effort.as_deref(), Some("none"));
        assert_eq!(mistral.temperature, 0.0);
    }

    #[test]
    fn the_debug_of_the_configs_shows_no_key() {
        let transcriber = format!("{:?}", TranscriberConfig::default());
        let chat = format!("{:?}", ChatConfig::default());

        assert!(transcriber.starts_with("Mistral(TranscribeConfig {"));
        assert!(chat.starts_with("Mistral(ChatConfig {"));
        assert!(!transcriber.contains(KEY));
        assert!(!chat.contains(KEY));
    }

    #[test]
    fn build_chat_model_starts_the_http_client() {
        let result = build_chat_model(ChatConfig::default(), KEY.into(), Duration::from_secs(8));

        assert!(result.is_ok());
    }
}
