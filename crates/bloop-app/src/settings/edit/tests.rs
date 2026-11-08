use bloop_providers::ChatConfig;

use super::with_change;
use crate::settings::template::template;
use crate::settings::{AppId, Change, HoldMs, PromptName, parse};

fn provider() -> &'static str {
    ChatConfig::default().provider()
}

fn changed(text: &str, change: &Change) -> String {
    with_change(text, change, &parse(text).unwrap()).unwrap()
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
    let text = "# Bloop settings\nschema = 1\n\n\n# The correction\n[refine]  # menu\n  # Set from the menu\n  enabled   =   false   # off by default\n\n# End\n";

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

fn add(prompt: &str, app: &str) -> Change {
    Change::AddApp {
        prompt: PromptName::try_from(prompt.to_owned()).unwrap(),
        app: AppId::try_from(app.to_owned()).unwrap(),
    }
}

fn remove(prompt: &str, app: &str) -> Change {
    Change::RemoveApp {
        prompt: PromptName::try_from(prompt.to_owned()).unwrap(),
        app: AppId::try_from(app.to_owned()).unwrap(),
    }
}

const APPS: &str = r#"schema = 1

[refine]
enabled = true

[refine.apps]
# my terminals
code = ["com.apple.Terminal", "com.mitchellh.ghostty"]   # mine
mail = ["com.apple.mail"]

[chat]
provider = "mistral"
"#;

#[test]
fn add_moves_the_app_from_its_list_and_writes_both_keys() {
    let new = changed(APPS, &add("mail", "com.mitchellh.ghostty"));

    assert_eq!(
        new,
        APPS.replace(
            r#"code = ["com.apple.Terminal", "com.mitchellh.ghostty"]   # mine
mail = ["com.apple.mail"]"#,
            r#"code = ["com.apple.Terminal"]                            # mine
mail = ["com.apple.mail", "com.mitchellh.ghostty"]"#
        )
    );
    let lists = parse(&new).unwrap().refine.apps;
    assert_eq!(lists.iter().count(), 2);
}

#[test]
fn add_to_a_list_without_a_key_writes_the_built_in_list_and_the_list_that_loses_the_app() {
    let new = changed(APPS, &add("natural-light", "com.apple.mail"));

    assert_eq!(
        new,
        APPS.replace(
            "mail = [\"com.apple.mail\"]\n",
            "mail = []\nnatural-light = [\"com.apple.MobileSMS\", \"net.whatsapp.WhatsApp\", \"ru.keepcoder.Telegram\", \"org.telegram.desktop\", \"org.whispersystems.signal-desktop\", \"com.apple.mail\"]\n"
        )
    );
    assert!(parse(&new).is_ok());
}

#[test]
fn remove_from_a_built_in_list_writes_the_rest_of_the_list() {
    let text = "schema = 1\n\n[refine]\nenabled = true\n";

    assert_eq!(
        changed(text, &remove("code", "com.apple.dt.Xcode")),
        format!(
            "{text}\n[refine.apps]\ncode = [\"com.apple.Terminal\", \"com.mitchellh.ghostty\", \"com.googlecode.iterm2\", \"dev.warp.Warp-Stable\", \"com.microsoft.VSCode\", \"com.todesktop.230313mzl4w4u92\", \"dev.zed.Zed\", \"com.anthropic.claudefordesktop\"]\n"
        )
    );
}

#[test]
fn remove_writes_the_rest_of_the_list_and_keeps_the_comment_above_the_key() {
    assert_eq!(
        changed(APPS, &remove("code", "com.apple.Terminal")),
        APPS.replace(
            r#"code = ["com.apple.Terminal", "com.mitchellh.ghostty"]   # mine"#,
            r#"code = ["com.mitchellh.ghostty"]                         # mine"#
        )
    );
}

#[test]
fn a_change_that_changes_no_list_keeps_the_text_byte_for_byte() {
    for change in [
        add("code", "com.apple.Terminal"),
        remove("code", "com.apple.Notes"),
        remove("absent", "com.apple.Terminal"),
    ] {
        assert_eq!(changed(APPS, &change), APPS, "{change:?}");
    }
}

#[test]
fn a_new_apps_table_goes_directly_after_the_refine_table() {
    let text = "schema = 1\n\n[refine]\nenabled = true   # on\n\n[chat]\nprovider = \"mistral\"\n";

    assert_eq!(
        changed(text, &add("mail", "com.apple.mail")),
        "schema = 1\n\n[refine]\nenabled = true   # on\n\n[refine.apps]\nmail = [\"com.apple.mail\"]\n\n[chat]\nprovider = \"mistral\"\n"
    );
}

#[test]
fn a_new_apps_table_without_the_refine_table_has_no_empty_refine_header() {
    let text = "schema = 1\n\n[chat]\nprovider = \"mistral\"\n";

    let new = changed(text, &add("mail", "com.apple.mail"));

    assert_eq!(
        new,
        format!("{text}\n[refine.apps]\nmail = [\"com.apple.mail\"]\n")
    );
    assert_eq!(
        correction(&new, true),
        format!("{text}\n[refine]\nenabled = true\n\n[refine.apps]\nmail = [\"com.apple.mail\"]\n")
    );
}

#[test]
fn a_list_in_an_inline_or_a_dotted_apps_table_changes_in_place() {
    for (text, new) in [
        (
            "[refine]\napps = { mail = [\"com.apple.mail\"] }\n",
            "[refine]\napps = { mail = [\"com.apple.mail\"] , notes = [\"com.apple.Notes\"] }\n",
        ),
        (
            "[refine]\napps.mail = [\"com.apple.mail\"]\n",
            "[refine]\napps.mail = [\"com.apple.mail\"]\napps.notes = [\"com.apple.Notes\"]\n",
        ),
        (
            "refine = { enabled = true }\n",
            "refine = { enabled = true , apps = { notes = [\"com.apple.Notes\"] } }\n",
        ),
    ] {
        let result = changed(text, &add("notes", "com.apple.Notes"));

        assert_eq!(result, new, "{text}");
        assert!(parse(&result).is_ok(), "{result}");
    }
}

#[test]
fn the_comments_inside_a_multi_line_list_are_lost() {
    let text = "[refine.apps]\n# above\nmail = [\n  \"com.apple.mail\",  # mail\n  \"com.apple.Notes\",\n]\n";

    assert_eq!(
        changed(text, &remove("mail", "com.apple.Notes")),
        "[refine.apps]\n# above\nmail = [\"com.apple.mail\"]\n"
    );
}

#[test]
fn an_application_change_on_the_template_puts_the_table_above_the_commented_example() {
    let text = template();
    let example = text.find("\n# The prompt of each application").unwrap();

    let new = changed(&text, &add("mail", "com.apple.mail"));

    assert_eq!(
        new,
        format!(
            "{}\n[refine.apps]\nmail = [\"com.apple.mail\"]\n{}",
            &text[..example],
            &text[example..]
        )
    );
    assert!(parse(&new).is_ok());
}
