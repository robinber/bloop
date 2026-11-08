use std::cell::{Cell, OnceCell};
use std::fs;
use std::rc::{Rc, Weak};
use std::time::Duration;

use bloop_core::error::SecretError;
use bloop_core::ports::platform::{ActionId, MenuItem, MenuModel, MenuSource as _, SecretStore};
use bloop_providers::ChatConfig;
use secrecy::SecretString;

use super::{
    GHOSTTY, NOTES, READY, TERMINAL, WHATSAPP, facts, installed_name, options, prompt_marks, shown,
    submenu, titles,
};
use crate::files::tests::scratch_dir;
use crate::hub::Hub;
use crate::menu::{AppMenu, Lookups};
use crate::prompts;
use crate::settings::Paths;

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
