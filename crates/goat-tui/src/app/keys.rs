use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use goat_protocol::Op;

use super::{App, CLEAR_ARM_TICKS, PendingScreen, QUIT_ARM_TICKS};
use crate::keymap;

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
        if let Some(ch) = keymap::ctrl_key(&key) {
            if ch == 'c' {
                return self.on_ctrl_c();
            }
            self.arming.quit = None;
            self.arming.clear = None;
            self.arming.rewind = None;
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
                    self.dirty |= self.viewport.transcript.toggle_thinking();
                }
                _ => {}
            }
            return Vec::new();
        }
        self.arming.quit = None;
        if !matches!(key.code, KeyCode::Esc) {
            self.arming.clear = None;
            self.arming.rewind = None;
        }
        let mut ops = self.on_normal_key(key);
        ops.extend(self.tick_screen());
        ops
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
                self.viewport.scroll = self.viewport.scroll.saturating_sub(self.page_rows());
                self.viewport.follow = false;
                self.dirty = true;
                Vec::new()
            }
            KeyCode::PageDown => {
                self.viewport.scroll = self.viewport.scroll.saturating_add(self.page_rows());
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
                self.screens.active = PendingScreen::None;
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
                    self.viewport.scroll = 0;
                    self.viewport.follow = false;
                    self.dirty = true;
                } else {
                    self.dirty |= self.composer.move_home();
                }
                Vec::new()
            }
            KeyCode::End => {
                if self.composer.is_empty() {
                    self.viewport.follow = true;
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
                if self.viewport.selection.take().is_some() {
                    return Vec::new();
                }
                if let Some(id) = self.turn.active {
                    self.arming.clear = None;
                    self.arming.rewind = None;
                    return vec![Op::Interrupt { id }];
                }
                self.screens.active = PendingScreen::None;
                if self.composer.is_empty() {
                    self.arming.clear = None;
                    if self.composer.shell() {
                        self.arming.rewind = None;
                        self.composer.exit_shell();
                        return Vec::new();
                    }
                    if self.arming.rewind.take().is_some() {
                        return self.request_rewind();
                    }
                    self.arming.rewind = Some(CLEAR_ARM_TICKS);
                    return Vec::new();
                }
                self.arming.rewind = None;
                if self.arming.clear.take().is_some() {
                    self.composer.discard();
                } else {
                    self.arming.clear = Some(CLEAR_ARM_TICKS);
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

    pub(crate) fn on_ctrl_c(&mut self) -> Vec<Op> {
        self.dirty = true;
        self.arming.clear = None;
        if self.turn.active_shell
            && let Some(id) = self.turn.active
        {
            return vec![Op::Interrupt { id }];
        }
        if self.arming.quit.is_some() {
            self.exit_requested = true;
            self.should_quit = true;
        } else {
            self.composer.discard();
            self.arming.quit = Some(QUIT_ARM_TICKS);
        }
        Vec::new()
    }
}
