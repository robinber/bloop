//! The change of one key in the settings text with `toml_edit`. The comments and the other keys stay.

use toml_edit::{DocumentMut, Item, RawString, TomlError, Value};

use super::Invalid;
use crate::dictionary::line;

/// Returns `text` with `[refine] enabled` set to `on`. The other text does not change.
pub(super) fn with_correction(text: &str, on: bool) -> Result<String, Invalid> {
    let mut document: DocumentMut = text.parse().map_err(|error: TomlError| Invalid::Parse {
        line: error.span().and_then(|span| line(text, span.start)),
    })?;
    let refine = document
        .entry("refine")
        .or_insert(toml_edit::table())
        .as_table_like_mut()
        .ok_or(Invalid::Parse { line: None })?;
    let new = Value::from(on);
    // `insert` on an existing key resets the key format, which removes the comments above it.
    match refine.get_mut("enabled") {
        Some(item) => {
            *item = Item::Value(match item.as_value() {
                Some(old) => with_decor_of(old, new),
                None => new,
            });
        }
        None => {
            refine.insert("enabled", Item::Value(new));
        }
    }
    Ok(document.to_string())
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
mod tests {
    use super::with_correction;
    use crate::settings::template::template;

    #[test]
    fn a_new_file_gets_the_template_with_the_correction_state() {
        let text = template();

        assert_eq!(
            with_correction(&text, true).unwrap(),
            text.replace("enabled = false ", "enabled = true  ")
        );
        assert_eq!(with_correction(&text, false).unwrap(), text);
    }

    #[test]
    fn the_change_replaces_the_value() {
        assert_eq!(
            with_correction("schema = 1\n\n[refine]\nenabled = true\n", false).unwrap(),
            "schema = 1\n\n[refine]\nenabled = false\n"
        );
    }

    #[test]
    fn the_change_keeps_the_comments_and_the_spacing() {
        let text = "# Bloop settings\nschema = 1\n\n\n# The correction\n[refine]  # menu\n  # Set from the menu\n  enabled   =   false   # off for now\n\n# End\n";

        assert_eq!(
            with_correction(text, true).unwrap(),
            text.replace("=   false   #", "=   true    #")
        );
    }

    #[test]
    fn the_change_keeps_the_column_of_the_comment() {
        let text = "[refine]\nenabled = false   # off\nprompt = \"x\"     # name\n";
        let on = "[refine]\nenabled = true    # off\nprompt = \"x\"     # name\n";

        assert_eq!(with_correction(text, true).unwrap(), on);
        assert_eq!(with_correction(on, false).unwrap(), text);
    }

    #[test]
    fn a_comment_without_room_keeps_one_space() {
        assert_eq!(
            with_correction("[refine]\nenabled = true # on\n", false).unwrap(),
            "[refine]\nenabled = false # on\n"
        );
    }

    #[test]
    fn a_comment_without_a_space_before_it_does_not_move() {
        assert_eq!(
            with_correction("[refine]\nenabled = true#on\n", false).unwrap(),
            "[refine]\nenabled = false#on\n"
        );
    }

    #[test]
    fn a_file_without_the_refine_table_gets_a_standard_table() {
        assert_eq!(
            with_correction("# Bloop settings\nschema = 1\n", true).unwrap(),
            "# Bloop settings\nschema = 1\n\n[refine]\nenabled = true\n"
        );
    }
}
