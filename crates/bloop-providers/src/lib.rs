//! AI provider adapters of Bloop, with one module for each vendor.
//!
//! `bloop-app` selects a provider with [`TranscriberConfig`] and [`ChatConfig`],
//! and builds it with [`build_transcriber`] and [`build_chat_model`].

#![forbid(unsafe_code)]

mod http;
pub mod mistral;

#[cfg(test)]
mod test_log;

use std::sync::Arc;
use std::time::Duration;

use bloop_core::error::ProviderError;
use bloop_core::ports::ai::{ChatModel, Transcriber};
use secrecy::SecretString;

/// The transcription provider and its settings.
///
/// In the settings file, the `provider` key selects the variant, in snake case.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum TranscriberConfig {
    /// Voxtral realtime transcription.
    Mistral(mistral::TranscribeConfig),
}

impl TranscriberConfig {
    /// The `SecretStore` account of the API key of the provider.
    #[must_use]
    pub fn account(&self) -> &'static str {
        match *self {
            Self::Mistral(_) => mistral::ACCOUNT,
        }
    }

    /// The commented `[transcriber]` table of the default provider, with its default values, for the settings template.
    #[must_use]
    pub fn template() -> &'static str {
        mistral::TRANSCRIBER_TEMPLATE
    }
}

impl Default for TranscriberConfig {
    fn default() -> Self {
        Self::Mistral(mistral::TranscribeConfig::default())
    }
}

/// The chat provider and its settings.
///
/// In the settings file, the `provider` key selects the variant, in snake case.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ChatConfig {
    /// Mistral chat completions.
    Mistral(mistral::ChatConfig),
}

impl ChatConfig {
    /// The `SecretStore` account of the API key of the provider.
    #[must_use]
    pub fn account(&self) -> &'static str {
        match *self {
            Self::Mistral(_) => mistral::ACCOUNT,
        }
    }

    /// The `provider` value of the `[chat]` table that selects this provider in the settings file.
    #[must_use]
    pub fn provider(&self) -> &'static str {
        match *self {
            Self::Mistral(_) => mistral::PROVIDER,
        }
    }

    /// The model identifier.
    #[must_use]
    pub fn model(&self) -> &str {
        match *self {
            Self::Mistral(ref config) => &config.model,
        }
    }

    /// The models that the user can select in the status menu.
    #[must_use]
    pub fn models(&self) -> &[String] {
        match *self {
            Self::Mistral(ref config) => &config.models,
        }
    }

    /// The commented `[chat]` table of the default provider, with its default values, for the settings template.
    #[must_use]
    pub fn template() -> &'static str {
        mistral::CHAT_TEMPLATE
    }
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self::Mistral(mistral::ChatConfig::default())
    }
}

/// Builds the transcriber that `config` selects. `key` is the API key of its provider.
#[must_use]
pub fn build_transcriber(config: TranscriberConfig, key: SecretString) -> Box<dyn Transcriber> {
    match config {
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
pub fn build_chat_model(
    config: ChatConfig,
    key: SecretString,
    limit: Duration,
) -> Result<Arc<dyn ChatModel>, ProviderError> {
    match config {
        ChatConfig::Mistral(config) => Ok(Arc::new(mistral::chat::Chat::new(
            http::client(limit)?,
            key,
            config,
        ))),
    }
}

#[cfg(test)]
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
        assert_eq!(mistral.reasoning_effort, "none");
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
    fn the_debug_of_the_configs_names_the_provider_and_its_settings() {
        let transcriber = format!("{:?}", TranscriberConfig::default());
        let chat = format!("{:?}", ChatConfig::default());

        assert!(transcriber.starts_with("Mistral(TranscribeConfig {"));
        assert!(chat.starts_with("Mistral(ChatConfig {"));
    }

    #[test]
    fn build_chat_model_starts_the_http_client() {
        let result = build_chat_model(ChatConfig::default(), KEY.into(), Duration::from_secs(8));

        assert!(result.is_ok());
    }
}
