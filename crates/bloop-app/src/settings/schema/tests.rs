use std::time::Duration;

use bloop_providers::{ChatConfig, TranscriberConfig};

use super::{Settings, parse};
use crate::settings::Invalid;

/// The correction state of a valid file.
fn correction(text: &str) -> Result<bool, Invalid> {
    parse(text).map(|settings| settings.refine.enabled)
}

/// The line of the error, for a file that must be invalid.
fn error_line(text: &str) -> Option<usize> {
    match parse(text) {
        Err(Invalid::Parse { line }) => line,
        other => panic!("{text:?} gives {other:?}"),
    }
}

#[test]
fn an_empty_file_turns_the_correction_off() {
    assert_eq!(correction(""), Ok(false));
}

#[test]
fn the_refine_table_gives_the_correction_state() {
    assert_eq!(correction("[refine]\nenabled = true\n"), Ok(true));
    assert_eq!(correction("[refine]\nenabled = false\n"), Ok(false));
    assert_eq!(correction("[refine]\n"), Ok(false));
}

#[test]
fn schema_1_is_valid() {
    assert_eq!(correction("schema = 1\n"), Ok(false));
    assert_eq!(
        correction("schema = 1\n\n[refine]\nenabled = true\n"),
        Ok(true)
    );
}

#[test]
fn another_schema_is_an_error_with_its_line() {
    assert_eq!(
        parse("# Bloop\nschema = 2\n"),
        Err(Invalid::Schema { line: Some(2) })
    );
    assert_eq!(
        parse("schema = 0\n"),
        Err(Invalid::Schema { line: Some(1) })
    );
}

#[test]
fn an_unknown_key_is_an_error_without_the_file_content() {
    let error = parse("schema = 1\nsecret = \"dictée\"\n").unwrap_err();

    assert_eq!(error, Invalid::Parse { line: Some(2) });
    assert_eq!(format!("{error:?}"), "Parse { line: Some(2) }");
}

#[test]
fn an_unknown_refine_key_is_an_error() {
    assert_eq!(
        parse("[refine]\nenabled = true\nstyle = \"natural-light\"\n"),
        Err(Invalid::Parse { line: Some(3) })
    );
}

#[test]
fn a_value_of_another_type_is_an_error() {
    assert_eq!(
        parse("schema = 1\n\n[refine]\nenabled = \"yes\"\n"),
        Err(Invalid::Parse { line: Some(4) })
    );
    assert_eq!(
        parse("schema = \"1\"\n"),
        Err(Invalid::Parse { line: Some(1) })
    );
}

#[test]
fn an_empty_file_gives_the_default_settings() {
    let settings = parse("").unwrap();

    assert_eq!(settings, Settings::default());
    assert_eq!(
        settings.dictation.hold_ms.duration(),
        Duration::from_millis(220)
    );
    assert_eq!(settings.dictation.language.as_str(), "fr");
    assert_eq!(settings.transcriber, TranscriberConfig::default());
    assert_eq!(settings.chat, ChatConfig::default());
    assert!(!settings.refine.enabled);
    assert_eq!(settings.refine.prompt.as_str(), "natural-light");
}

#[test]
fn a_present_table_without_keys_gives_the_default_values() {
    let tables = [
        "[dictation]\n",
        "[transcriber]\nprovider = \"mistral\"\n",
        "[chat]\nprovider = \"mistral\"\n",
        "[refine]\n",
    ];
    for text in tables {
        assert_eq!(parse(text), Ok(Settings::default()), "{text}");
    }
}

#[test]
fn the_tables_give_their_values() {
    let text = "[dictation]\nhold_ms = 300\nlanguage = \"en\"\n\n\
                [transcriber]\nprovider = \"mistral\"\nmodel = \"voxtral-test\"\ntarget_streaming_delay_ms = 240\n\n\
                [chat]\nprovider = \"mistral\"\nmodel = \"mistral-small-latest\"\n\n\
                [refine]\nenabled = true\nprompt = \"dev\"\n";

    let settings = parse(text).unwrap();

    assert_eq!(
        settings.dictation.hold_ms.duration(),
        Duration::from_millis(300)
    );
    assert_eq!(settings.dictation.language.as_str(), "en");
    assert_ne!(settings.transcriber, TranscriberConfig::default());
    assert_eq!(settings.chat.model(), "mistral-small-latest");
    assert!(settings.refine.enabled);
    assert_eq!(settings.refine.prompt.as_str(), "dev");
}

#[test]
fn the_hold_time_accepts_100_to_1000_ms() {
    for value in [100, 1000] {
        let settings = parse(&format!("[dictation]\nhold_ms = {value}\n")).unwrap();

        assert_eq!(settings.dictation.hold_ms.get(), value);
    }
}

#[test]
fn a_hold_time_outside_its_limits_is_an_error_with_the_line_of_the_value() {
    for value in ["99", "1001", "0", "-1"] {
        let text = format!("schema = 1\n[dictation]\nhold_ms = {value}\n");

        assert_eq!(error_line(&text), Some(3), "{value}");
    }
}

#[test]
fn the_language_is_two_lowercase_ascii_letters() {
    for value in ["en", "de"] {
        let settings = parse(&format!("[dictation]\nlanguage = \"{value}\"\n")).unwrap();

        assert_eq!(settings.dictation.language.as_str(), value);
    }
    for value in ["", "f", "fra", "FR", "Fr", "é", "f1"] {
        let text = format!("[dictation]\nhold_ms = 220\nlanguage = \"{value}\"\n");

        assert_eq!(error_line(&text), Some(3), "{value}");
    }
}

#[test]
fn the_prompt_name_accepts_1_to_64_letters_digits_underscores_and_hyphens() {
    let longest = "a".repeat(64);
    for value in ["a", "Dev_2-b", longest.as_str()] {
        let settings = parse(&format!("[refine]\nprompt = \"{value}\"\n")).unwrap();

        assert_eq!(settings.refine.prompt.as_str(), value);
    }
}

#[test]
fn an_invalid_prompt_name_is_an_error_with_its_line() {
    let too_long = "a".repeat(65);
    for value in ["../x", "", too_long.as_str(), "a b", "dev.md", "/dev", "é"] {
        let text = format!("[refine]\nenabled = true\nprompt = \"{value}\"\n");

        assert_eq!(error_line(&text), Some(3), "{value}");
    }
}

#[test]
fn an_unknown_key_in_each_table_is_an_error_with_its_line() {
    let cases = [
        ("schema = 1\nvoice = 1\n", 2),
        ("[dictation]\nhold_ms = 220\nhold = 220\n", 3),
        ("[refine]\nenabled = true\nmodel = \"x\"\n", 3),
        ("[voice]\nenabled = true\n", 1),
    ];
    for (text, line) in cases {
        assert_eq!(error_line(text), Some(line), "{text}");
    }
}

#[test]
fn the_dictation_key_is_not_a_setting_yet() {
    assert_eq!(
        error_line("[dictation]\nkey = \"right_command\"\n"),
        Some(2)
    );
}

#[test]
fn an_unknown_key_in_a_provider_table_gives_the_line_of_the_table() {
    let cases = [
        "schema = 1\n\n[transcriber]\nprovider = \"mistral\"\nlanguage = \"fr\"\n",
        "schema = 1\n\n[chat]\nprovider = \"mistral\"\ntemperature = 0.5\n",
    ];
    for text in cases {
        assert_eq!(error_line(text), Some(3), "{text}");
    }
}

#[test]
fn a_wrong_type_is_an_error_with_its_line() {
    let cases = [
        ("dictation = 1\n", 1),
        ("[dictation]\nhold_ms = \"220\"\n", 2),
        ("[dictation]\nlanguage = 1\n", 2),
        ("[refine]\nprompt = 1\n", 2),
        ("[transcriber]\nprovider = 1\n", 2),
    ];
    for (text, line) in cases {
        assert_eq!(error_line(text), Some(line), "{text}");
    }
}

#[test]
fn a_wrong_type_in_a_provider_table_gives_the_line_of_the_table() {
    let cases = [
        "schema = 1\n\n[transcriber]\nprovider = \"mistral\"\ntarget_streaming_delay_ms = \"480\"\n",
        "schema = 1\n\n[chat]\nprovider = \"mistral\"\nmodels = \"mistral-small-latest\"\n",
    ];
    for text in cases {
        assert_eq!(error_line(text), Some(3), "{text}");
    }
}

#[test]
fn a_missing_provider_table_gives_the_default_config() {
    let settings = parse("schema = 1\n\n[refine]\nenabled = true\n").unwrap();

    assert_eq!(settings.transcriber, TranscriberConfig::default());
    assert_eq!(settings.chat, ChatConfig::default());
}

#[test]
fn a_present_provider_table_needs_the_provider_key() {
    let cases = [
        "schema = 1\n\n[transcriber]\nmodel = \"voxtral-test\"\n",
        "schema = 1\n\n[chat]\nmodel = \"mistral-small-latest\"\n",
    ];
    for text in cases {
        assert_eq!(error_line(text), Some(3), "{text}");
    }
}

#[test]
fn an_unknown_provider_is_an_error_with_its_own_line() {
    let cases = [
        "[transcriber]\nmodel = \"voxtral-test\"\nprovider = \"other\"\n",
        "[chat]\nmodel = \"mistral-small-latest\"\nprovider = \"Mistral\"\n",
    ];
    for text in cases {
        assert_eq!(error_line(text), Some(3), "{text}");
    }
}
