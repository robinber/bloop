//! The change of one key in the settings text with `toml_edit`. The comments and the other keys stay.

use toml_edit::{DocumentMut, Item, RawString, TableLike, TomlError, Value};

use super::{Change, Invalid};
use crate::dictionary::line;

/// Returns `text` with the key of `change` set. The other text does not change.
/// A missing table goes at the end of the text.
///
/// `provider` is the `provider` value of the current `[chat]` settings. A `[chat]` table without `provider` is invalid.
/// Thus, when a model change creates the table, or the table has no `provider` key, the change also writes `provider`.
pub(super) fn with_change(text: &str, change: &Change, provider: &str) -> Result<String, Invalid> {
    let mut document: DocumentMut = text.parse().map_err(|error: TomlError| Invalid::Parse {
        line: error.span().and_then(|span| line(text, span.start)),
    })?;
    let (table, key, value, tag) = match change {
        Change::Correction(on) => ("refine", "enabled", Value::from(*on), None),
        Change::Prompt(name) => ("refine", "prompt", Value::from(name.as_str()), None),
        Change::Model(model) => ("chat", "model", Value::from(model.as_str()), Some(provider)),
        Change::Hold(hold) => (
            "dictation",
            "hold_ms",
            Value::from(i64::from(hold.get())),
            None,
        ),
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
