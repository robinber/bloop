//! The change of one key in the settings text with `toml_edit`. The comments and the other keys stay.

use toml_edit::{DocumentMut, Item, TomlError, Value};

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
    let mut new = Value::from(on);
    // `insert` on an existing key resets the key format, which removes the comments above it.
    match refine.get_mut("enabled") {
        Some(item) => {
            if let Some(old) = item.as_value() {
                *new.decor_mut() = old.decor().clone();
            }
            *item = Item::Value(new);
        }
        None => {
            refine.insert("enabled", Item::Value(new));
        }
    }
    Ok(document.to_string())
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
            text.replace("enabled = false", "enabled = true")
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
            text.replace("=   false", "=   true")
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
