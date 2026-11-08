//! The commented settings file that Bloop writes when `config.toml` does not exist.

use bloop_providers::{ChatConfig, TranscriberConfig};

/// The header, `schema`, and the `[dictation]` table.
const HEAD: &str = r#"# Bloop settings. Each key is optional: a missing key has the value below.
# A choice in the Bloop menu changes its key in this file. Bloop keeps your comments.

schema = 1                    # The version of this file format.

[dictation]
hold_ms = 220                 # The hold time of the dictation key before the recording starts: 100 to 1000 ms.
language = "fr"               # The language of the dictation: two lowercase ASCII letters.
"#;

/// The `[refine]` table, and the `[refine.apps]` table in comments.
const REFINE: &str = r#"[refine]
enabled = false               # The correction by the chat model: "Correct text" in the menu.
prompt = "natural-light"      # A built-in prompt, or a file of the prompts folder without ".md".

# The prompt of each application, by bundle identifier. Another application uses "prompt".
# "code" and "natural-light" have built-in lists. A key replaces the built-in list of its prompt.
# [refine.apps]
# code = ["com.apple.Terminal", "com.mitchellh.ghostty"]
# natural-light = ["com.apple.MobileSMS", "net.whatsapp.WhatsApp"]
"#;

/// Returns the commented settings file with the default values.
/// The provider tables come from `bloop-providers`, so this text names no provider.
pub(crate) fn template() -> String {
    [
        HEAD,
        TranscriberConfig::template(),
        ChatConfig::template(),
        REFINE,
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::template;
    use crate::settings::schema::{Settings, parse};

    #[test]
    fn the_template_parses_to_the_default_settings() {
        assert_eq!(parse(&template()), Ok(Settings::default()));
    }

    #[test]
    fn the_commented_apps_table_parses_without_its_comment_marks() {
        let text = template();
        let start = text.find("# [refine.apps]").unwrap();
        let uncommented = format!("{}{}", &text[..start], text[start..].replace("# ", ""));

        let settings = parse(&uncommented).unwrap();

        assert_ne!(settings.refine.apps, Settings::default().refine.apps);
    }

    #[test]
    fn the_template_has_one_blank_line_between_the_tables() {
        let text = template();

        assert!(text.contains("\n\n[transcriber]\n"), "{text}");
        assert!(text.contains("\n\n[chat]\n"), "{text}");
        assert!(text.contains("\n\n[refine]\n"), "{text}");
        assert!(!text.contains("\n\n\n"), "{text}");
        assert!(text.ends_with('\n') && !text.ends_with("\n\n"), "{text}");
    }
}
