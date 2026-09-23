use crossterm::event::{KeyCode, KeyEvent};
use goat_command::KeyHint;
use goat_protocol::Op;
use ratatui::{Frame, layout::Rect};

use super::{App, PendingScreen, slash_command_name};
use crate::{
    command::{CommandMenu, CommandMenuContext, RuntimeChoiceGroup},
    files::FileMenu,
    keymap,
    theme::Theme,
};

pub(crate) enum ComposerMenu {
    Commands(CommandMenu),
    Files(FileMenu),
}

impl ComposerMenu {
    pub(crate) fn desired_height(&self) -> u16 {
        match self {
            Self::Commands(menu) => menu.desired_height(),
            Self::Files(menu) => menu.desired_height(),
        }
    }

    pub(crate) fn hints(&self) -> Option<Vec<KeyHint>> {
        match self {
            Self::Commands(_) => Some(vec![
                KeyHint {
                    key: crate::symbols::key::TAB,
                    label: "complete",
                },
                KeyHint {
                    key: crate::symbols::key::ENTER,
                    label: "run",
                },
            ]),
            Self::Files(_) => None,
        }
    }

    pub(crate) fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        match self {
            Self::Commands(menu) => menu.render(frame, area, theme),
            Self::Files(menu) => menu.render(frame, area, theme),
        }
    }

    fn move_up(&mut self) {
        match self {
            Self::Commands(menu) => menu.move_up(),
            Self::Files(menu) => menu.move_up(),
        }
    }

    fn move_down(&mut self) {
        match self {
            Self::Commands(menu) => menu.move_down(),
            Self::Files(menu) => menu.move_down(),
        }
    }
}

enum MenuWant {
    None,
    Commands(String),
    Files(String),
}

enum EnterAfter {
    Stay,
    Submit,
    Close,
}

impl App {
    pub(crate) fn sync_composer_menu(&mut self) {
        let revision = self.composer.revision();
        if self
            .menu_dismissed
            .is_some_and(|dismissed| dismissed != revision)
        {
            self.menu_dismissed = None;
        }
        let want = if self.menu_dismissed.is_some() || self.composer.shell() {
            MenuWant::None
        } else if let Some(query) = self.composer.at_query() {
            MenuWant::Files(query)
        } else {
            let text = self.composer.text();
            let trimmed = text.trim_start();
            if trimmed.starts_with('/')
                && slash_command_name(trimmed).is_none_or(|name| !name.contains('/'))
            {
                MenuWant::Commands(trimmed.to_owned())
            } else {
                MenuWant::None
            }
        };
        match want {
            MenuWant::None => {
                if self.composer_menu.take().is_some() {
                    self.dirty = true;
                }
            }
            MenuWant::Files(query) => {
                if let Some(ComposerMenu::Files(menu)) = &mut self.composer_menu {
                    menu.update(&query);
                } else {
                    if !self.files_loaded {
                        self.outbox.push(Op::ListFiles {});
                    }
                    self.composer_menu = Some(ComposerMenu::Files(FileMenu::new(
                        self.files.clone(),
                        !self.files_loaded,
                        &query,
                    )));
                    self.dirty = true;
                }
            }
            MenuWant::Commands(trimmed) => {
                let effort_options = self.effort_choice_options();
                let model_options = self.model_choice_options();
                let groups = [
                    RuntimeChoiceGroup {
                        command: "effort",
                        parameter: "level",
                        options: &effort_options,
                        empty_hint: if self.catalog.selected.is_some() {
                            "this model does not support reasoning effort"
                        } else {
                            "select a model first"
                        },
                    },
                    RuntimeChoiceGroup {
                        command: "model",
                        parameter: "name",
                        options: &model_options,
                        empty_hint: "no models yet — run /config to connect a provider",
                    },
                ];
                let context = CommandMenuContext { choices: &groups };
                if let Some(ComposerMenu::Commands(menu)) = &mut self.composer_menu {
                    menu.update(&self.commands, &trimmed, &context);
                } else {
                    self.composer_menu = Some(ComposerMenu::Commands(CommandMenu::new(
                        &self.commands,
                        &trimmed,
                        &context,
                    )));
                    self.dirty = true;
                }
            }
        }
    }

    pub(crate) fn composer_menu_key(&mut self, key: &KeyEvent) -> Option<Vec<Op>> {
        if !self.composer_menu_live() {
            return None;
        }
        if let Some(ch) = keymap::ctrl_key(key) {
            match ch {
                'n' => self.move_composer_menu(true),
                'p' => self.move_composer_menu(false),
                _ => return None,
            }
            self.dirty = true;
            return Some(Vec::new());
        }
        match key.code {
            KeyCode::Up => self.move_composer_menu(false),
            KeyCode::Down => self.move_composer_menu(true),
            KeyCode::Esc => {
                self.menu_dismissed = Some(self.composer.revision());
                self.composer_menu = None;
            }
            KeyCode::Tab => self.apply_menu_completion(),
            KeyCode::Enter if key.modifiers.is_empty() => return self.menu_enter(),
            _ => return None,
        }
        self.dirty = true;
        Some(Vec::new())
    }

    fn composer_menu_live(&self) -> bool {
        self.composer_menu.is_some()
            && self.panel_visible
            && matches!(self.screens.active, PendingScreen::None)
    }

    fn move_composer_menu(&mut self, down: bool) {
        if let Some(menu) = &mut self.composer_menu {
            if down {
                menu.move_down();
            } else {
                menu.move_up();
            }
        }
    }

    fn apply_menu_completion(&mut self) {
        let mut close = false;
        match &self.composer_menu {
            Some(ComposerMenu::Commands(menu)) => {
                if let Some(completion) = menu.selected_completion() {
                    let completed = completion.apply(&self.composer.text());
                    self.composer.set_plain_text(&completed);
                }
            }
            Some(ComposerMenu::Files(menu)) => {
                if let Some(path) = menu.selected() {
                    self.composer.replace_at_query(&path);
                }
                close = true;
            }
            None => {}
        }
        if close {
            self.composer_menu = None;
        }
    }

    fn menu_enter(&mut self) -> Option<Vec<Op>> {
        let after = match &self.composer_menu {
            Some(ComposerMenu::Commands(menu)) => {
                if let Some(completion) = menu.selected_command_completion() {
                    let completed = completion.apply(&self.composer.text());
                    self.composer.set_plain_text(&completed);
                    EnterAfter::Stay
                } else {
                    if let Some(completion) = menu.selected_submit_completion() {
                        let completed = completion.apply(&self.composer.text());
                        self.composer.set_plain_text(&completed);
                    }
                    EnterAfter::Submit
                }
            }
            Some(ComposerMenu::Files(menu)) => {
                if let Some(path) = menu.selected() {
                    self.composer.replace_at_query(&path);
                }
                EnterAfter::Close
            }
            None => EnterAfter::Close,
        };
        self.dirty = true;
        match after {
            EnterAfter::Stay => Some(Vec::new()),
            EnterAfter::Submit => {
                self.composer_menu = None;
                None
            }
            EnterAfter::Close => {
                self.composer_menu = None;
                Some(Vec::new())
            }
        }
    }
}
