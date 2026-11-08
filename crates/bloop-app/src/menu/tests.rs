use std::cell::{Cell, OnceCell};
use std::fs;
use std::io;
use std::rc::{Rc, Weak};
use std::time::Duration;

use bloop_core::error::{Permission, SecretError};
use bloop_core::ports::platform::{ActionId, MenuItem, MenuModel, MenuSource as _, SecretStore};
use bloop_providers::ChatConfig;
use secrecy::SecretString;

use super::{Action, AppMenu, AppNames, Facts, Lookups, build};
use crate::dictionary;
use crate::files::tests::scratch_dir;
use crate::hub::{Hub, Summary, View};
use crate::prompts::{self, AppLists, PromptError};
use crate::settings::{self, AppId, HoldMs, Paths, PromptName};

const READY: &str = "Ready: hold Right Command to dictate";
const RESTART: &str = "Then open Bloop again";
const TERMINAL: &str = "com.apple.Terminal";
const GHOSTTY: &str = "com.mitchellh.ghostty";
const WHATSAPP: &str = "net.whatsapp.WhatsApp";
const NOTES: &str = "com.apple.Notes";

fn facts() -> Facts {
    Facts {
        missing: Vec::new(),
        tap: true,
        key: None,
    }
}

fn summary(correction: bool) -> Summary {
    Summary {
        correction,
        prompt: name("natural-light"),
        model: "mistral-medium-latest".to_owned(),
        models: vec![
            "mistral-medium-latest".to_owned(),
            "mistral-small-latest".to_owned(),
        ],
        hold: hold(220),
    }
}

fn view() -> View {
    View {
        settings: Ok(summary(false)),
        prompt: None,
        routed: Vec::new(),
        dictionary: None,
        key_refused: false,
        prompts: vec![name("dev"), name("natural-light")],
        lists: AppLists::new(),
    }
}

fn name(value: &str) -> PromptName {
    PromptName::try_from(value.to_owned()).unwrap()
}

fn hold(ms: u64) -> HoldMs {
    HoldMs::try_from(ms).unwrap()
}

fn id(value: &str) -> AppId {
    AppId::try_from(value.to_owned()).unwrap()
}

fn lists(lists: &[(&str, &[&str])]) -> AppLists {
    lists
        .iter()
        .map(|(prompt, apps)| (name(prompt), apps.iter().map(|app| id(app)).collect()))
        .collect()
}

/// Returns the name that macOS gives to an installed application of the tests: Terminal, Ghostty, and `WhatsApp`.
fn installed_name(app: &str) -> Option<String> {
    match app {
        TERMINAL => Some("Mac Terminal".to_owned()),
        GHOSTTY => Some("Ghostty".to_owned()),
        WHATSAPP => Some("WhatsApp".to_owned()),
        _ => None,
    }
}

/// The applications of the tests: the names of the installed applications, and the open applications `running`.
fn apps(running: &[(&str, &str)]) -> AppNames {
    AppNames {
        names: [TERMINAL, GHOSTTY, WHATSAPP]
            .into_iter()
            .filter_map(|app| Some((id(app), installed_name(app)?)))
            .collect(),
        running: running
            .iter()
            .map(|(app, name)| (id(app), (*name).to_owned()))
            .collect(),
    }
}

/// A view with the prompts `code`, `dev`, and `natural-light`, and their lists.
fn routing() -> View {
    View {
        prompts: vec![name("code"), name("dev"), name("natural-light")],
        lists: lists(&[
            ("code", &[TERMINAL, GHOSTTY]),
            ("natural-light", &[WHATSAPP]),
        ]),
        ..view()
    }
}

/// Returns the items of the submenu at `path`: the title of each submenu from the top of the menu.
fn submenu<'a>(model: &'a MenuModel, path: &[&str]) -> &'a [MenuItem] {
    let mut items = model.items.as_slice();
    for title in path {
        items = items
            .iter()
            .find_map(|item| match item {
                MenuItem::Submenu {
                    title: name, items, ..
                } if name == title => Some(items.as_slice()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no submenu {path:?}: {model:?}"));
    }
    items
}

/// Returns the options of the list `title`: the title of each option, and `true` for the selected option.
fn options(model: &MenuModel, title: &str) -> Vec<(String, bool)> {
    submenu(model, &[title])
        .iter()
        .map(|item| match item {
            MenuItem::Toggle { title, checked, .. } => (title.clone(), *checked),
            other => panic!("{other:?}"),
        })
        .collect()
}

fn shown(titles: &[&str], selected: &str) -> Vec<(String, bool)> {
    titles
        .iter()
        .map(|title| ((*title).to_owned(), *title == selected))
        .collect()
}

/// Returns the items before the first separator, with the action of each item from the action list.
fn status(facts: &Facts, view: &View) -> Vec<(String, Option<Action>)> {
    let (model, actions) = build(facts, view, &AppNames::default());
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

/// Returns the title of each item of `items`, and `---` for a separator.
fn titles(items: &[MenuItem]) -> Vec<&str> {
    items
        .iter()
        .map(|item| match item {
            MenuItem::Status { title }
            | MenuItem::Action { title, .. }
            | MenuItem::Toggle { title, .. }
            | MenuItem::Submenu { title, .. } => title.as_str(),
            MenuItem::Separator => "---",
        })
        .collect()
}

/// Returns the action identifier of each item of `items` and of their submenus, in display order.
fn action_ids(items: &[MenuItem]) -> Vec<ActionId> {
    items
        .iter()
        .flat_map(|item| match item {
            MenuItem::Action { action, .. } | MenuItem::Toggle { action, .. } => vec![*action],
            MenuItem::Submenu { items, .. } => action_ids(items),
            MenuItem::Status { .. } | MenuItem::Separator => Vec::new(),
        })
        .collect()
}

fn model(view: &View) -> (MenuModel, Vec<Action>) {
    build(&facts(), view, &AppNames::default())
}

#[test]
fn with_nothing_to_do_the_menu_says_how_to_dictate_and_shows_each_item() {
    let (model, actions) = model(&view());

    assert_eq!(
        titles(&model.items),
        [
            READY,
            "---",
            "Correct text",
            "Prompt",
            "Model",
            "Hold delay",
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
            Action::Prompt(name("dev")),
            Action::ChooseApp {
                prompt: name("dev")
            },
            Action::Prompt(name("natural-light")),
            Action::ChooseApp {
                prompt: name("natural-light")
            },
            Action::Model("mistral-medium-latest".to_owned()),
            Action::Model("mistral-small-latest".to_owned()),
            Action::Hold(hold(150)),
            Action::Hold(hold(220)),
            Action::Hold(hold(300)),
            Action::Hold(hold(400)),
            Action::CopyLastResult,
            Action::OpenConfig,
            Action::OpenPrompts,
            Action::OpenDictionary,
        ]
    );
}

/// Returns the title of each prompt submenu of "Prompt", and `true` for the submenu with a checkmark.
fn prompt_marks(model: &MenuModel) -> Vec<(String, bool)> {
    submenu(model, &["Prompt"])
        .iter()
        .map(|item| match item {
            MenuItem::Submenu { title, checked, .. } => (title.clone(), *checked),
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn each_list_has_a_checkmark_on_the_current_value() {
    let (model, _) = model(&view());

    assert_eq!(
        prompt_marks(&model),
        shown(&["dev", "natural-light"], "natural-light")
    );
    assert_eq!(
        options(&model, "Model"),
        shown(
            &["mistral-medium-latest", "mistral-small-latest"],
            "mistral-medium-latest"
        )
    );
    assert_eq!(
        options(&model, "Hold delay"),
        shown(&["150 ms", "220 ms", "300 ms", "400 ms"], "220 ms")
    );
}

#[test]
fn a_current_value_outside_its_list_is_added_at_the_end_and_selected() {
    let view = View {
        settings: Ok(Summary {
            prompt: name("absent"),
            model: "ministral-8b-2512".to_owned(),
            hold: hold(250),
            ..summary(false)
        }),
        ..view()
    };
    let (model, actions) = model(&view);

    assert_eq!(
        prompt_marks(&model),
        shown(&["dev", "natural-light", "absent"], "absent")
    );
    assert_eq!(
        options(&model, "Model"),
        shown(
            &[
                "mistral-medium-latest",
                "mistral-small-latest",
                "ministral-8b-2512"
            ],
            "ministral-8b-2512"
        )
    );
    assert_eq!(
        options(&model, "Hold delay"),
        shown(
            &["150 ms", "220 ms", "300 ms", "400 ms", "250 ms"],
            "250 ms"
        )
    );
    assert!(actions.contains(&Action::Prompt(name("absent"))));
    assert!(actions.contains(&Action::Model("ministral-8b-2512".to_owned())));
    assert!(actions.contains(&Action::Hold(hold(250))));
}

#[test]
fn a_model_listed_twice_has_one_option() {
    let view = View {
        settings: Ok(Summary {
            models: vec![
                "mistral-small-latest".to_owned(),
                "mistral-medium-latest".to_owned(),
                "mistral-small-latest".to_owned(),
            ],
            ..summary(false)
        }),
        ..view()
    };

    assert_eq!(
        options(&model(&view).0, "Model"),
        shown(
            &["mistral-small-latest", "mistral-medium-latest"],
            "mistral-medium-latest"
        )
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
        ..routing()
    };
    let (model, actions) = build(&facts, &view, &apps(&[(NOTES, "Notes")]));

    let ids = action_ids(&model.items);

    assert_eq!(ids, (0..actions.len()).map(ActionId).collect::<Vec<_>>());
    for action in [
        Action::RemoveApp {
            prompt: name("code"),
            app: id(GHOSTTY),
        },
        Action::AddApp {
            prompt: name("dev"),
            app: id(NOTES),
        },
        Action::ChooseApp {
            prompt: name("natural-light"),
        },
    ] {
        assert!(actions.contains(&action), "{action:?} in {actions:?}");
    }
    assert_eq!(actions[0], Action::OpenPermission(Permission::Microphone));
    assert_eq!(actions[1], Action::OpenDictionary);
    assert_eq!(actions[2], Action::ToggleCorrection);
    assert_eq!(actions[3], Action::Prompt(name("code")));
}

/// An item of a submenu: its title, its action, and its checkmark. A separator is `---`.
type Entry = (String, Option<Action>, bool);

/// Returns the items of the submenu at `path`. An item with a submenu has no action.
fn entries(model: &(MenuModel, Vec<Action>), path: &[&str]) -> Vec<Entry> {
    let (model, actions) = model;
    submenu(model, path)
        .iter()
        .map(|item| match item {
            MenuItem::Status { title } => (title.clone(), None, false),
            MenuItem::Action { title, action } => {
                (title.clone(), Some(actions[action.0].clone()), false)
            }
            MenuItem::Toggle {
                title,
                action,
                checked,
            } => (title.clone(), Some(actions[action.0].clone()), *checked),
            MenuItem::Submenu { title, checked, .. } => (title.clone(), None, *checked),
            MenuItem::Separator => ("---".to_owned(), None, false),
        })
        .collect()
}

fn line() -> Entry {
    ("---".to_owned(), None, false)
}

fn note(title: &str) -> Entry {
    (title.to_owned(), None, false)
}

fn default_prompt(prompt: &str, checked: bool) -> Entry {
    let action = Action::Prompt(name(prompt));
    ("Default prompt".to_owned(), Some(action), checked)
}

fn remove(prompt: &str, title: &str, app: &str) -> Entry {
    let action = Action::RemoveApp {
        prompt: name(prompt),
        app: id(app),
    };
    (title.to_owned(), Some(action), true)
}

fn add(prompt: &str, title: &str, app: &str) -> Entry {
    let action = Action::AddApp {
        prompt: name(prompt),
        app: id(app),
    };
    (title.to_owned(), Some(action), false)
}

fn choose(prompt: &str) -> Entry {
    let action = Action::ChooseApp {
        prompt: name(prompt),
    };
    ("Choose…".to_owned(), Some(action), false)
}

fn add_application() -> Entry {
    ("Add application".to_owned(), None, false)
}

#[test]
fn the_prompt_submenu_has_a_submenu_for_each_prompt_and_a_checkmark_on_the_default_only() {
    let view = View {
        settings: Ok(Summary {
            prompt: name("code"),
            ..summary(true)
        }),
        ..routing()
    };
    let model = build(&facts(), &view, &apps(&[]));

    assert_eq!(
        prompt_marks(&model.0),
        shown(&["code", "dev", "natural-light"], "code")
    );
    for (prompt, checked) in [("code", true), ("dev", false), ("natural-light", false)] {
        assert_eq!(
            entries(&model, &["Prompt", prompt])[0],
            default_prompt(prompt, checked)
        );
    }
}

#[test]
fn a_prompt_submenu_has_default_prompt_then_its_apps_then_add_application() {
    let model = build(&facts(), &routing(), &apps(&[]));

    assert_eq!(
        entries(&model, &["Prompt", "code"]),
        [
            default_prompt("code", false),
            line(),
            remove("code", "Mac Terminal", TERMINAL),
            remove("code", "Ghostty", GHOSTTY),
            line(),
            add_application(),
        ]
    );
    assert_eq!(
        entries(&model, &["Prompt", "natural-light"]),
        [
            default_prompt("natural-light", true),
            line(),
            remove("natural-light", "WhatsApp", WHATSAPP),
            line(),
            add_application(),
        ]
    );
    assert_eq!(
        entries(&model, &["Prompt", "dev"]),
        [
            default_prompt("dev", false),
            line(),
            note("No application"),
            line(),
            add_application(),
        ]
    );
}

#[test]
fn add_application_lists_the_open_apps_that_the_list_lacks_then_choose() {
    let model = build(
        &facts(),
        &routing(),
        &apps(&[(GHOSTTY, "Ghostty"), (NOTES, "Notes")]),
    );

    assert_eq!(
        entries(&model, &["Prompt", "code", "Add application"]),
        [add("code", "Notes", NOTES), line(), choose("code")]
    );
    assert_eq!(
        entries(&model, &["Prompt", "dev", "Add application"]),
        [
            add("dev", "Ghostty", GHOSTTY),
            add("dev", "Notes", NOTES),
            line(),
            choose("dev"),
        ]
    );
}

#[test]
fn without_an_open_app_to_add_add_application_has_only_choose() {
    for running in [&[][..], &[(TERMINAL, "Terminal"), (GHOSTTY, "Ghostty")][..]] {
        let model = build(&facts(), &routing(), &apps(running));

        assert_eq!(
            entries(&model, &["Prompt", "code", "Add application"]),
            [choose("code")],
            "{running:?}"
        );
    }
}

#[test]
fn an_app_name_is_the_name_of_macos_else_the_bundle_identifier() {
    let view = View {
        lists: lists(&[("code", &[TERMINAL, "com.example.editor"])]),
        ..routing()
    };
    let model = build(&facts(), &view, &apps(&[]));

    assert_eq!(
        titles(submenu(&model.0, &["Prompt", "code"])),
        [
            "Default prompt",
            "---",
            "Mac Terminal",
            "com.example.editor",
            "---",
            "Add application"
        ]
    );
}

#[test]
fn a_key_of_another_prompt_name_has_its_apps_without_default_prompt_or_add_application() {
    let view = View {
        lists: lists(&[
            ("code", &[TERMINAL]),
            ("mail", &["com.apple.mail"]),
            ("unused", &[]),
        ]),
        ..routing()
    };
    let model = build(&facts(), &view, &apps(&[(NOTES, "Notes")]));

    assert_eq!(
        titles(submenu(&model.0, &["Prompt"])),
        ["code", "dev", "natural-light", "mail"]
    );
    assert_eq!(
        entries(&model, &["Prompt", "mail"]),
        [remove("mail", "com.apple.mail", "com.apple.mail")]
    );
}

#[test]
fn the_checkmark_of_correct_text_follows_the_settings() {
    for correction in [false, true] {
        let view = View {
            settings: Ok(summary(correction)),
            ..view()
        };
        let (model, _) = model(&view);

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
        let (model, _) = model(&view);

        assert_eq!(status(&facts(), &view), [opens(title, Action::OpenConfig)]);
        assert_eq!(
            titles(&model.items),
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
            settings: Ok(summary(true)),
            prompt: Some((name("dev"), error)),
            ..view()
        };

        assert_eq!(status(&facts(), &view), [opens(title, Action::OpenPrompts)]);
        assert!(titles(&model(&view).0.items).contains(&"Correct text"));
    }
}

#[test]
fn each_routed_prompt_error_follows_the_selected_prompt_error() {
    let view = View {
        settings: Ok(summary(true)),
        prompt: Some((name("dev"), PromptError::Missing)),
        routed: vec![
            (name("code"), PromptError::Empty),
            (name("mail"), PromptError::Missing),
        ],
        ..view()
    };

    assert_eq!(
        status(&facts(), &view),
        [
            opens("Prompt \"dev\": not found", Action::OpenPrompts),
            opens("Prompt \"code\": not valid", Action::OpenPrompts),
            opens("Prompt \"mail\": not found", Action::OpenPrompts),
        ]
    );
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
        settings: Ok(summary(true)),
        prompt: Some((name("dev"), PromptError::Missing)),
        dictionary: Some(dictionary::Invalid::Parse { line: Some(4) }),
        key_refused: true,
        ..view()
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

/// The open applications of the tests, unsorted.
fn running() -> Vec<(String, String)> {
    vec![
        (NOTES.to_owned(), "Notes".to_owned()),
        (GHOSTTY.to_owned(), "Ghostty".to_owned()),
    ]
}

fn app_menu(dir: &std::path::Path) -> AppMenu {
    menu_with(dir, running, |_| Box::new(|| None))
}

/// Returns the menu of the settings folder `dir`, with the open applications of `running` and the file panel that
/// `choose` returns for the hub of the menu.
/// The installed applications are Terminal, Ghostty, and `WhatsApp`. They make the default lists.
fn menu_with(
    dir: &std::path::Path,
    running: fn() -> Vec<(String, String)>,
    choose: impl FnOnce(Rc<Hub>) -> Box<dyn Fn() -> Option<String>>,
) -> AppMenu {
    let defaults = prompts::default_lists(|app| installed_name(app).is_some());
    let hub = Rc::new(Hub::start(
        Paths::new(dir),
        &Keys,
        Duration::from_secs(8),
        defaults,
    ));
    let lookups = Lookups {
        name: installed_name,
        running,
        choose: choose(Rc::clone(&hub)),
    };
    AppMenu::new(hub, facts(), lookups)
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

    menu.activate(ActionId(0));
    menu.model();
    let actions = menu.last.borrow().1.len();
    menu.activate(ActionId(actions));
    menu.activate(ActionId(usize::MAX));

    assert_eq!(fs::read_to_string(dir.join("config.toml")).unwrap(), text);
    fs::remove_dir_all(&dir).unwrap();
}

/// Returns the action of the item `title` in the submenu at `path`.
fn option_at(model: &MenuModel, path: &[&str], title: &str) -> ActionId {
    submenu(model, path)
        .iter()
        .find_map(|item| match item {
            MenuItem::Action {
                title: name,
                action,
            }
            | MenuItem::Toggle {
                title: name,
                action,
                ..
            } if name == title => Some(*action),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no item {title} in {path:?}: {model:?}"))
}

fn option(model: &MenuModel, list: &str, title: &str) -> ActionId {
    option_at(model, &[list], title)
}

#[test]
fn a_click_on_each_list_option_writes_its_key() {
    let dir = scratch_dir("menu-lists");
    let text = "schema = 1\n\n[refine]\nenabled = true\n";
    fs::write(dir.join("config.toml"), text).unwrap();
    fs::create_dir(dir.join("prompts")).unwrap();
    fs::write(dir.join("prompts").join("dev.md"), "Corrige.\n").unwrap();
    let menu = app_menu(&dir);

    menu.activate(option_at(
        &menu.model(),
        &["Prompt", "dev"],
        "Default prompt",
    ));
    menu.activate(option(&menu.model(), "Model", "mistral-small-latest"));
    menu.activate(option(&menu.model(), "Hold delay", "400 ms"));

    let new = fs::read_to_string(dir.join("config.toml")).unwrap();
    assert_eq!(
        new,
        format!(
            "{text}prompt = \"dev\"\n\n[chat]\nprovider = \"{}\"\nmodel = \"mistral-small-latest\"\n\n[dictation]\nhold_ms = 400\n",
            ChatConfig::default().provider()
        )
    );
    let model = menu.model();
    assert_eq!(
        prompt_marks(&model),
        shown(&["code", "dev", "natural-light"], "dev")
    );
    assert_eq!(
        options(&model, "Model"),
        shown(
            &["mistral-medium-latest", "mistral-small-latest"],
            "mistral-small-latest"
        )
    );
    assert_eq!(
        options(&model, "Hold delay"),
        shown(&["150 ms", "220 ms", "300 ms", "400 ms"], "400 ms")
    );
    assert_eq!(titles(&model.items).first(), Some(&READY));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn with_invalid_settings_a_click_on_an_old_list_option_changes_nothing() {
    let dir = scratch_dir("menu-lists-invalid");
    fs::write(dir.join("config.toml"), "schema = 1\n").unwrap();
    let menu = app_menu(&dir);
    let id = option(&menu.model(), "Hold delay", "400 ms");
    let invalid = "schema = 1\n\n[dictation]\nhold_ms = 5000\n";
    fs::write(dir.join("config.toml"), invalid).unwrap();

    menu.activate(id);

    assert_eq!(
        fs::read_to_string(dir.join("config.toml")).unwrap(),
        invalid
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_open_apps_are_sorted_by_name_without_duplicates_or_invalid_identifiers() {
    let dir = scratch_dir("menu-running");
    fs::write(dir.join("config.toml"), "schema = 1\n").unwrap();
    let menu = menu_with(
        &dir,
        || {
            [
                (NOTES, "Notes"),
                ("dev.zed.Zed", "zed"),
                ("bad id", "Bad"),
                (GHOSTTY, "Ghostty"),
                (NOTES, "Notes again"),
                ("com.apple.mail", "Mail"),
            ]
            .map(|(app, name)| (app.to_owned(), name.to_owned()))
            .to_vec()
        },
        |_| Box::new(|| None),
    );

    let model = menu.model();

    assert_eq!(
        titles(submenu(
            &model,
            &["Prompt", "natural-light", "Add application"]
        )),
        ["Ghostty", "Mail", "Notes", "zed", "---", "Choose…"]
    );
    assert_eq!(
        titles(submenu(&model, &["Prompt", "code", "Add application"])),
        ["Mail", "Notes", "zed", "---", "Choose…"]
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_click_on_an_open_app_moves_it_and_the_first_change_writes_every_default_list() {
    let dir = scratch_dir("menu-add-app");
    let text = "schema = 1\n\n[refine]\nenabled = true\n";
    fs::write(dir.join("config.toml"), text).unwrap();
    let menu = app_menu(&dir);
    let model = menu.model();

    menu.activate(option_at(
        &model,
        &["Prompt", "natural-light", "Add application"],
        "Ghostty",
    ));

    assert_eq!(
        fs::read_to_string(dir.join("config.toml")).unwrap(),
        format!(
            "{text}\n[refine.apps]\ncode = [\"{TERMINAL}\"]\nnatural-light = [\"{WHATSAPP}\", \"{GHOSTTY}\"]\n"
        )
    );
    let model = menu.model();
    assert_eq!(
        titles(submenu(&model, &["Prompt", "natural-light"]))[2..4],
        ["WhatsApp", "Ghostty"]
    );
    assert!(!titles(submenu(&model, &["Prompt", "code"])).contains(&"Ghostty"));
    assert!(titles(submenu(&model, &["Prompt", "code", "Add application"])).contains(&"Ghostty"));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_click_on_a_listed_app_removes_it_and_the_first_change_writes_every_default_list() {
    let dir = scratch_dir("menu-remove-app");
    fs::write(dir.join("config.toml"), "schema = 1\n").unwrap();
    let menu = app_menu(&dir);
    let model = menu.model();

    menu.activate(option_at(&model, &["Prompt", "natural-light"], "WhatsApp"));

    assert_eq!(
        fs::read_to_string(dir.join("config.toml")).unwrap(),
        format!(
            "schema = 1\n\n[refine.apps]\ncode = [\"{TERMINAL}\", \"{GHOSTTY}\"]\nnatural-light = []\n"
        )
    );
    let model = menu.model();
    assert_eq!(
        titles(submenu(&model, &["Prompt", "natural-light"]))[2],
        "No application"
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn choose_adds_the_chosen_app_and_the_hub_is_free_while_the_panel_runs() {
    let dir = scratch_dir("menu-choose");
    let text = "schema = 1\n\n[refine]\nenabled = true\n";
    fs::write(dir.join("config.toml"), text).unwrap();
    fs::create_dir(dir.join("prompts")).unwrap();
    fs::write(dir.join("prompts").join("dev.md"), "Corrige.\n").unwrap();
    let menu = menu_with(&dir, running, |hub| {
        Box::new(move || {
            assert!(hub.refresh(Some(GHOSTTY)).is_some());
            assert!(hub.view().is_some());
            Some(NOTES.to_owned())
        })
    });
    let model = menu.model();

    menu.activate(option_at(
        &model,
        &["Prompt", "dev", "Add application"],
        "Choose…",
    ));

    assert_eq!(
        fs::read_to_string(dir.join("config.toml")).unwrap(),
        format!(
            "{text}\n[refine.apps]\ncode = [\"{TERMINAL}\", \"{GHOSTTY}\"]\ndev = [\"{NOTES}\"]\nnatural-light = [\"{WHATSAPP}\"]\n"
        )
    );
    assert_eq!(titles(submenu(&menu.model(), &["Prompt", "dev"]))[2], NOTES);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_choose_click_while_the_panel_runs_has_no_effect() {
    let dir = scratch_dir("menu-choose-twice");
    let text = "schema = 1\n\n[refine]\nenabled = true\n";
    fs::write(dir.join("config.toml"), text).unwrap();
    let slot: Rc<OnceCell<Weak<AppMenu>>> = Rc::new(OnceCell::new());
    let panels = Rc::new(Cell::new(0));
    let menu = Rc::new(menu_with(&dir, running, |_| {
        let slot = Rc::clone(&slot);
        let panels = Rc::clone(&panels);
        Box::new(move || {
            panels.set(panels.get() + 1);
            let menu = slot.get().unwrap().upgrade().unwrap();
            let model = menu.model();
            menu.activate(option_at(
                &model,
                &["Prompt", "code", "Add application"],
                "Choose…",
            ));
            Some(NOTES.to_owned())
        })
    }));
    assert!(slot.set(Rc::downgrade(&menu)).is_ok());
    let model = menu.model();

    menu.activate(option_at(
        &model,
        &["Prompt", "code", "Add application"],
        "Choose…",
    ));

    assert_eq!(panels.get(), 1);
    assert_eq!(
        fs::read_to_string(dir.join("config.toml")).unwrap(),
        format!(
            "{text}\n[refine.apps]\ncode = [\"{TERMINAL}\", \"{GHOSTTY}\", \"{NOTES}\"]\nnatural-light = [\"{WHATSAPP}\"]\n"
        )
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_cancelled_or_invalid_choice_changes_nothing() {
    let dir = scratch_dir("menu-choose-nothing");
    let text = "schema = 1\n";
    fs::write(dir.join("config.toml"), text).unwrap();
    let cancelled = menu_with(&dir, running, |_| Box::new(|| None));
    let invalid = menu_with(&dir, running, |_| Box::new(|| Some("bad id".to_owned())));

    for menu in [cancelled, invalid] {
        let model = menu.model();
        menu.activate(option_at(
            &model,
            &["Prompt", "code", "Add application"],
            "Choose…",
        ));

        assert_eq!(fs::read_to_string(dir.join("config.toml")).unwrap(), text);
    }
    fs::remove_dir_all(&dir).unwrap();
}
