use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use bloop_core::error::{AppError, ProviderError, SecretError};
use bloop_core::ports::ai::ChatModel;
use bloop_core::ports::platform::SecretStore;
use secrecy::SecretString;

use super::{Change, Hub, HubSnapshots, Snapshot};
use crate::driver::Snapshots;
use crate::files::tests::scratch_dir;
use crate::settings::{self, Paths};
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

fn start(dir: &Path) -> Hub {
    Hub::start(Paths::new(dir), &Secrets::with_keys(), LIMIT)
}

fn current(hub: &Hub) -> Rc<Snapshot> {
    hub.refresh().expect("a free hub")
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
    let Some(Ok(prompt)) = &inner.state.prompt else {
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
        Some(Err(prompts::PromptError::Missing))
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

    hub.set(Change::Correction(true));

    assert!(hub.correction_on());
    assert!(!Rc::ptr_eq(&first, &hub.inner.borrow().snapshot));
    assert_eq!(
        fs::read_to_string(dir.join("config.toml")).unwrap(),
        "# mine\nschema = 1\n\n[refine]\nenabled = true    # the correction\n"
    );
    assert_eq!(no_correction(&current(&hub)), None);
    hub.set(Change::Correction(false));
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

    hub.set(Change::Correction(true));

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
    let mut snapshots = HubSnapshots::new(Rc::clone(&hub));
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
