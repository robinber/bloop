use bloop_providers::ChatConfig;

use super::with_change;
use crate::settings::template::template;
use crate::settings::{Change, HoldMs, PromptName, parse};

fn provider() -> &'static str {
    ChatConfig::default().provider()
}

fn changed(text: &str, change: &Change) -> String {
    with_change(text, change, provider()).unwrap()
}

fn correction(text: &str, on: bool) -> String {
    changed(text, &Change::Correction(on))
}

fn prompt(name: &str) -> Change {
    Change::Prompt(PromptName::try_from(name.to_owned()).unwrap())
}

fn hold(ms: u64) -> Change {
    Change::Hold(HoldMs::try_from(ms).unwrap())
}

fn model(name: &str) -> Change {
    Change::Model(name.to_owned())
}

#[test]
fn a_new_file_gets_the_template_with_the_correction_state() {
    let text = template();

    assert_eq!(
        correction(&text, true),
        text.replace("enabled = false ", "enabled = true  ")
    );
    assert_eq!(correction(&text, false), text);
}

#[test]
fn the_change_replaces_the_value() {
    assert_eq!(
        correction("schema = 1\n\n[refine]\nenabled = true\n", false),
        "schema = 1\n\n[refine]\nenabled = false\n"
    );
}

#[test]
fn the_change_keeps_the_comments_and_the_spacing() {
    let text = "# Bloop settings\nschema = 1\n\n\n# The correction\n[refine]  # menu\n  # Set from the menu\n  enabled   =   false   # off for now\n\n# End\n";

    assert_eq!(
        correction(text, true),
        text.replace("=   false   #", "=   true    #")
    );
}

#[test]
fn the_change_keeps_the_column_of_the_comment() {
    let text = "[refine]\nenabled = false   # off\nprompt = \"x\"     # name\n";
    let on = "[refine]\nenabled = true    # off\nprompt = \"x\"     # name\n";

    assert_eq!(correction(text, true), on);
    assert_eq!(correction(on, false), text);
}

#[test]
fn a_comment_without_room_keeps_one_space() {
    assert_eq!(
        correction("[refine]\nenabled = true # on\n", false),
        "[refine]\nenabled = false # on\n"
    );
}

#[test]
fn a_comment_without_a_space_before_it_does_not_move() {
    assert_eq!(
        correction("[refine]\nenabled = true#on\n", false),
        "[refine]\nenabled = false#on\n"
    );
}

#[test]
fn a_file_without_the_refine_table_gets_a_standard_table() {
    assert_eq!(
        correction("# Bloop settings\nschema = 1\n", true),
        "# Bloop settings\nschema = 1\n\n[refine]\nenabled = true\n"
    );
}

#[test]
fn each_change_of_the_template_changes_only_its_value_and_keeps_the_comment_column() {
    let text = template();
    for (change, old, new) in [
        (
            prompt("dev"),
            "prompt = \"natural-light\"      #",
            "prompt = \"dev\"                #",
        ),
        (
            model("mistral-small-latest"),
            "model = \"mistral-medium-latest\"                             #",
            "model = \"mistral-small-latest\"                              #",
        ),
        (
            hold(400),
            "hold_ms = 220                 #",
            "hold_ms = 400                 #",
        ),
        (
            hold(1000),
            "hold_ms = 220                 #",
            "hold_ms = 1000                #",
        ),
    ] {
        assert_eq!(text.matches(old).count(), 1, "{old}");

        assert_eq!(
            changed(&text, &change),
            text.replace(old, new),
            "{change:?}"
        );
    }
}

const MINE: &str = r#"# My settings
schema = 1

[dictation]  # timing
hold_ms = 220   # mine
language = "fr"

[chat]
provider = "mistral"
# the model I use
model = "mistral-medium-latest"  # now
models = ["mistral-medium-latest", "mistral-small-latest"]

[refine]
enabled = true
prompt = "natural-light"   # name
# end
"#;

#[test]
fn each_change_of_a_hand_written_file_keeps_the_other_text_byte_for_byte() {
    for (change, old, new) in [
        (
            Change::Correction(false),
            "enabled = true\n",
            "enabled = false\n",
        ),
        (
            prompt("dev"),
            "prompt = \"natural-light\"   # name",
            "prompt = \"dev\"             # name",
        ),
        (
            model("mistral-small-latest"),
            "model = \"mistral-medium-latest\"  # now",
            "model = \"mistral-small-latest\"   # now",
        ),
        (
            hold(150),
            "hold_ms = 220   # mine",
            "hold_ms = 150   # mine",
        ),
    ] {
        assert_eq!(MINE.matches(old).count(), 1, "{old}");

        assert_eq!(changed(MINE, &change), MINE.replace(old, new), "{change:?}");
    }
}

#[test]
fn a_model_change_without_the_chat_table_writes_the_provider_of_the_current_settings() {
    let text = "schema = 1\n\n[refine]\nenabled = true\n";

    let new = changed(text, &model("mistral-small-latest"));

    assert_eq!(
        new,
        format!(
            "{text}\n[chat]\nprovider = \"{}\"\nmodel = \"mistral-small-latest\"\n",
            provider()
        )
    );
    let settings = parse(&new).unwrap();
    assert_eq!(settings.chat.model(), "mistral-small-latest");
    assert_eq!(settings.chat.provider(), provider());
    assert!(settings.refine.enabled);
}

#[test]
fn a_model_change_keeps_the_provider_of_the_chat_table() {
    let text = "[chat]\nprovider = \"mistral\"  # mine\n";

    assert_eq!(
        changed(text, &model("mistral-small-latest")),
        format!("{text}model = \"mistral-small-latest\"\n")
    );
}

#[test]
fn a_change_without_its_table_adds_a_standard_table_at_the_end() {
    let text = "schema = 1\n";

    assert_eq!(
        changed(text, &prompt("dev")),
        "schema = 1\n\n[refine]\nprompt = \"dev\"\n"
    );
    assert_eq!(
        changed(text, &hold(300)),
        "schema = 1\n\n[dictation]\nhold_ms = 300\n"
    );
}

#[test]
fn a_comment_after_the_last_table_goes_below_a_new_table() {
    let chat = r#"[chat]
provider = "mistral"
# the model I use
model = "mistral-medium-latest"  # now
models = ["mistral-medium-latest", "mistral-small-latest"]

"#;
    assert_eq!(MINE.matches(chat).count(), 1);
    let text = MINE.replace(chat, "");

    assert_eq!(
        changed(&text, &model("mistral-small-latest")),
        r#"# My settings
schema = 1

[dictation]  # timing
hold_ms = 220   # mine
language = "fr"

[refine]
enabled = true
prompt = "natural-light"   # name

[chat]
provider = "mistral"
model = "mistral-small-latest"
# end
"#
    );
}
