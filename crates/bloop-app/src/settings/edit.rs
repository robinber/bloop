//! The change of one or more keys of one table in the settings text with `toml_edit`.
//! The comments and the other keys stay.

use toml_edit::{
    Array, DocumentMut, InlineTable, Item, RawString, Table, TableLike, TomlError, Value,
};

use super::{AppId, Change, Invalid, PromptName, Settings};
use crate::dictionary::line;
use crate::prompts;

/// Returns `text` with the keys of `change` set. The other text does not change.
/// A missing table goes at the end of the text. A missing `[refine.apps]` goes directly after `[refine]`.
///
/// `settings` are the current settings of `text`. A `[chat]` table without `provider` is invalid.
/// Thus, when a model change creates the table, or the table has no `provider` key, the change also writes `provider`.
/// An application change computes the new lists from the lists of `settings`, never from an older menu.
pub(super) fn with_change(
    text: &str,
    change: &Change,
    settings: &Settings,
) -> Result<String, Invalid> {
    let mut document: DocumentMut = text.parse().map_err(|error: TomlError| Invalid::Parse {
        line: error.span().and_then(|span| line(text, span.start)),
    })?;
    let (table, key, value, tag) = match change {
        Change::Correction(on) => ("refine", "enabled", Value::from(*on), None),
        Change::Prompt(name) => ("refine", "prompt", Value::from(name.as_str()), None),
        Change::Model(model) => (
            "chat",
            "model",
            Value::from(model.as_str()),
            Some(settings.chat.provider()),
        ),
        Change::Hold(hold) => (
            "dictation",
            "hold_ms",
            Value::from(i64::from(hold.get())),
            None,
        ),
        Change::AddApp { prompt, app } => {
            set_lists(&mut document, &added(settings, prompt, app))?;
            return Ok(document.to_string());
        }
        Change::RemoveApp { prompt, app } => {
            set_lists(&mut document, &removed(settings, prompt, app))?;
            return Ok(document.to_string());
        }
    };
    let table = document
        .entry(table)
        .or_insert(toml_edit::table())
        .as_table_like_mut()
        .ok_or(Invalid::Parse { line: None })?;
    if let Some(tag) = tag
        && !table.contains_key("provider")
    {
        table.insert("provider", Item::Value(Value::from(tag)));
    }
    set(table, key, value);
    Ok(document.to_string())
}

/// The new lists of the application change: the key of each list and its applications.
type Lists = Vec<(PromptName, Vec<AppId>)>;

/// Returns the lists that change when `app` joins the list of `prompt`: that list, and each list that loses `app`.
/// It returns no list when the list of `prompt` already contains `app`.
fn added(settings: &Settings, prompt: &PromptName, app: &AppId) -> Lists {
    let lists = prompts::app_lists(settings);
    let mut target = lists.get(prompt).cloned().unwrap_or_default();
    if target.contains(app) {
        return Vec::new();
    }
    target.push(app.clone());
    let mut changed = vec![(prompt.clone(), target)];
    for (name, list) in lists {
        if list.contains(app) {
            changed.push((name, without(&list, app)));
        }
    }
    changed
}

/// Returns the list of `prompt` without `app`, or no list when the list of `prompt` does not contain `app`.
fn removed(settings: &Settings, prompt: &PromptName, app: &AppId) -> Lists {
    prompts::app_lists(settings)
        .remove(prompt)
        .filter(|list| list.contains(app))
        .map(|list| vec![(prompt.clone(), without(&list, app))])
        .unwrap_or_default()
}

fn without(list: &[AppId], app: &AppId) -> Vec<AppId> {
    list.iter().filter(|id| *id != app).cloned().collect()
}

/// Sets each list of `lists` in `[refine.apps]`. A new `[refine.apps]` table prints directly after `[refine]`.
/// Without `[refine]`, the new parent table is implicit, so that the text gets no empty `[refine]` header.
fn set_lists(document: &mut DocumentMut, lists: &Lists) -> Result<(), Invalid> {
    if lists.is_empty() {
        return Ok(());
    }
    let refine = document.entry("refine").or_insert_with(|| {
        let mut table = Table::new();
        table.set_implicit(true);
        Item::Table(table)
    });
    let new = if refine.is_table() {
        toml_edit::table()
    } else {
        Item::Value(Value::InlineTable(InlineTable::new()))
    };
    let apps = refine
        .as_table_like_mut()
        .ok_or(Invalid::Parse { line: None })?
        .entry("apps")
        .or_insert(new)
        .as_table_like_mut()
        .ok_or(Invalid::Parse { line: None })?;
    for (prompt, list) in lists {
        let list: Array = list.iter().map(AppId::as_str).collect();
        set(apps, prompt.as_str(), Value::Array(list));
    }
    Ok(())
}

/// Sets `key` of `table` to `value`. An existing key keeps its spaces and its comments.
fn set(table: &mut dyn TableLike, key: &str, value: Value) {
    // `insert` on an existing key resets the key format, which removes the comments above it.
    match table.get_mut(key) {
        Some(item) => {
            *item = Item::Value(match item.as_value() {
                Some(old) => with_decor_of(old, value),
                None => value,
            });
        }
        None => {
            table.insert(key, Item::Value(value));
        }
    }
}

/// Returns `new` with the spaces and the comments of `old`.
/// When a comment follows the value on its line, the comment stays in its column, with at least one space before it.
fn with_decor_of(old: &Value, mut new: Value) -> Value {
    *new.decor_mut() = old.decor().clone();
    let Some(suffix) = old.decor().suffix().and_then(RawString::as_str) else {
        return new;
    };
    let comment = suffix.trim_start_matches(' ');
    let gap = suffix.len() - comment.len();
    if gap > 0 && comment.starts_with('#') {
        let gap = (gap + width(old)).saturating_sub(width(&new)).max(1);
        let suffix = format!("{}{comment}", " ".repeat(gap));
        new.decor_mut().set_suffix(suffix);
    }
    new
}

/// Returns the number of characters of the value, without its spaces and comments.
fn width(value: &Value) -> usize {
    let mut bare = value.clone();
    bare.decor_mut().clear();
    bare.to_string().chars().count()
}

#[cfg(test)]
mod tests;
