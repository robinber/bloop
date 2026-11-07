use std::fs;
use std::io;
use std::rc::Rc;
use std::time::Duration;

use bloop_core::error::{Permission, SecretError};
use bloop_core::ports::platform::{ActionId, MenuItem, MenuModel, MenuSource as _, SecretStore};
use secrecy::SecretString;

use super::{Action, AppMenu, Facts, build};
use crate::dictionary;
use crate::files::tests::scratch_dir;
use crate::hub::{Hub, Summary, View};
use crate::prompts::PromptError;
use crate::settings::{self, Paths, PromptName};

const READY: &str = "Ready: hold Right Command to dictate";
const RESTART: &str = "Then open Bloop again";

fn facts() -> Facts {
    Facts {
        missing: Vec::new(),
        tap: true,
        key: None,
    }
}

fn view() -> View {
    View {
        settings: Ok(Summary { correction: false }),
        prompt: None,
        dictionary: None,
        key_refused: false,
    }
}

fn name(value: &str) -> PromptName {
    PromptName::try_from(value.to_owned()).unwrap()
}

/// Returns the items before the first separator, with the action of each item from the action list.
fn status(facts: &Facts, view: &View) -> Vec<(String, Option<Action>)> {
    let (model, actions) = build(facts, view);
    model
        .items
        .iter()
        .take_while(|item| **item != MenuItem::Separator)
        .map(|item| match item {
            MenuItem::Status { title } => (title.clone(), None),
            MenuItem::Action { title, action } => (title.clone(), Some(actions[action.0].clone())),
            other => panic!("{other:?}"),
        })
        .collect()
}

fn text(title: &str) -> (String, Option<Action>) {
    (title.to_owned(), None)
}

fn opens(title: &str, action: Action) -> (String, Option<Action>) {
    (title.to_owned(), Some(action))
}

fn titles(model: &MenuModel) -> Vec<&str> {
    model
        .items
        .iter()
        .map(|item| match item {
            MenuItem::Status { title }
            | MenuItem::Action { title, .. }
            | MenuItem::Toggle { title, .. }
            | MenuItem::Choice { title, .. } => title.as_str(),
            MenuItem::Separator => "---",
        })
        .collect()
}

#[test]
fn with_nothing_to_do_the_menu_says_how_to_dictate_and_shows_each_item() {
    let (model, actions) = build(&facts(), &view());

    assert_eq!(
        titles(&model),
        [
            READY,
            "---",
            "Correct text",
            "---",
            "Copy last result",
            "Open config.toml…",
            "Open prompts folder…",
            "Open dictionary…",
        ]
    );
    assert_eq!(
        actions,
        [
            Action::ToggleCorrection,
            Action::CopyLastResult,
            Action::OpenConfig,
            Action::OpenPrompts,
            Action::OpenDictionary,
        ]
    );
}

#[test]
fn each_action_id_is_the_index_of_its_action() {
    let facts = Facts {
        missing: vec![Permission::Microphone],
        ..facts()
    };
    let view = View {
        dictionary: Some(dictionary::Invalid::Parse { line: Some(2) }),
        ..view()
    };
    let (model, actions) = build(&facts, &view);

    let ids: Vec<ActionId> = model
        .items
        .iter()
        .filter_map(|item| match item {
            MenuItem::Action { action, .. } | MenuItem::Toggle { action, .. } => Some(*action),
            _ => None,
        })
        .collect();

    assert_eq!(ids, (0..actions.len()).map(ActionId).collect::<Vec<_>>());
    assert_eq!(actions[0], Action::OpenPermission(Permission::Microphone));
    assert_eq!(actions[1], Action::OpenDictionary);
    assert_eq!(actions[2], Action::ToggleCorrection);
}

#[test]
fn the_checkmark_of_correct_text_follows_the_settings() {
    for correction in [false, true] {
        let view = View {
            settings: Ok(Summary { correction }),
            ..view()
        };
        let (model, _) = build(&facts(), &view);

        assert!(
            model.items.contains(&MenuItem::Toggle {
                title: "Correct text".to_owned(),
                action: ActionId(0),
                checked: correction,
            }),
            "{model:?}"
        );
    }
}

#[test]
fn each_missing_permission_opens_its_settings() {
    let facts = Facts {
        missing: vec![Permission::InputMonitoring, Permission::Accessibility],
        tap: false,
        ..facts()
    };

    assert_eq!(
        status(&facts, &view()),
        [
            opens(
                "Allow Input Monitoring…",
                Action::OpenPermission(Permission::InputMonitoring)
            ),
            opens(
                "Allow Accessibility access…",
                Action::OpenPermission(Permission::Accessibility)
            ),
            text(RESTART),
        ]
    );
}

#[test]
fn a_refused_tap_without_missing_permissions_asks_for_a_restart() {
    let facts = Facts {
        tap: false,
        ..facts()
    };

    assert_eq!(
        status(&facts, &view()),
        [text("Open Bloop again to use the dictation key")]
    );
}

#[test]
fn a_missing_key_asks_to_add_it() {
    let facts = Facts {
        key: Some(SecretError::NotFound),
        ..facts()
    };

    assert_eq!(
        status(&facts, &view()),
        [text("Add the API key to Keychain"), text(RESTART)]
    );
}

#[test]
fn the_key_line_follows_the_missing_permissions() {
    let facts = Facts {
        missing: vec![Permission::Accessibility],
        tap: false,
        key: Some(SecretError::Unavailable),
    };

    assert_eq!(
        status(&facts, &view()),
        [
            opens(
                "Allow Accessibility access…",
                Action::OpenPermission(Permission::Accessibility)
            ),
            text("Allow Bloop to read the API key in Keychain"),
            text(RESTART),
        ]
    );
}

#[test]
fn a_refused_key_asks_for_a_restart() {
    let view = View {
        key_refused: true,
        ..view()
    };

    assert_eq!(
        status(&facts(), &view),
        [text("The provider refused the API key"), text(RESTART)]
    );
}

#[test]
fn an_invalid_config_opens_the_file_and_hides_correct_text() {
    for (error, title) in [
        (
            settings::Invalid::Parse { line: Some(3) },
            "config.toml: invalid setting, line 3",
        ),
        (
            settings::Invalid::Schema { line: Some(1) },
            "config.toml: invalid setting, line 1",
        ),
        (
            settings::Invalid::Parse { line: None },
            "config.toml: invalid setting",
        ),
        (settings::Invalid::TooLarge, "config.toml: file too large"),
        (
            settings::Invalid::Unreadable(io::ErrorKind::PermissionDenied),
            "config.toml: file not readable",
        ),
    ] {
        let view = View {
            settings: Err(error),
            ..view()
        };
        let (model, _) = build(&facts(), &view);

        assert_eq!(status(&facts(), &view), [opens(title, Action::OpenConfig)]);
        assert_eq!(
            titles(&model),
            [
                title,
                "---",
                "Copy last result",
                "Open config.toml…",
                "Open prompts folder…",
                "Open dictionary…",
            ]
        );
    }
}

#[test]
fn a_prompt_error_opens_the_prompts_folder() {
    for (error, title) in [
        (PromptError::Missing, "Prompt \"dev\": not found"),
        (PromptError::Empty, "Prompt \"dev\": not valid"),
        (PromptError::TooLarge, "Prompt \"dev\": not valid"),
        (
            PromptError::Unreadable(io::ErrorKind::InvalidData),
            "Prompt \"dev\": not valid",
        ),
    ] {
        let view = View {
            settings: Ok(Summary { correction: true }),
            prompt: Some((name("dev"), error)),
            ..view()
        };

        assert_eq!(status(&facts(), &view), [opens(title, Action::OpenPrompts)]);
        assert!(titles(&build(&facts(), &view).0).contains(&"Correct text"));
    }
}

#[test]
fn an_invalid_dictionary_opens_the_file() {
    for (error, title) in [
        (
            dictionary::Invalid::Parse { line: Some(5) },
            "dictionary.toml: invalid entry, line 5",
        ),
        (
            dictionary::Invalid::EmptyWritten { line: Some(2) },
            "dictionary.toml: invalid entry, line 2",
        ),
        (
            dictionary::Invalid::Parse { line: None },
            "dictionary.toml: invalid entry",
        ),
        (
            dictionary::Invalid::TooLarge,
            "dictionary.toml: file too large",
        ),
        (
            dictionary::Invalid::Unreadable(io::ErrorKind::InvalidData),
            "dictionary.toml: file not readable",
        ),
    ] {
        let view = View {
            dictionary: Some(error),
            ..view()
        };

        assert_eq!(
            status(&facts(), &view),
            [opens(title, Action::OpenDictionary)]
        );
    }
}

#[test]
fn the_restart_lines_come_before_the_file_errors() {
    let facts = Facts {
        missing: vec![Permission::Microphone],
        tap: true,
        key: None,
    };
    let view = View {
        settings: Ok(Summary { correction: true }),
        prompt: Some((name("dev"), PromptError::Missing)),
        dictionary: Some(dictionary::Invalid::Parse { line: Some(4) }),
        key_refused: true,
    };

    assert_eq!(
        status(&facts, &view),
        [
            opens(
                "Allow Microphone access…",
                Action::OpenPermission(Permission::Microphone)
            ),
            text("The provider refused the API key"),
            text(RESTART),
            opens("Prompt \"dev\": not found", Action::OpenPrompts),
            opens(
                "dictionary.toml: invalid entry, line 4",
                Action::OpenDictionary
            ),
        ]
    );
}

#[test]
fn the_config_error_comes_before_the_dictionary_error() {
    let view = View {
        settings: Err(settings::Invalid::Parse { line: Some(7) }),
        dictionary: Some(dictionary::Invalid::TooLarge),
        ..view()
    };

    assert_eq!(
        status(&facts(), &view),
        [
            opens("config.toml: invalid setting, line 7", Action::OpenConfig),
            opens("dictionary.toml: file too large", Action::OpenDictionary),
        ]
    );
}

struct Keys;

impl SecretStore for Keys {
    fn get(&self, _account: &str) -> Result<SecretString, SecretError> {
        Ok(SecretString::from("test-key"))
    }
}

fn app_menu(dir: &std::path::Path) -> AppMenu {
    let paths = Paths::new(dir);
    let hub = Rc::new(Hub::start(paths.clone(), &Keys, Duration::from_secs(8)));
    AppMenu::new(hub, facts(), paths)
}

fn toggle(model: &MenuModel) -> (ActionId, bool) {
    model
        .items
        .iter()
        .find_map(|item| match item {
            MenuItem::Toggle {
                action, checked, ..
            } => Some((*action, *checked)),
            _ => None,
        })
        .expect("a toggle")
}

#[test]
fn a_click_on_correct_text_writes_the_file_and_the_next_model_shows_it() {
    let dir = scratch_dir("menu-toggle");
    fs::write(dir.join("config.toml"), "schema = 1\n").unwrap();
    let menu = app_menu(&dir);

    let (id, checked) = toggle(&menu.model());
    assert!(!checked);
    menu.activate(id);

    assert_eq!(
        fs::read_to_string(dir.join("config.toml")).unwrap(),
        "schema = 1\n\n[refine]\nenabled = true\n"
    );
    let (id, checked) = toggle(&menu.model());
    assert!(checked);
    menu.activate(id);
    assert!(!toggle(&menu.model()).1);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_checkmark_follows_a_hand_edit_at_the_next_open() {
    let dir = scratch_dir("menu-hand-edit");
    fs::write(dir.join("config.toml"), "schema = 1\n").unwrap();
    let menu = app_menu(&dir);
    assert!(!toggle(&menu.model()).1);

    fs::write(
        dir.join("config.toml"),
        "schema = 1\n\n[refine]\nenabled = true\n",
    )
    .unwrap();

    assert!(toggle(&menu.model()).1);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_unknown_action_has_no_effect() {
    let dir = scratch_dir("menu-unknown");
    let text = "schema = 1\n";
    fs::write(dir.join("config.toml"), text).unwrap();
    let menu = app_menu(&dir);
    let actions = build(&facts(), &view()).1.len();

    menu.activate(ActionId(0));
    menu.model();
    menu.activate(ActionId(actions));
    menu.activate(ActionId(usize::MAX));

    assert_eq!(fs::read_to_string(dir.join("config.toml")).unwrap(), text);
    fs::remove_dir_all(&dir).unwrap();
}
