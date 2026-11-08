//! The content of the status menu: the status lines, "Correct text", the lists, and the menu actions.
//!
//! Each time the menu opens, `AppMenu` refreshes the settings hub, looks up the frontmost application and the names
//! of the applications outside the hub borrow, and builds the model with the pure function `build`.
//! The status lines give the file and the line of an error, never the file content.
//! `AppKit` calls `AppMenu` from Objective-C, thus nothing here panics. A failed borrow logs an error.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use bloop_core::error::{Permission, SecretError};
use bloop_core::ports::platform::{ActionId, EventSink as _, MenuItem, MenuModel, MenuSource};
use bloop_core::session::Event;
use bloop_macos::events::MainSink;
use bloop_macos::{permissions, workspace};

use crate::dictionary;
use crate::hub::{Hub, View};
use crate::prompts::{self, PromptError};
use crate::settings::{self, AppId, Change, HoldMs, Paths, PromptName};

/// The options of the "Hold delay" list, in milliseconds.
const HOLDS: [u64; 4] = [150, 220, 300, 400];

/// The status at start that the settings files do not change.
pub(crate) struct Facts {
    /// The permissions that are missing at start. macOS can keep the old result until Bloop starts again.
    pub(crate) missing: Vec<Permission>,
    /// `true` when the event tap operates.
    pub(crate) tap: bool,
    /// The error of the transcription API key at start.
    pub(crate) key: Option<SecretError>,
}

/// The action of a menu item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// Opens the System Settings pane of a missing permission.
    OpenPermission(Permission),
    /// Changes the state of "Correct text".
    ToggleCorrection,
    /// Selects the correction prompt.
    Prompt(PromptName),
    /// Selects the correction model.
    Model(String),
    /// Selects the hold time of the dictation key.
    Hold(HoldMs),
    /// Adds an application to the list of a prompt, and removes it from the other lists.
    AddApp {
        prompt: PromptName,
        app: AppId,
    },
    /// Removes an application from the list of a prompt.
    RemoveApp {
        prompt: PromptName,
        app: AppId,
    },
    CopyLastResult,
    OpenConfig,
    OpenPrompts,
    OpenDictionary,
}

/// The application lookups of the status menu: the `NSWorkspace` functions, or fakes in the tests.
pub(crate) struct Lookups {
    /// Returns the bundle identifier of the frontmost application.
    pub(crate) frontmost: fn() -> Option<String>,
    /// Returns the name of the installed application with a bundle identifier.
    pub(crate) name: fn(&str) -> Option<String>,
}

/// The applications that the status menu names. `AppMenu::model` looks them up outside the hub borrow.
#[derive(Default)]
pub(crate) struct AppNames {
    /// The frontmost application, when its bundle identifier is valid.
    pub(crate) frontmost: Option<AppId>,
    /// The names that macOS gives to the installed applications.
    pub(crate) names: BTreeMap<AppId, String>,
}

impl AppNames {
    /// Returns the name of `app`: the name that macOS gives, else its name in a built-in list, else its identifier.
    fn name(&self, app: &AppId) -> String {
        match self.names.get(app) {
            Some(name) => name.clone(),
            None => prompts::built_in_name(app.as_str())
                .unwrap_or(app.as_str())
                .to_owned(),
        }
    }
}

/// The menu source of the status menu.
pub(crate) struct AppMenu {
    hub: Rc<Hub>,
    facts: Facts,
    paths: Paths,
    lookups: Lookups,
    /// The last model and its actions. `ActionId(n)` is the index `n` of the actions.
    last: RefCell<(MenuModel, Vec<Action>)>,
}

impl AppMenu {
    pub(crate) fn new(hub: Rc<Hub>, facts: Facts, paths: Paths, lookups: Lookups) -> Self {
        Self {
            hub,
            facts,
            paths,
            lookups,
            last: RefCell::new((MenuModel { items: Vec::new() }, Vec::new())),
        }
    }

    /// Looks up the frontmost application, and the name of each application of the lists and of the frontmost one.
    fn apps(&self, view: &View) -> AppNames {
        let frontmost = (self.lookups.frontmost)().and_then(|id| AppId::try_from(id).ok());
        let mut names = BTreeMap::new();
        for app in view.lists.values().flatten().chain(&frontmost) {
            if !names.contains_key(app)
                && let Some(name) = (self.lookups.name)(app.as_str())
            {
                names.insert(app.clone(), name);
            }
        }
        AppNames { frontmost, names }
    }

    /// Returns the action of `id` in the last model.
    fn action(&self, id: ActionId) -> Option<Action> {
        let Ok(last) = self.last.try_borrow() else {
            tracing::error!("the menu is already in use, the click has no effect");
            return None;
        };
        let action = last.1.get(id.0).cloned();
        if action.is_none() {
            tracing::error!(?id, "the menu action is unknown, the click has no effect");
        }
        action
    }
}

impl MenuSource for AppMenu {
    /// Refreshes the hub and returns the new model. When the hub is in use, it returns the last model.
    fn model(&self) -> MenuModel {
        let view = self.hub.view().map(|view| {
            let apps = self.apps(&view);
            (view, apps)
        });
        let Ok(mut last) = self.last.try_borrow_mut() else {
            tracing::error!("the menu is already in use, the menu shows no status");
            return MenuModel { items: Vec::new() };
        };
        if let Some((view, apps)) = view {
            *last = build(&self.facts, &view, &apps);
        }
        last.0.clone()
    }

    fn activate(&self, id: ActionId) {
        let Some(action) = self.action(id) else {
            return;
        };
        match action {
            Action::OpenPermission(permission) => permissions::open_settings(permission),
            Action::ToggleCorrection => {
                let on = !self.hub.correction_on();
                tracing::info!(on, "the correction toggle changed");
                self.hub.set(&Change::Correction(on));
            }
            Action::Prompt(name) => {
                tracing::info!(prompt = name.as_str(), "the menu selected a prompt");
                self.hub.set(&Change::Prompt(name));
            }
            Action::Model(model) => {
                tracing::info!(model, "the menu selected a correction model");
                self.hub.set(&Change::Model(model));
            }
            Action::Hold(hold) => {
                tracing::info!(hold_ms = hold.get(), "the menu selected a hold time");
                self.hub.set(&Change::Hold(hold));
            }
            Action::AddApp { prompt, app } => {
                tracing::info!(
                    prompt = prompt.as_str(),
                    app = app.as_str(),
                    "the menu added an application to a prompt"
                );
                self.hub.set(&Change::AddApp { prompt, app });
            }
            Action::RemoveApp { prompt, app } => {
                tracing::info!(
                    prompt = prompt.as_str(),
                    app = app.as_str(),
                    "the menu removed an application from a prompt"
                );
                self.hub.set(&Change::RemoveApp { prompt, app });
            }
            Action::CopyLastResult => MainSink.send(Event::CopyLastResult),
            Action::OpenConfig => workspace::open_file(&self.paths.config),
            Action::OpenPrompts => workspace::open_folder(&self.paths.prompts),
            Action::OpenDictionary => workspace::open_file(&self.paths.dictionary),
        }
    }
}

/// Builds the menu model and its actions from the status at start, the state of the settings files,
/// and the applications that the menu names.
pub(crate) fn build(facts: &Facts, view: &View, apps: &AppNames) -> (MenuModel, Vec<Action>) {
    let mut menu = Builder::default();
    status(&mut menu, facts, view);
    menu.items.push(MenuItem::Separator);
    if let Ok(summary) = &view.settings {
        menu.toggle("Correct text", Action::ToggleCorrection, summary.correction);
        menu.choice(
            "Prompt",
            &view.prompts,
            &summary.prompt,
            |name| name.as_str().to_owned(),
            Action::Prompt,
        );
        by_app(&mut menu, view, apps);
        menu.choice(
            "Model",
            &summary.models,
            &summary.model,
            Clone::clone,
            Action::Model,
        );
        let holds: Vec<HoldMs> = HOLDS
            .into_iter()
            .filter_map(|ms| HoldMs::try_from(ms).ok())
            .collect();
        menu.choice(
            "Hold delay",
            &holds,
            &summary.hold,
            |hold| format!("{} ms", hold.get()),
            Action::Hold,
        );
        menu.items.push(MenuItem::Separator);
    }
    menu.action("Copy last result".to_owned(), Action::CopyLastResult);
    menu.action("Open config.toml…".to_owned(), Action::OpenConfig);
    menu.action("Open prompts folder…".to_owned(), Action::OpenPrompts);
    menu.action("Open dictionary…".to_owned(), Action::OpenDictionary);
    (MenuModel { items: menu.items }, menu.actions)
}

/// Adds the "Prompt by app" submenu: the prompt of the frontmost application, then one submenu for each prompt name
/// of the "Prompt" list and for each `[refine.apps]` key of another name with applications.
fn by_app(menu: &mut Builder, view: &View, apps: &AppNames) {
    let listed = view.prompts.iter().map(|prompt| (prompt, true));
    let others = view
        .lists
        .iter()
        .filter(|(name, list)| !list.is_empty() && !view.prompts.contains(name))
        .map(|(name, _)| (name, false));
    menu.submenu("Prompt by app", |menu| {
        if let Some(app) = &apps.frontmost {
            let title = match prompts::route(&view.lists, Some(app.as_str())) {
                Some(prompt) => format!("{} uses {}", apps.name(app), prompt.as_str()),
                None => format!("{} uses the default prompt", apps.name(app)),
            };
            menu.status(&title);
            menu.items.push(MenuItem::Separator);
        }
        for (prompt, exists) in listed.chain(others) {
            menu.submenu(prompt.as_str(), |menu| {
                app_list(menu, view, apps, prompt, exists);
            });
        }
    });
}

/// Adds one checked item for each application of the list of `prompt`: a click removes the application.
/// When `prompt` exists, it then adds "Add" for the frontmost application, unless the list already contains it.
fn app_list(menu: &mut Builder, view: &View, apps: &AppNames, prompt: &PromptName, exists: bool) {
    let list = view.lists.get(prompt).map_or(&[][..], Vec::as_slice);
    for app in list {
        let action = Action::RemoveApp {
            prompt: prompt.clone(),
            app: app.clone(),
        };
        menu.toggle(&apps.name(app), action, true);
    }
    if let Some(app) = apps
        .frontmost
        .as_ref()
        .filter(|app| exists && !list.contains(app))
    {
        if !list.is_empty() {
            menu.items.push(MenuItem::Separator);
        }
        let action = Action::AddApp {
            prompt: prompt.clone(),
            app: app.clone(),
        };
        menu.action(format!("Add {}", apps.name(app)), action);
    }
    if menu.items.is_empty() {
        menu.status("No application");
    }
}

/// Adds the status lines: the missing permissions and the API key, which need a restart, then the file errors.
/// When nothing needs action, the line says how to dictate.
fn status(menu: &mut Builder, facts: &Facts, view: &View) {
    for &permission in &facts.missing {
        menu.action(
            permission_title(permission).to_owned(),
            Action::OpenPermission(permission),
        );
    }
    if let Some(error) = facts.key {
        menu.status(key_title(error));
    }
    if view.key_refused {
        menu.status("The provider refused the API key");
    }
    if !menu.items.is_empty() {
        menu.status("Then open Bloop again");
    } else if !facts.tap {
        menu.status("Open Bloop again to use the dictation key");
    }
    if let Err(error) = &view.settings {
        menu.action(config_title(*error), Action::OpenConfig);
    }
    for (name, error) in view.prompt.iter().chain(&view.routed) {
        menu.action(prompt_title(name, *error), Action::OpenPrompts);
    }
    if let Some(error) = &view.dictionary {
        menu.action(dictionary_title(*error), Action::OpenDictionary);
    }
    if menu.items.is_empty() {
        menu.status("Ready: hold Right Command to dictate");
    }
}

fn permission_title(permission: Permission) -> &'static str {
    match permission {
        Permission::Microphone => "Allow Microphone access…",
        Permission::Accessibility => "Allow Accessibility access…",
        Permission::InputMonitoring => "Allow Input Monitoring…",
    }
}

fn key_title(error: SecretError) -> &'static str {
    match error {
        SecretError::NotFound => "Add the API key to Keychain",
        SecretError::Unavailable => "Allow Bloop to read the API key in Keychain",
    }
}

fn config_title(error: settings::Invalid) -> String {
    use settings::Invalid;
    match error {
        Invalid::Parse { line } | Invalid::Schema { line } => {
            with_line("config.toml: invalid setting", line)
        }
        Invalid::TooLarge => "config.toml: file too large".to_owned(),
        Invalid::Unreadable(_) => "config.toml: file not readable".to_owned(),
    }
}

fn dictionary_title(error: dictionary::Invalid) -> String {
    use dictionary::Invalid;
    match error {
        Invalid::Parse { line } | Invalid::EmptyWritten { line } => {
            with_line("dictionary.toml: invalid entry", line)
        }
        Invalid::TooLarge => "dictionary.toml: file too large".to_owned(),
        Invalid::Unreadable(_) => "dictionary.toml: file not readable".to_owned(),
    }
}

fn with_line(title: &str, line: Option<usize>) -> String {
    match line {
        Some(line) => format!("{title}, line {line}"),
        None => title.to_owned(),
    }
}

fn prompt_title(name: &PromptName, error: PromptError) -> String {
    let reason = match error {
        PromptError::Missing => "not found",
        PromptError::Empty | PromptError::TooLarge | PromptError::Unreadable(_) => "not valid",
    };
    format!("Prompt \"{}\": {reason}", name.as_str())
}

/// The items of a model and the list of their actions.
#[derive(Default)]
struct Builder {
    items: Vec<MenuItem>,
    actions: Vec<Action>,
}

impl Builder {
    /// Adds `action` to the list and returns its identifier.
    fn add(&mut self, action: Action) -> ActionId {
        let id = ActionId(self.actions.len());
        self.actions.push(action);
        id
    }

    fn status(&mut self, title: &str) {
        self.items.push(MenuItem::Status {
            title: title.to_owned(),
        });
    }

    fn action(&mut self, title: String, action: Action) {
        let action = self.add(action);
        self.items.push(MenuItem::Action { title, action });
    }

    fn toggle(&mut self, title: &str, action: Action, checked: bool) {
        let action = self.add(action);
        self.items.push(MenuItem::Toggle {
            title: title.to_owned(),
            action,
            checked,
        });
    }

    /// Adds a submenu with the items that `fill` adds. The actions of the submenu follow the actions before it.
    fn submenu(&mut self, title: &str, fill: impl FnOnce(&mut Self)) {
        let outer = std::mem::take(&mut self.items);
        fill(self);
        let items = std::mem::replace(&mut self.items, outer);
        self.items.push(MenuItem::Submenu {
            title: title.to_owned(),
            items,
        });
    }

    /// Adds a submenu with one option for each of `values`, without duplicates, and a checkmark on `current`.
    /// When `current` is not in `values`, the submenu adds it at the end. Thus the menu always shows the current value.
    fn choice<T: Clone + PartialEq>(
        &mut self,
        title: &str,
        values: &[T],
        current: &T,
        label: impl Fn(&T) -> String,
        action: impl Fn(T) -> Action,
    ) {
        let mut shown: Vec<&T> = Vec::new();
        for value in values.iter().chain([current]) {
            if !shown.contains(&value) {
                shown.push(value);
            }
        }
        self.submenu(title, |menu| {
            for value in shown {
                menu.toggle(&label(value), action(value.clone()), value == current);
            }
        });
    }
}

#[cfg(test)]
mod tests;
