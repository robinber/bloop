use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use bloop_core::domain::Generation;
use bloop_core::error::{AppError, ProviderError, SecretError};
use bloop_core::ports::ai::ChatModel;
use bloop_core::ports::platform::SecretStore;
use bloop_core::session::{Command, Core, Event};
use secrecy::SecretString;

use super::{Hub, HubSnapshots, Snapshot, View, refuses_key};
use crate::driver::Snapshots;
use crate::files::tests::scratch_dir;
use crate::settings::{self, Change, Paths, PromptName};
use crate::worker::{NoCorrection, Speech};
use crate::{dictionary, prompts};

const LIMIT: Duration = Duration::from_secs(8);
const HOLD: Duration = Duration::from_millis(220);
const OFF: &str = "schema = 1\n";
const ON: &str = "schema = 1\n\n[refine]\nenabled = true\n";

/// A key store with a dummy key for each account, or without keys. It records the accounts that Bloop reads.
struct Secrets {
    key: bool,
    reads: RefCell<Vec<String>>,
}

impl Secrets {
    fn with_keys() -> Self {
        Self {
            key: true,
            reads: RefCell::default(),
        }
    }

    fn without_keys() -> Self {
        Self {
            key: false,
            reads: RefCell::default(),
        }
    }
}

impl SecretStore for Secrets {
    fn get(&self, account: &str) -> Result<SecretString, SecretError> {
        self.reads.borrow_mut().push(account.to_owned());
        if self.key {
            Ok(SecretString::from("test-key"))
        } else {
            Err(SecretError::NotFound)
        }
    }
}

/// Writes `config` to `config.toml` in a new folder, and returns the folder.
fn folder(test: &str, config: &str) -> std::path::PathBuf {
    let dir = scratch_dir(test);
    fs::write(dir.join("config.toml"), config).unwrap();
    dir
}

fn name(value: &str) -> PromptName {
    PromptName::try_from(value.to_owned()).unwrap()
}

fn start(dir: &Path) -> Hub {
    Hub::start(Paths::new(dir), &Secrets::with_keys(), LIMIT)
}

fn current(hub: &Hub) -> Rc<Snapshot> {
    hub.refresh(None).expect("a free hub")
}

fn speech(snapshot: &Snapshot) -> &Speech {
    match &snapshot.speech {
        Ok(speech) => speech,
        Err(error) => panic!("{error:?}"),
    }
}

fn error(snapshot: &Snapshot) -> Option<AppError> {
    snapshot.speech.as_ref().err().copied()
}

fn no_correction(snapshot: &Snapshot) -> Option<NoCorrection> {
    speech(snapshot).correction.as_ref().err().copied()
}

fn chat_model(hub: &Hub) -> Arc<dyn ChatModel> {
    let inner = hub.inner.borrow();
    let (_, model) = inner.chat.as_ref().expect("a chat model");
    Arc::clone(model)
}

#[test]
fn an_unchanged_text_builds_no_new_snapshot() {
    let dir = folder("hub-unchanged", ON);
    fs::write(
        dir.join("dictionary.toml"),
        "[[entry]]\nwritten = \"objc2\"\n",
    )
    .unwrap();
    let hub = start(&dir);
    let first = Rc::clone(&hub.inner.borrow().snapshot);

    assert!(Rc::ptr_eq(&first, &current(&hub)));
    assert!(Rc::ptr_eq(&first, &current(&hub)));
    assert_eq!(speech(&first).vocabulary, ["objc2"]);
    assert_eq!(no_correction(&first), None);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_changed_config_builds_a_new_snapshot() {
    let dir = folder("hub-config", OFF);
    let hub = start(&dir);
    let first = current(&hub);
    assert_eq!(first.hold, HOLD);

    fs::write(
        dir.join("config.toml"),
        "schema = 1\n\n[dictation]\nhold_ms = 400\nlanguage = \"en\"\n",
    )
    .unwrap();
    let next = current(&hub);

    assert!(!Rc::ptr_eq(&first, &next));
    assert_eq!(next.hold, Duration::from_millis(400));
    assert_eq!(speech(&next).language, "en");
    assert!(Rc::ptr_eq(&next, &current(&hub)));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_changed_dictionary_builds_a_new_snapshot() {
    let dir = folder("hub-dictionary", OFF);
    let hub = start(&dir);
    let first = current(&hub);
    assert_eq!(speech(&first).vocabulary, [] as [String; 0]);

    fs::write(
        dir.join("dictionary.toml"),
        "[[entry]]\nwritten = \"objc2\"\n",
    )
    .unwrap();
    let next = current(&hub);

    assert!(!Rc::ptr_eq(&first, &next));
    assert_eq!(speech(&next).vocabulary, ["objc2"]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_changed_prompt_file_builds_a_new_snapshot_with_the_same_chat_model() {
    let dir = folder(
        "hub-prompt",
        "schema = 1\n\n[refine]\nenabled = true\nprompt = \"dev\"\n",
    );
    fs::create_dir(dir.join("prompts")).unwrap();
    fs::write(dir.join("prompts").join("dev.md"), "Corrige.\n").unwrap();
    let hub = start(&dir);
    let first = current(&hub);
    let model = chat_model(&hub);
    assert_eq!(no_correction(&first), None);

    fs::write(dir.join("prompts").join("dev.md"), "Corrige le texte.\n").unwrap();
    let next = current(&hub);

    assert!(!Rc::ptr_eq(&first, &next));
    assert_eq!(no_correction(&next), None);
    assert!(Arc::ptr_eq(&model, &chat_model(&hub)));
    let inner = hub.inner.borrow();
    let Some((_, Ok(prompt))) = &inner.state.prompt else {
        panic!("{:?}", inner.state.prompt);
    };
    assert_eq!(prompt.text, "Corrige le texte.\n");
    drop(inner);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_changed_chat_table_builds_a_new_chat_model() {
    let dir = folder("hub-chat", ON);
    let hub = start(&dir);
    let model = chat_model(&hub);

    fs::write(
        dir.join("config.toml"),
        format!("{ON}\n[chat]\nprovider = \"mistral\"\nmodel = \"mistral-small-latest\"\n"),
    )
    .unwrap();
    let next = current(&hub);

    assert_eq!(no_correction(&next), None);
    assert!(!Arc::ptr_eq(&model, &chat_model(&hub)));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_changed_model_list_builds_a_new_chat_model() {
    let chat = "[chat]\nprovider = \"mistral\"\nmodel = \"mistral-medium-latest\"\n";
    let dir = folder("hub-chat-models", &format!("{ON}\n{chat}"));
    let hub = start(&dir);
    let model = chat_model(&hub);

    fs::write(
        dir.join("config.toml"),
        format!("{ON}\n{chat}models = [\"mistral-medium-latest\"]\n"),
    )
    .unwrap();
    let next = current(&hub);

    assert_eq!(no_correction(&next), None);
    assert!(!Arc::ptr_eq(&model, &chat_model(&hub)));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_config_change_that_keeps_the_chat_table_keeps_the_chat_model() {
    let dir = folder("hub-chat-kept", ON);
    let hub = start(&dir);
    let model = chat_model(&hub);

    fs::write(
        dir.join("config.toml"),
        format!("{ON}\n[dictation]\nhold_ms = 400\n"),
    )
    .unwrap();
    let next = current(&hub);

    assert_eq!(next.hold, Duration::from_millis(400));
    assert_eq!(no_correction(&next), None);
    assert!(Arc::ptr_eq(&model, &chat_model(&hub)));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_invalid_config_gives_invalid_settings_and_the_default_hold() {
    let dir = folder(
        "hub-invalid-config",
        "schema = 1\n\n[dictation]\nhold_ms = 5000\n",
    );
    let hub = start(&dir);

    let first = current(&hub);
    assert_eq!(error(&first), Some(AppError::InvalidSettings));
    assert_eq!(first.hold, HOLD);
    fs::write(
        dir.join("config.toml"),
        "schema = 1\n\n[dictation]\nhold_ms = 400\nlanguage = 1\n",
    )
    .unwrap();
    let next = current(&hub);
    assert_eq!(error(&next), Some(AppError::InvalidSettings));
    assert_eq!(next.hold, HOLD);

    fs::write(
        dir.join("config.toml"),
        "schema = 1\n\n[dictation]\nhold_ms = 400\n",
    )
    .unwrap();
    let fixed = current(&hub);
    assert_eq!(error(&fixed), None);
    assert_eq!(fixed.hold, Duration::from_millis(400));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_config_over_its_limit_is_invalid() {
    let limit = usize::try_from(settings::LIMIT).unwrap();
    let dir = folder("hub-large-config", &format!("{OFF}{}", "#".repeat(limit)));
    let hub = start(&dir);

    let snapshot = current(&hub);

    assert_eq!(error(&snapshot), Some(AppError::InvalidSettings));
    assert_eq!(snapshot.hold, HOLD);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_unreadable_config_gives_invalid_settings() {
    let dir = scratch_dir("hub-unreadable-config");
    fs::create_dir(dir.join("config.toml")).unwrap();
    let hub = start(&dir);

    assert_eq!(error(&current(&hub)), Some(AppError::InvalidSettings));
    assert!(matches!(
        view(&hub).settings.err(),
        Some(settings::Invalid::Unreadable(_))
    ));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_invalid_dictionary_gives_invalid_settings_with_the_hold_of_the_file() {
    let dir = folder(
        "hub-invalid-dictionary",
        "schema = 1\n\n[dictation]\nhold_ms = 400\n",
    );
    fs::write(dir.join("dictionary.toml"), "[[entry]]\nwritten = \"\"\n").unwrap();
    let hub = start(&dir);

    let snapshot = current(&hub);

    assert_eq!(error(&snapshot), Some(AppError::InvalidSettings));
    assert_eq!(snapshot.hold, Duration::from_millis(400));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_unreadable_or_too_large_dictionary_gives_invalid_settings() {
    let dir = folder("hub-unreadable-dictionary", OFF);
    fs::create_dir(dir.join("dictionary.toml")).unwrap();
    let hub = start(&dir);
    assert_eq!(error(&current(&hub)), Some(AppError::InvalidSettings));

    fs::remove_dir(dir.join("dictionary.toml")).unwrap();
    assert_eq!(error(&current(&hub)), None);
    let limit = usize::try_from(dictionary::LIMIT).unwrap();
    fs::write(dir.join("dictionary.toml"), "#".repeat(limit + 1)).unwrap();

    assert_eq!(error(&current(&hub)), Some(AppError::InvalidSettings));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn with_the_correction_off_the_prompt_is_not_read() {
    let dir = folder(
        "hub-off",
        "schema = 1\n\n[refine]\nenabled = false\nprompt = \"absent\"\n",
    );
    let hub = start(&dir);

    let off = current(&hub);
    assert_eq!(no_correction(&off), Some(NoCorrection::Off));
    assert!(hub.inner.borrow().state.prompt.is_none());
    assert!(hub.inner.borrow().chat.is_none());

    fs::write(
        dir.join("config.toml"),
        "schema = 1\n\n[refine]\nenabled = true\nprompt = \"absent\"\n",
    )
    .unwrap();
    assert_eq!(
        no_correction(&current(&hub)),
        Some(NoCorrection::InvalidPrompt)
    );
    assert_eq!(
        hub.inner.borrow().state.prompt,
        Some((name("absent"), Err(prompts::PromptError::Missing)))
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn set_correction_writes_the_key_and_the_next_snapshot_has_the_correction() {
    let text = "# mine\nschema = 1\n\n[refine]\nenabled = false   # the correction\n";
    let dir = folder("hub-set", text);
    let hub = start(&dir);
    let first = current(&hub);
    assert_eq!(no_correction(&first), Some(NoCorrection::Off));
    assert!(!hub.correction_on());

    hub.set(&Change::Correction(true));

    assert!(hub.correction_on());
    assert!(!Rc::ptr_eq(&first, &hub.inner.borrow().snapshot));
    assert_eq!(
        fs::read_to_string(dir.join("config.toml")).unwrap(),
        "# mine\nschema = 1\n\n[refine]\nenabled = true    # the correction\n"
    );
    assert_eq!(no_correction(&current(&hub)), None);
    hub.set(&Change::Correction(false));
    assert_eq!(fs::read_to_string(dir.join("config.toml")).unwrap(), text);
    assert_eq!(no_correction(&current(&hub)), Some(NoCorrection::Off));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn set_on_an_invalid_file_changes_nothing() {
    let text = "schema = 1\n\n[refine]\nenabled = \"yes\"\n";
    let dir = folder("hub-set-invalid", text);
    let hub = start(&dir);
    let first = current(&hub);

    hub.set(&Change::Correction(true));

    assert_eq!(fs::read_to_string(dir.join("config.toml")).unwrap(), text);
    let next = current(&hub);
    assert!(Rc::ptr_eq(&first, &next));
    assert_eq!(error(&next), Some(AppError::InvalidSettings));
    assert!(!hub.correction_on());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn without_a_key_at_start_each_dictation_fails_with_unauthorized() {
    let dir = folder("hub-no-key", ON);
    let secrets = Secrets::without_keys();
    let hub = Hub::start(Paths::new(&dir), &secrets, LIMIT);

    assert_eq!(
        error(&current(&hub)),
        Some(AppError::Provider(ProviderError::Unauthorized))
    );
    assert_eq!(hub.key_error(), Some(SecretError::NotFound));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_keys_are_read_one_time_at_start() {
    let dir = folder("hub-keys", ON);
    let secrets = Secrets::with_keys();
    let hub = Hub::start(Paths::new(&dir), &secrets, LIMIT);
    assert_eq!(hub.key_error(), None);

    fs::write(dir.join("config.toml"), OFF).unwrap();
    let snapshot = current(&hub);

    assert_eq!(error(&snapshot), None);
    assert_eq!(*secrets.reads.borrow(), ["mistral"]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_driver_gets_the_last_snapshot_when_the_hub_is_in_use() {
    let dir = folder("hub-in-use", OFF);
    let hub = Rc::new(start(&dir));
    let mut snapshots = HubSnapshots::new(Rc::clone(&hub), || None);
    let first = snapshots.snapshot();
    assert!(Rc::ptr_eq(&first, &current(&hub)));
    fs::write(dir.join("config.toml"), ON).unwrap();

    let borrow = hub.inner.borrow_mut();
    let during = snapshots.snapshot();
    assert!(!hub.correction_on());
    drop(borrow);

    assert!(Rc::ptr_eq(&first, &during));
    let after = snapshots.snapshot();
    assert!(!Rc::ptr_eq(&first, &after));
    assert_eq!(no_correction(&after), None);
    fs::remove_dir_all(&dir).unwrap();
}

fn generation() -> Generation {
    let commands = Core::new().handle(Event::DictationKeyDown);
    let [Command::StartHoldTimer(generation)] = commands.as_slice() else {
        panic!("{commands:?}");
    };
    *generation
}

fn view(hub: &Hub) -> View {
    hub.view().expect("a free hub")
}

#[test]
fn the_view_gives_the_correction_state_of_the_file() {
    let dir = folder("hub-view", OFF);
    let hub = start(&dir);
    let first = view(&hub);
    assert!(!first.settings.as_ref().unwrap().correction);
    assert_eq!(first.prompt, None);
    assert_eq!(first.dictionary, None);
    assert!(!first.key_refused);

    fs::write(dir.join("config.toml"), ON).unwrap();

    assert!(view(&hub).settings.unwrap().correction);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_view_gives_the_line_of_each_file_error() {
    let dir = folder(
        "hub-view-errors",
        "schema = 1\n\n[dictation]\nhold_ms = 5000\n",
    );
    fs::write(
        dir.join("dictionary.toml"),
        "[[entry]]\nwritten = \"objc2\"\n\n[[entry]]\nwritten = \" \"\n",
    )
    .unwrap();
    let hub = start(&dir);

    let view = view(&hub);

    assert_eq!(
        view.settings.err(),
        Some(settings::Invalid::Parse { line: Some(4) })
    );
    assert_eq!(view.prompt, None);
    assert_eq!(
        view.dictionary,
        Some(dictionary::Invalid::EmptyWritten { line: Some(5) })
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_view_gives_the_prompt_error_only_with_the_correction_on() {
    let dir = folder(
        "hub-view-prompt",
        "schema = 1\n\n[refine]\nenabled = false\nprompt = \"absent\"\n",
    );
    let hub = start(&dir);
    assert_eq!(view(&hub).prompt, None);

    fs::write(
        dir.join("config.toml"),
        "schema = 1\n\n[refine]\nenabled = true\nprompt = \"absent\"\n",
    )
    .unwrap();
    let (name, error) = view(&hub).prompt.unwrap();

    assert_eq!(name.as_str(), "absent");
    assert_eq!(error, prompts::PromptError::Missing);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_view_of_a_hub_in_use_is_none() {
    let dir = folder("hub-view-in-use", OFF);
    let hub = start(&dir);

    let borrow = hub.inner.borrow_mut();
    assert!(hub.view().is_none());
    drop(borrow);

    assert!(hub.view().is_some());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn only_an_unauthorized_failure_refuses_the_key() {
    let generation = generation();
    let unauthorized = AppError::Provider(ProviderError::Unauthorized);

    assert!(refuses_key(&Event::CaptureFailed(generation, unauthorized)));
    assert!(refuses_key(&Event::WorkFailed(generation, unauthorized)));
    for error in [
        AppError::Provider(ProviderError::Timeout),
        AppError::Provider(ProviderError::Http(500)),
        AppError::InvalidSettings,
        AppError::Internal,
    ] {
        assert!(!refuses_key(&Event::CaptureFailed(generation, error)));
        assert!(!refuses_key(&Event::WorkFailed(generation, error)));
    }
    for event in [
        Event::DictationKeyDown,
        Event::NothingHeard(generation),
        Event::CopyLastResult,
    ] {
        assert!(!refuses_key(&event));
    }
}

#[test]
fn a_refused_key_stays_until_the_next_start() {
    let dir = folder("hub-refused", OFF);
    let hub = start(&dir);
    let generation = generation();
    hub.note(&Event::WorkFailed(
        generation,
        AppError::Provider(ProviderError::Timeout),
    ));
    assert!(!view(&hub).key_refused);

    hub.note(&Event::CaptureFailed(
        generation,
        AppError::Provider(ProviderError::Unauthorized),
    ));
    hub.note(&Event::DictationKeyUp);
    fs::write(dir.join("config.toml"), ON).unwrap();

    assert!(view(&hub).key_refused);
    assert!(!view(&start(&dir)).key_refused);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn without_a_key_at_start_an_unauthorized_failure_is_not_a_refused_key() {
    let dir = folder("hub-refused-no-key", OFF);
    let hub = Hub::start(Paths::new(&dir), &Secrets::without_keys(), LIMIT);

    hub.note(&Event::CaptureFailed(
        generation(),
        AppError::Provider(ProviderError::Unauthorized),
    ));

    assert!(!view(&hub).key_refused);
    assert_eq!(hub.key_error(), Some(SecretError::NotFound));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_view_gives_the_values_of_the_lists_and_the_prompt_names() {
    let dir = folder(
        "hub-view-lists",
        "schema = 1\n\n[dictation]\nhold_ms = 300\n\n[chat]\nprovider = \"mistral\"\nmodel = \"b\"\nmodels = [\"a\", \"b\"]\n\n[refine]\nprompt = \"dev\"\n",
    );
    fs::create_dir(dir.join("prompts")).unwrap();
    fs::write(dir.join("prompts").join("dev.md"), "Corrige.\n").unwrap();
    let hub = start(&dir);

    let view = view(&hub);

    let summary = view.settings.unwrap();
    assert!(!summary.correction);
    assert_eq!(summary.prompt.as_str(), "dev");
    assert_eq!(summary.model, "b");
    assert_eq!(summary.models, ["a", "b"]);
    assert_eq!(summary.hold.get(), 300);
    let names: Vec<&str> = view.prompts.iter().map(PromptName::as_str).collect();
    assert_eq!(names, ["code", "dev", "natural-light"]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn set_writes_each_change_and_the_next_snapshot_uses_it() {
    let dir = folder("hub-set-lists", ON);
    let hub = start(&dir);
    let model = chat_model(&hub);

    hub.set(&Change::Hold(settings::HoldMs::try_from(400).unwrap()));
    hub.set(&Change::Model("mistral-small-latest".to_owned()));
    hub.set(&Change::Prompt(
        PromptName::try_from("absent".to_owned()).unwrap(),
    ));

    let snapshot = current(&hub);
    assert_eq!(snapshot.hold, Duration::from_millis(400));
    assert_eq!(no_correction(&snapshot), Some(NoCorrection::InvalidPrompt));
    let summary = view(&hub).settings.unwrap();
    assert_eq!(summary.model, "mistral-small-latest");
    assert_eq!(summary.prompt.as_str(), "absent");
    assert!(!Arc::ptr_eq(&model, &chat_model(&hub)));
    fs::remove_dir_all(&dir).unwrap();
}

const GHOSTTY: &str = "com.mitchellh.ghostty";
const WHATSAPP: &str = "net.whatsapp.WhatsApp";
const NOTES: &str = "com.apple.Notes";

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
fn a_key_replaces_the_built_in_list_and_an_empty_key_disables_it() {
    let dir = folder(
        "hub-route-replace",
        &format!("{ON}\n[refine.apps]\ncode = [\"{NOTES}\"]\n"),
    );
    let hub = start(&dir);
    assert_eq!(prompt_of(&hub, Some(NOTES)).as_deref(), Some("code"));
    assert_eq!(
        prompt_of(&hub, Some(GHOSTTY)).as_deref(),
        Some("natural-light")
    );

    fs::write(
        dir.join("config.toml"),
        format!("{ON}prompt = \"code\"\n\n[refine.apps]\nnatural-light = []\n"),
    )
    .unwrap();

    assert_eq!(prompt_of(&hub, Some(WHATSAPP)).as_deref(), Some("code"));
    assert_eq!(prompt_of(&hub, Some(GHOSTTY)).as_deref(), Some("code"));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_app_of_a_key_wins_over_its_built_in_list() {
    let dir = folder(
        "hub-route-wins",
        &format!("{ON}prompt = \"code\"\n\n[refine.apps]\nnatural-light = [\"{GHOSTTY}\"]\n"),
    );
    let hub = start(&dir);

    assert_eq!(
        prompt_of(&hub, Some(GHOSTTY)).as_deref(),
        Some("natural-light")
    );
    assert_eq!(
        prompt_of(&hub, Some("com.apple.Terminal")).as_deref(),
        Some("code")
    );
    assert_eq!(prompt_of(&hub, Some(WHATSAPP)).as_deref(), Some("code"));
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
    assert!(!view(&hub).lists[&name("code")].contains(&ghostty));

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
    assert_eq!(
        prompt_of(&hub, Some("com.apple.Terminal")).as_deref(),
        Some("code")
    );
    fs::remove_dir_all(&dir).unwrap();
}
