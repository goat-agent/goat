use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use goat_protocol::Op;

use super::{App, CLEAR_ARM_TICKS, Overlay, QUIT_ARM_TICKS};
use crate::{ask::AskOutcome, config::ConfigOutcome, keymap};

impl App {
    pub(crate) fn on_key(&mut self, key: KeyEvent) -> Vec<Op> {
        tracing::trace!(code = ?key.code, modifiers = ?key.modifiers, "key");
        if keymap::super_char(&key) == Some('c') {
            self.copy_selection();
            return Vec::new();
        }
        if let Some(ops) = self.handle_screen_input(&crossterm::event::Event::Key(key)) {
            return ops;
        }
        match &self.overlay {
            Overlay::Screen(_) => {}
            Overlay::Config(_) => return self.on_config_key(key),
            Overlay::Runs(_) => return self.on_run_selector_key(key),
            Overlay::Ask(_, _) => return self.on_ask_picker_key(key),
            Overlay::Plan(_) => return self.on_plan_sheet_key(key),
            Overlay::Commands(_) => {
                if let Some(result) = self.on_command_menu_key(key) {
                    return result;
                }
            }
            Overlay::Files(_) => {
                if let Some(result) = self.on_file_menu_key(key) {
                    return result;
                }
            }
            Overlay::Usage | Overlay::Status => return self.on_usage_key(key),
            Overlay::ImageZoom(_) => {
                self.overlay = Overlay::None;
                self.dirty = true;
                return Vec::new();
            }
            Overlay::None => {}
        }
        if let Some(ch) = keymap::ctrl_key(&key) {
            if ch == 'c' {
                return self.on_ctrl_c();
            }
            self.quit_arm = None;
            self.clear_arm = None;
            self.rewind_arm = None;
            match ch {
                'a' => {
                    self.dirty |= self.composer.move_home();
                }
                'e' => {
                    self.dirty |= self.composer.move_end();
                }
                'w' => {
                    self.composer.delete_word_before();
                    self.update_command_menu();
                    self.dirty = true;
                }
                't' => {
                    self.dirty |= self.transcript.toggle_thinking();
                }
                _ => {}
            }
            return Vec::new();
        }
        self.quit_arm = None;
        if !matches!(key.code, KeyCode::Esc) {
            self.clear_arm = None;
            self.rewind_arm = None;
        }
        self.on_normal_key(key)
    }

    pub(crate) fn on_command_menu_key(&mut self, key: KeyEvent) -> Option<Vec<Op>> {
        self.dirty = true;
        match key.code {
            KeyCode::Tab => {
                if let Overlay::Commands(menu) = &self.overlay
                    && let Some(completion) = menu.selected_completion()
                {
                    let text = self.composer.text();
                    let completed = completion.apply(&text);
                    self.composer.set_plain_text(&completed);
                    self.update_command_menu();
                }
                Some(Vec::new())
            }
            KeyCode::Enter => {
                if let Overlay::Commands(menu) = &self.overlay
                    && let Some(completion) = menu.selected_command_completion()
                {
                    let text = self.composer.text();
                    let completed = completion.apply(&text);
                    self.composer.set_plain_text(&completed);
                    self.update_command_menu();
                    return Some(Vec::new());
                }
                if let Overlay::Commands(menu) = &self.overlay
                    && let Some(completion) = menu.selected_submit_completion()
                {
                    let text = self.composer.text();
                    let completed = completion.apply(&text);
                    self.composer.set_plain_text(&completed);
                }
                self.overlay = Overlay::None;
                self.dirty = true;
                Some(self.submit())
            }
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Some(Vec::new())
            }
            KeyCode::Up => {
                if let Overlay::Commands(menu) = &mut self.overlay {
                    menu.move_up();
                }
                Some(Vec::new())
            }
            KeyCode::Down => {
                if let Overlay::Commands(menu) = &mut self.overlay {
                    menu.move_down();
                }
                Some(Vec::new())
            }
            _ => None,
        }
    }

    pub(crate) fn on_file_menu_key(&mut self, key: KeyEvent) -> Option<Vec<Op>> {
        match key.code {
            KeyCode::Tab | KeyCode::Enter => {
                if let Overlay::Files(menu) = &self.overlay
                    && let Some(path) = menu.selected()
                {
                    self.composer.replace_at_query(&path);
                }
                self.overlay = Overlay::None;
                self.dirty = true;
                Some(Vec::new())
            }
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                self.dirty = true;
                Some(Vec::new())
            }
            KeyCode::Up => {
                if let Overlay::Files(menu) = &mut self.overlay {
                    menu.move_up();
                }
                self.dirty = true;
                Some(Vec::new())
            }
            KeyCode::Down => {
                if let Overlay::Files(menu) = &mut self.overlay {
                    menu.move_down();
                }
                self.dirty = true;
                Some(Vec::new())
            }
            _ => None,
        }
    }

    pub(crate) fn on_plan_sheet_key(&mut self, key: KeyEvent) -> Vec<Op> {
        let Overlay::Plan(sheet) = &mut self.overlay else {
            return Vec::new();
        };
        self.dirty = true;
        if sheet.rejecting() {
            return match key.code {
                KeyCode::Esc => {
                    sheet.cancel_reject();
                    Vec::new()
                }
                KeyCode::Backspace => {
                    sheet.pop_feedback();
                    Vec::new()
                }
                KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    sheet.push_feedback(ch);
                    Vec::new()
                }
                KeyCode::Enter => {
                    let call = sheet.call;
                    let feedback = sheet.submit_reject();
                    self.overlay = Overlay::None;
                    vec![Op::ResolvePlan {
                        call,
                        decision: goat_protocol::PlanDecision::Reject { feedback },
                    }]
                }
                _ => Vec::new(),
            };
        }
        match key.code {
            KeyCode::Char('a') => {
                let call = sheet.call;
                self.overlay = Overlay::None;
                vec![Op::ResolvePlan {
                    call,
                    decision: goat_protocol::PlanDecision::Approve {},
                }]
            }
            KeyCode::Char('r') => {
                sheet.begin_reject();
                Vec::new()
            }
            KeyCode::PageUp => {
                let page = sheet.page();
                sheet.scroll_up(page);
                Vec::new()
            }
            KeyCode::PageDown => {
                let page = sheet.page();
                sheet.scroll_down(page);
                Vec::new()
            }
            KeyCode::Up => {
                sheet.scroll_up(1);
                Vec::new()
            }
            KeyCode::Down => {
                sheet.scroll_down(1);
                Vec::new()
            }
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) fn on_normal_key(&mut self, key: KeyEvent) -> Vec<Op> {
        match key.code {
            KeyCode::BackTab => {
                let mode = self.mode.toggled();
                self.mode = mode;
                self.dirty = true;
                vec![Op::SetMode { mode }]
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(self.page_rows());
                self.follow = false;
                self.dirty = true;
                Vec::new()
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(self.page_rows());
                self.dirty = true;
                Vec::new()
            }
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
            {
                self.composer.newline();
                self.dirty = true;
                Vec::new()
            }
            KeyCode::Enter => {
                self.overlay = Overlay::None;
                self.dirty = true;
                self.submit()
            }
            KeyCode::Backspace => {
                if self.composer.is_empty() && self.composer.shell() {
                    self.composer.exit_shell();
                } else if self.composer.is_empty()
                    && let Some((id, _, _, _)) = self.queued.last()
                {
                    return vec![Op::DequeueMessage { id: *id }];
                } else {
                    self.composer.backspace();
                    self.update_command_menu();
                }
                self.dirty = true;
                Vec::new()
            }
            KeyCode::Delete => {
                self.composer.delete_forward();
                self.update_command_menu();
                self.dirty = true;
                Vec::new()
            }
            KeyCode::Left => {
                let changed = if key.modifiers.contains(KeyModifiers::ALT) {
                    self.composer.move_word_left()
                } else {
                    self.composer.move_left()
                };
                self.dirty |= changed;
                Vec::new()
            }
            KeyCode::Right => {
                let changed = if key.modifiers.contains(KeyModifiers::ALT) {
                    self.composer.move_word_right()
                } else {
                    self.composer.move_right()
                };
                self.dirty |= changed;
                Vec::new()
            }
            KeyCode::Home => {
                if self.composer.is_empty() {
                    self.scroll = 0;
                    self.follow = false;
                    self.dirty = true;
                } else {
                    self.dirty |= self.composer.move_home();
                }
                Vec::new()
            }
            KeyCode::End => {
                if self.composer.is_empty() {
                    self.follow = true;
                    self.dirty = true;
                } else {
                    self.dirty |= self.composer.move_end();
                }
                Vec::new()
            }
            KeyCode::Up => {
                if self.composer.on_first_row() {
                    self.composer.history_prev();
                    self.dirty = true;
                } else {
                    self.dirty |= self.composer.move_up();
                }
                Vec::new()
            }
            KeyCode::Down => {
                if self.composer.is_empty() && !self.run_targets().is_empty() {
                    self.move_run_cursor(0);
                } else if self.composer.on_last_row() {
                    self.composer.history_next();
                    self.dirty = true;
                } else {
                    self.dirty |= self.composer.move_down();
                }
                Vec::new()
            }
            KeyCode::Esc => {
                self.dirty = true;
                if self.selection.take().is_some() {
                    return Vec::new();
                }
                if let Some(id) = self.turn.active {
                    self.clear_arm = None;
                    self.rewind_arm = None;
                    return vec![Op::Interrupt { id }];
                }
                self.overlay = Overlay::None;
                if self.composer.is_empty() {
                    self.clear_arm = None;
                    if self.composer.shell() {
                        self.rewind_arm = None;
                        self.composer.exit_shell();
                        return Vec::new();
                    }
                    if self.rewind_arm.take().is_some() {
                        return self.request_rewind();
                    }
                    self.rewind_arm = Some(CLEAR_ARM_TICKS);
                    return Vec::new();
                }
                self.rewind_arm = None;
                if self.clear_arm.take().is_some() {
                    self.composer.discard();
                } else {
                    self.clear_arm = Some(CLEAR_ARM_TICKS);
                }
                Vec::new()
            }
            KeyCode::Char('!') if self.composer.is_empty() && !self.composer.shell() => {
                self.composer.enter_shell();
                self.dirty = true;
                Vec::new()
            }
            KeyCode::Char(c) => {
                self.composer.insert_char(c);
                self.update_command_menu();
                self.dirty = true;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn on_config_key(&mut self, key: KeyEvent) -> Vec<Op> {
        self.dirty = true;
        if let Some(ch) = keymap::ctrl_key(&key) {
            if ch == 'c' {
                self.overlay = Overlay::None;
            }
            return Vec::new();
        }
        match key.code {
            KeyCode::Esc => {
                if let Overlay::Config(config) = &mut self.overlay {
                    config.cancel_stage();
                    if matches!(config.stage_kind(), crate::config::StageKind::List) {
                        self.overlay = Overlay::None;
                    }
                }
            }
            KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                if let Overlay::Config(config) = &mut self.overlay {
                    config.tab();
                }
            }
            KeyCode::Up => {
                if let Overlay::Config(config) = &mut self.overlay {
                    config.move_up();
                }
            }
            KeyCode::Down => {
                if let Overlay::Config(config) = &mut self.overlay {
                    config.move_down();
                }
            }
            KeyCode::Backspace => {
                if let Overlay::Config(config) = &mut self.overlay {
                    if matches!(config.stage_kind(), crate::config::StageKind::List) {
                        let outcome = config.remove_selected();
                        return self.apply_config_outcome(outcome);
                    }
                    config.backspace();
                }
            }
            KeyCode::Delete => {
                let outcome = if let Overlay::Config(config) = &mut self.overlay {
                    config.remove_selected()
                } else {
                    ConfigOutcome::Pending
                };
                return self.apply_config_outcome(outcome);
            }
            KeyCode::Enter => {
                let outcome = if let Overlay::Config(config) = &mut self.overlay {
                    config.enter()
                } else {
                    ConfigOutcome::Pending
                };
                return self.apply_config_outcome(outcome);
            }
            KeyCode::Char(c) => {
                if let Overlay::Config(config) = &mut self.overlay {
                    config.on_char(c);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    pub(crate) fn on_ask_picker_key(&mut self, key: KeyEvent) -> Vec<Op> {
        self.dirty = true;
        if let Some(ch) = keymap::ctrl_key(&key) {
            if ch == 'c' {
                self.overlay = Overlay::None;
                if let Some(id) = self.turn.active {
                    return vec![Op::Interrupt { id }];
                }
            }
            return Vec::new();
        }
        match key.code {
            KeyCode::Esc => return self.ask_esc(),
            KeyCode::Up => {
                if let Overlay::Ask(ref mut picker, _) = self.overlay {
                    picker.move_up();
                }
            }
            KeyCode::Down => {
                if let Overlay::Ask(ref mut picker, _) = self.overlay {
                    picker.move_down();
                }
            }
            KeyCode::Left => {
                if let Overlay::Ask(ref mut picker, _) = self.overlay {
                    picker.go_back();
                }
            }
            KeyCode::Right => {
                let outcome = if let Overlay::Ask(ref mut picker, call) = self.overlay {
                    match picker.skip() {
                        AskOutcome::Submit(answers) => Some((call, answers)),
                        AskOutcome::Pending | AskOutcome::NoOp => None,
                    }
                } else {
                    None
                };
                if let Some((call, answers)) = outcome {
                    self.overlay = Overlay::None;
                    if let Some(id) = self.turn.active {
                        return vec![Op::Answer { id, call, answers }];
                    }
                }
            }
            KeyCode::Backspace => {
                if let Overlay::Ask(ref mut picker, _) = self.overlay {
                    picker.backspace();
                }
            }
            KeyCode::Enter => return self.ask_enter(),
            KeyCode::Char(c) => {
                if let Overlay::Ask(ref mut picker, _) = self.overlay {
                    if c == ' ' && picker.wants_toggle() {
                        picker.toggle();
                    } else {
                        picker.on_char(c);
                    }
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn ask_esc(&mut self) -> Vec<Op> {
        let handled = if let Overlay::Ask(ref mut picker, _) = self.overlay {
            picker.is_confirming() || picker.is_typing()
        } else {
            false
        };
        if handled {
            if let Overlay::Ask(ref mut picker, _) = self.overlay {
                picker.go_back();
            }
            return Vec::new();
        }
        self.overlay = Overlay::None;
        if let Some(id) = self.turn.active {
            return vec![Op::Interrupt { id }];
        }
        Vec::new()
    }

    fn ask_enter(&mut self) -> Vec<Op> {
        let submit = if let Overlay::Ask(ref mut picker, call) = self.overlay {
            match picker.choose() {
                AskOutcome::Submit(answers) => Some((call, answers)),
                AskOutcome::Pending | AskOutcome::NoOp => None,
            }
        } else {
            None
        };
        if let Some((call, answers)) = submit {
            self.overlay = Overlay::None;
            if let Some(id) = self.turn.active {
                return vec![Op::Answer { id, call, answers }];
            }
        }
        Vec::new()
    }

    pub(crate) fn on_ctrl_c(&mut self) -> Vec<Op> {
        self.dirty = true;
        self.clear_arm = None;
        if self.turn.active_shell
            && let Some(id) = self.turn.active
        {
            return vec![Op::Interrupt { id }];
        }
        if self.quit_arm.is_some() {
            self.exit_requested = true;
            self.should_quit = true;
        } else {
            self.composer.discard();
            self.quit_arm = Some(QUIT_ARM_TICKS);
        }
        Vec::new()
    }

    pub(crate) fn on_run_selector_key(&mut self, key: KeyEvent) -> Vec<Op> {
        self.dirty = true;
        match key.code {
            KeyCode::Esc => self.close_run_selector(),
            KeyCode::Enter => {
                if let Some(cursor) = self.run_selector() {
                    self.open_run(cursor);
                }
            }
            KeyCode::Up => match self.run_selector() {
                Some(0) | None => self.close_run_selector(),
                Some(cursor) => self.move_run_cursor(cursor - 1),
            },
            KeyCode::Down => match self.run_selector() {
                Some(cursor) if cursor + 1 < self.run_row_count() => {
                    self.move_run_cursor(cursor + 1);
                }
                Some(_) => self.close_run_selector(),
                None => {}
            },
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(self.page_rows());
                self.follow = false;
                self.dirty = true;
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(self.page_rows());
                self.dirty = true;
            }
            _ => {}
        }
        Vec::new()
    }

    pub(crate) fn on_usage_key(&mut self, key: KeyEvent) -> Vec<Op> {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                self.overlay = Overlay::None;
                self.dirty = true;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.usage.scroll = self.usage.scroll.saturating_sub(1);
                self.dirty = true;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.usage.scroll = self.usage.scroll.saturating_add(1);
                self.dirty = true;
            }
            KeyCode::PageUp => {
                self.usage.scroll = self.usage.scroll.saturating_sub(8);
                self.dirty = true;
            }
            KeyCode::PageDown => {
                self.usage.scroll = self.usage.scroll.saturating_add(8);
                self.dirty = true;
            }
            _ => {}
        }
        Vec::new()
    }
}
