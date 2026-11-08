use std::fs;
use std::rc::Rc;
use std::sync::Arc;

use bloop_core::error::AppError;

use super::{
    LIMIT, ON, Secrets, chat_model, current, error, folder, name, no_correction, start, view,
};
use crate::driver::Snapshots;
use crate::hub::{Hub, HubSnapshots, Snapshot};
use crate::prompts;
use crate::settings::{self, Change, Paths};
use crate::worker::NoCorrection;

const GHOSTTY: &str = "com.mitchellh.ghostty";
const WHATSAPP: &str = "net.whatsapp.WhatsApp";
const NOTES: &str = "com.apple.Notes";
const TERMINAL: &str = "com.apple.Terminal";

/// Refreshes the hub for a dictation in `app`, and returns the snapshot and the name of its prompt.
fn routed(hub: &Hub, app: Option<&str>) -> (Rc<Snapshot>, Option<String>) {
    let snapshot = hub.refresh(app).expect("a free hub");
    let prompt = hub
        .inner
        .borrow()
        .state
        .prompt
        .as_ref()
        .map(|(name, _)| name.as_str().to_owned());
    (snapshot, prompt)
}

fn prompt_of(hub: &Hub, app: Option<&str>) -> Option<String> {
    routed(hub, app).1
}

#[test]
fn a_routed_app_uses_the_prompt_of_its_list() {
    let dir = folder("hub-route-app", ON);
    let hub = start(&dir);

    let (snapshot, prompt) = routed(&hub, Some(GHOSTTY));
    assert_eq!(prompt.as_deref(), Some("code"));
    assert_eq!(no_correction(&snapshot), None);
    assert_eq!(
        prompt_of(&hub, Some(WHATSAPP)).as_deref(),
        Some("natural-light")
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_unknown_app_and_no_app_use_the_selected_prompt() {
    let dir = folder(
        "hub-route-default",
        "schema = 1\n\n[refine]\nenabled = true\nprompt = \"code\"\n",
    );
    let hub = start(&dir);

    assert_eq!(
        prompt_of(&hub, Some(WHATSAPP)).as_deref(),
        Some("natural-light")
    );
    assert_eq!(prompt_of(&hub, Some(NOTES)).as_deref(), Some("code"));
    assert_eq!(prompt_of(&hub, None).as_deref(), Some("code"));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn with_the_correction_off_a_routed_app_reads_no_prompt_and_the_view_shows_no_error() {
    let text = "schema = 1\n\n[refine]\nenabled = false\nprompt = \"absent\"\n\n[refine.apps]\nmail = [\"com.apple.mail\"]\n";
    let dir = folder("hub-route-off", text);
    let hub = start(&dir);

    let (snapshot, prompt) = routed(&hub, Some("com.apple.mail"));

    assert_eq!(prompt, None);
    assert_eq!(no_correction(&snapshot), Some(NoCorrection::Off));
    assert_eq!(prompt_of(&hub, Some(GHOSTTY)), None);
    let view = view(&hub);
    assert_eq!(view.prompt, None);
    assert_eq!(view.routed, []);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn without_a_table_the_default_lists_route_and_an_application_outside_them_does_not() {
    let dir = folder("hub-route-defaults", &format!("{ON}prompt = \"dev\"\n"));
    let hub = Hub::start(
        Paths::new(&dir),
        &Secrets::with_keys(),
        LIMIT,
        prompts::default_lists(|id| id == GHOSTTY || id == WHATSAPP),
    );

    assert_eq!(prompt_of(&hub, Some(GHOSTTY)).as_deref(), Some("code"));
    assert_eq!(
        prompt_of(&hub, Some(WHATSAPP)).as_deref(),
        Some("natural-light")
    );
    assert_eq!(prompt_of(&hub, Some(TERMINAL)).as_deref(), Some("dev"));
    assert_eq!(prompt_of(&hub, Some(NOTES)).as_deref(), Some("dev"));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_table_is_the_whole_truth_and_the_default_lists_no_longer_apply() {
    let dir = folder(
        "hub-route-table",
        &format!("{ON}prompt = \"dev\"\n\n[refine.apps]\nnatural-light = [\"{GHOSTTY}\"]\n"),
    );
    let hub = start(&dir);

    assert_eq!(
        prompt_of(&hub, Some(GHOSTTY)).as_deref(),
        Some("natural-light")
    );
    assert_eq!(prompt_of(&hub, Some(TERMINAL)).as_deref(), Some("dev"));
    assert_eq!(prompt_of(&hub, Some(WHATSAPP)).as_deref(), Some("dev"));
    let lists = view(&hub).lists;
    assert_eq!(lists.keys().collect::<Vec<_>>(), [&name("natural-light")]);

    fs::write(
        dir.join("config.toml"),
        format!("{ON}prompt = \"dev\"\n\n[refine.apps]\n"),
    )
    .unwrap();

    assert_eq!(prompt_of(&hub, Some(GHOSTTY)).as_deref(), Some("dev"));
    assert_eq!(prompt_of(&hub, Some(WHATSAPP)).as_deref(), Some("dev"));
    assert!(view(&hub).lists.is_empty());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_same_app_in_two_keys_gives_invalid_settings() {
    let dir = folder(
        "hub-route-twice",
        &format!("{ON}\n[refine.apps]\ncode = [\"{NOTES}\"]\nnatural-light = [\"{NOTES}\"]\n"),
    );
    let hub = start(&dir);

    let (snapshot, prompt) = routed(&hub, Some(NOTES));

    assert_eq!(error(&snapshot), Some(AppError::InvalidSettings));
    assert_eq!(prompt, None);
    assert_eq!(
        view(&hub).settings.err(),
        Some(settings::Invalid::Parse { line: Some(6) })
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_routed_missing_prompt_fails_its_app_only_and_the_view_names_it() {
    let dir = folder(
        "hub-route-missing",
        &format!("{ON}\n[refine.apps]\nmail = [\"com.apple.mail\"]\n"),
    );
    let hub = start(&dir);

    let (mail, prompt) = routed(&hub, Some("com.apple.mail"));
    assert_eq!(prompt.as_deref(), Some("mail"));
    assert_eq!(no_correction(&mail), Some(NoCorrection::InvalidPrompt));
    let (notes, prompt) = routed(&hub, Some(NOTES));
    assert_eq!(prompt.as_deref(), Some("natural-light"));
    assert_eq!(no_correction(&notes), None);
    assert_eq!(no_correction(&current(&hub)), None);

    let view = view(&hub);
    assert_eq!(view.prompt, None);
    assert_eq!(view.routed, [(name("mail"), prompts::PromptError::Missing)]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_view_names_each_routed_prompt_error_once_and_never_the_selected_prompt() {
    let dir = folder(
        "hub-route-errors",
        &format!(
            "{ON}prompt = \"mail\"\n\n[refine.apps]\nmail = [\"com.apple.mail\"]\nnotes = [\"{NOTES}\"]\nunused = []\n"
        ),
    );
    fs::create_dir(dir.join("prompts")).unwrap();
    fs::write(dir.join("prompts").join("notes.md"), " \n").unwrap();
    let hub = start(&dir);

    let view = view(&hub);

    assert_eq!(
        view.prompt,
        Some((name("mail"), prompts::PromptError::Missing))
    );
    assert_eq!(view.routed, [(name("notes"), prompts::PromptError::Empty)]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn alternating_apps_reuse_the_chat_model() {
    let dir = folder("hub-route-chat", ON);
    let hub = start(&dir);
    let model = chat_model(&hub);

    let (code, _) = routed(&hub, Some(GHOSTTY));
    let (light, _) = routed(&hub, Some(WHATSAPP));
    let (again, _) = routed(&hub, Some(GHOSTTY));

    assert!(!Rc::ptr_eq(&code, &light));
    assert!(!Rc::ptr_eq(&light, &again));
    assert!(Rc::ptr_eq(&again, &routed(&hub, Some(GHOSTTY)).0));
    assert!(Arc::ptr_eq(&model, &chat_model(&hub)));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_driver_snapshot_uses_the_frontmost_app_and_a_view_does_not_change_it() {
    let dir = folder("hub-route-snapshots", ON);
    let hub = Rc::new(start(&dir));
    let mut snapshots = HubSnapshots::new(Rc::clone(&hub), || Some(GHOSTTY.to_owned()));

    let first = snapshots.snapshot();
    assert_eq!(prompt_name(&hub).as_deref(), Some("code"));
    let _ = view(&hub);
    assert_eq!(prompt_name(&hub).as_deref(), Some("natural-light"));
    let next = snapshots.snapshot();

    assert_eq!(prompt_name(&hub).as_deref(), Some("code"));
    assert_eq!(no_correction(&first), None);
    assert_eq!(no_correction(&next), None);
    fs::remove_dir_all(&dir).unwrap();
}

fn prompt_name(hub: &Hub) -> Option<String> {
    hub.inner
        .borrow()
        .state
        .prompt
        .as_ref()
        .map(|(name, _)| name.as_str().to_owned())
}

#[test]
fn set_moves_an_app_and_the_next_dictation_uses_its_new_prompt() {
    let dir = folder("hub-route-set", ON);
    let hub = start(&dir);
    assert_eq!(prompt_of(&hub, Some(GHOSTTY)).as_deref(), Some("code"));
    let ghostty = settings::AppId::try_from(GHOSTTY.to_owned()).unwrap();

    hub.set(&Change::AddApp {
        prompt: name("natural-light"),
        app: ghostty.clone(),
    });
    assert_eq!(
        prompt_of(&hub, Some(GHOSTTY)).as_deref(),
        Some("natural-light")
    );
    let mut code = prompts::default_lists(|_| true)[&name("code")].clone();
    code.retain(|app| *app != ghostty);
    assert_eq!(view(&hub).lists[&name("code")], code);

    hub.set(&Change::RemoveApp {
        prompt: name("natural-light"),
        app: ghostty.clone(),
    });
    let lists = view(&hub).lists;
    assert!(
        lists.values().all(|list| !list.contains(&ghostty)),
        "{lists:?}"
    );
    assert_eq!(
        prompt_of(&hub, Some(WHATSAPP)).as_deref(),
        Some("natural-light")
    );
    assert_eq!(prompt_of(&hub, Some(TERMINAL)).as_deref(), Some("code"));
    fs::remove_dir_all(&dir).unwrap();
}
