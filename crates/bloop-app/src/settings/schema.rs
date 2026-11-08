//! The settings schema: the tables, their default values, and the validated values.
//!
//! Each key is optional, and each table rejects unknown keys. The newtypes check their values at parse time.
//! Thus a `Settings` value is always valid.

use std::collections::BTreeMap;
use std::time::Duration;

use bloop_providers::{ChatConfig, TranscriberConfig};
use serde::Deserialize;
use toml::Spanned;

use super::Invalid;
use crate::dictionary::line;

/// The only `schema` value that this version of Bloop reads.
const SCHEMA: u64 = 1;

/// The valid settings. A missing key or table has its default value.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Settings {
    pub(crate) dictation: Dictation,
    pub(crate) transcriber: TranscriberConfig,
    pub(crate) chat: ChatConfig,
    pub(crate) refine: Refine,
}

/// The settings file as the parser reads it. `parse` checks `schema` and keeps the tables.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    schema: Option<Spanned<u64>>,
    #[serde(default)]
    dictation: Dictation,
    #[serde(default)]
    transcriber: TranscriberConfig,
    #[serde(default)]
    chat: ChatConfig,
    #[serde(default)]
    refine: Refine,
}

/// The `[dictation]` table.
#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Dictation {
    pub(crate) hold_ms: HoldMs,
    pub(crate) language: Language,
}

/// The `[refine]` table: the correction.
#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Refine {
    /// The state of the "Correct text" menu item.
    pub(crate) enabled: bool,
    /// The default prompt: the prompt of an application that no list of `apps` contains.
    pub(crate) prompt: PromptName,
    /// The application lists, or `None` when the file has no `[refine.apps]` table.
    pub(crate) apps: Option<Apps>,
}

/// The `[refine.apps]` table: the applications of each prompt. A prompt without a key has no application.
///
/// A list has no duplicate: the parser ignores a repeated application in one list.
/// An application in two lists makes the file invalid.
#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(try_from = "BTreeMap<PromptName, Vec<AppId>>")]
pub(crate) struct Apps(BTreeMap<PromptName, Vec<AppId>>);

impl Apps {
    /// Returns each prompt name of the table and its list, sorted by name.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&PromptName, &[AppId])> {
        self.0
            .iter()
            .map(|(prompt, apps)| (prompt, apps.as_slice()))
    }
}

impl TryFrom<BTreeMap<PromptName, Vec<AppId>>> for Apps {
    type Error = &'static str;

    fn try_from(lists: BTreeMap<PromptName, Vec<AppId>>) -> Result<Self, Self::Error> {
        let mut owners: BTreeMap<AppId, PromptName> = BTreeMap::new();
        let mut checked = BTreeMap::new();
        for (prompt, apps) in lists {
            let mut list = Vec::with_capacity(apps.len());
            for app in apps {
                match owners.get(&app) {
                    Some(owner) if *owner == prompt => {}
                    Some(_) => return Err("an application is in two lists of [refine.apps]"),
                    None => {
                        owners.insert(app.clone(), prompt.clone());
                        list.push(app);
                    }
                }
            }
            checked.insert(prompt, list);
        }
        Ok(Self(checked))
    }
}

/// The hold time of the dictation key before the capture starts: 100 to 1000 ms, 220 ms by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "u64")]
pub(crate) struct HoldMs(u16);

impl HoldMs {
    pub(crate) fn get(self) -> u16 {
        self.0
    }

    pub(crate) fn duration(self) -> Duration {
        Duration::from_millis(u64::from(self.0))
    }
}

impl Default for HoldMs {
    fn default() -> Self {
        Self(220)
    }
}

impl TryFrom<u64> for HoldMs {
    type Error = &'static str;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        u16::try_from(value)
            .ok()
            .filter(|ms| (100..=1000).contains(ms))
            .map(Self)
            .ok_or("the hold time is not from 100 to 1000 ms")
    }
}

/// The language of the dictation: two lowercase ASCII letters, `fr` by default.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct Language(String);

impl Language {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for Language {
    fn default() -> Self {
        Self("fr".to_owned())
    }
}

impl TryFrom<String> for Language {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() == 2 && value.bytes().all(|byte| byte.is_ascii_lowercase()) {
            Ok(Self(value))
        } else {
            Err("the language is not two lowercase ASCII letters")
        }
    }
}

/// The name of a correction prompt: 1 to 64 characters from `A-Z`, `a-z`, `0-9`, `_`, and `-`, `natural-light` by default.
/// Thus a name cannot point outside the prompts folder.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct PromptName(String);

impl PromptName {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for PromptName {
    fn default() -> Self {
        Self("natural-light".to_owned())
    }
}

impl TryFrom<String> for PromptName {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let allowed = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-';
        if (1..=64).contains(&value.len()) && value.bytes().all(allowed) {
            Ok(Self(value))
        } else {
            Err("the prompt name is not 1 to 64 characters from A-Z a-z 0-9 _ -")
        }
    }
}

/// The bundle identifier of an application: 1 to 255 characters from `A-Z`, `a-z`, `0-9`, `.`, `-`, and `_`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct AppId(String);

impl AppId {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AppId {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let allowed = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_');
        if (1..=255).contains(&value.len()) && value.bytes().all(allowed) {
            Ok(Self(value))
        } else {
            Err("the bundle identifier is not 1 to 255 characters from A-Z a-z 0-9 . - _")
        }
    }
}

/// Returns the settings of a valid file. The error gives the line, never the value or the parser message.
///
/// The line is the line of the value, or of the key. In `[transcriber]` and `[chat]`, an unknown key or a wrong type
/// gives the line of the table header, because the parser reads these tables before it selects the provider.
/// An application in two lists of `[refine.apps]` gives the line of that table header.
pub(crate) fn parse(text: &str) -> Result<Settings, Invalid> {
    let file: File = toml::from_str(text).map_err(|error| Invalid::Parse {
        line: error.span().and_then(|span| line(text, span.start)),
    })?;
    if let Some(schema) = file.schema
        && *schema.get_ref() != SCHEMA
    {
        return Err(Invalid::Schema {
            line: line(text, schema.span().start),
        });
    }
    Ok(Settings {
        dictation: file.dictation,
        transcriber: file.transcriber,
        chat: file.chat,
        refine: file.refine,
    })
}

#[cfg(test)]
mod tests;
