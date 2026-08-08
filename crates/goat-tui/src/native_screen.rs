use std::sync::{Arc, Mutex, Weak};

use crossterm::event::{Event as InputEvent, KeyCode};
use goat_command::{
    CommandEffect, InputOutcome, KeyHint, Placement, Screen, ScreenOutcome, Session, Theme,
};
use goat_protocol::{Event, Op, TaskId, ToolCallId, ToolImageData};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    ask::{AskOutcome, AskPicker},
    command::CommandMenu,
    files::FileMenu,
};

pub struct CommandMenuScreen {
    menu: Arc<Mutex<CommandMenu>>,
    done: bool,
}

impl CommandMenuScreen {
    pub fn new(menu: CommandMenu) -> (Self, Weak<Mutex<CommandMenu>>) {
        let menu = Arc::new(Mutex::new(menu));
        let handle = Arc::downgrade(&menu);
        (Self { menu, done: false }, handle)
    }
}

impl Screen for CommandMenuScreen {
    fn placement(&self) -> Placement {
        Placement::Panel {
            height: self.menu.lock().unwrap().desired_height(),
            hints: Some(vec![
                KeyHint {
                    key: crate::symbols::key::TAB,
                    label: "complete",
                },
                KeyHint {
                    key: crate::symbols::key::ENTER,
                    label: "run",
                },
            ]),
            composer_focused: true,
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.menu.lock().unwrap().render(frame, area, *theme);
    }

    fn handle_input(&mut self, event: &InputEvent, session: &mut dyn Session) -> InputOutcome {
        let InputEvent::Key(key) = event else {
            return InputOutcome::Ignored;
        };
        let outcome = match key.code {
            KeyCode::Tab => {
                if let Some(completion) = self.menu.lock().unwrap().selected_completion() {
                    let completed = completion.apply(&session.composer().text());
                    session.composer().set_plain_text(&completed);
                }
                ScreenOutcome::Continue
            }
            KeyCode::Enter => {
                if let Some(completion) = self.menu.lock().unwrap().selected_command_completion() {
                    let completed = completion.apply(&session.composer().text());
                    session.composer().set_plain_text(&completed);
                    ScreenOutcome::Continue
                } else {
                    if let Some(completion) = self.menu.lock().unwrap().selected_submit_completion()
                    {
                        let completed = completion.apply(&session.composer().text());
                        session.composer().set_plain_text(&completed);
                    }
                    self.done = true;
                    return InputOutcome::Ignored;
                }
            }
            KeyCode::Esc => ScreenOutcome::Close,
            KeyCode::Up => {
                self.menu.lock().unwrap().move_up();
                ScreenOutcome::Continue
            }
            KeyCode::Down => {
                self.menu.lock().unwrap().move_down();
                ScreenOutcome::Continue
            }
            _ => return InputOutcome::Ignored,
        };
        InputOutcome::Handled(outcome)
    }

    fn tick(&mut self) -> ScreenOutcome {
        if self.done {
            ScreenOutcome::Close
        } else {
            ScreenOutcome::Continue
        }
    }
}

pub struct FileMenuScreen {
    menu: Arc<Mutex<FileMenu>>,
}

impl FileMenuScreen {
    pub fn new(menu: FileMenu) -> (Self, Weak<Mutex<FileMenu>>) {
        let menu = Arc::new(Mutex::new(menu));
        let handle = Arc::downgrade(&menu);
        (Self { menu }, handle)
    }
}

impl Screen for FileMenuScreen {
    fn placement(&self) -> Placement {
        Placement::Panel {
            height: self.menu.lock().unwrap().desired_height(),
            hints: None,
            composer_focused: true,
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.menu.lock().unwrap().render(frame, area, *theme);
    }

    fn handle_input(&mut self, event: &InputEvent, session: &mut dyn Session) -> InputOutcome {
        let InputEvent::Key(key) = event else {
            return InputOutcome::Ignored;
        };
        let outcome = match key.code {
            KeyCode::Tab | KeyCode::Enter => {
                if let Some(path) = self.menu.lock().unwrap().selected() {
                    session.composer().replace_at_query(&path);
                }
                ScreenOutcome::Close
            }
            KeyCode::Esc => ScreenOutcome::Close,
            KeyCode::Up => {
                self.menu.lock().unwrap().move_up();
                ScreenOutcome::Continue
            }
            KeyCode::Down => {
                self.menu.lock().unwrap().move_down();
                ScreenOutcome::Continue
            }
            _ => return InputOutcome::Ignored,
        };
        InputOutcome::Handled(outcome)
    }
}

pub enum RunRow {
    Main {
        viewing: bool,
    },
    Subagent {
        done: Option<bool>,
        kind: String,
        label: String,
        tools: u64,
        tokens: u64,
        started_at: std::time::Instant,
        finished_at: Option<std::time::Instant>,
        viewing: bool,
    },
    Process {
        id: goat_protocol::RunId,
        command: String,
        state: goat_protocol::ProcessState,
        exit_code: Option<i32>,
        viewing: bool,
    },
}

pub struct RunScreenState {
    pub cursor: usize,
    pub rows: Vec<RunRow>,
    spinner: usize,
}

pub struct RunScreen {
    state: Arc<Mutex<RunScreenState>>,
}

impl RunScreen {
    pub fn new(rows: Vec<RunRow>, cursor: usize) -> (Self, Weak<Mutex<RunScreenState>>) {
        let state = Arc::new(Mutex::new(RunScreenState {
            cursor,
            rows,
            spinner: 0,
        }));
        let handle = Arc::downgrade(&state);
        (Self { state }, handle)
    }
}

impl Screen for RunScreen {
    fn placement(&self) -> Placement {
        let rows = self
            .state
            .lock()
            .unwrap()
            .rows
            .len()
            .clamp(1, crate::layout::LIST_MAX);
        Placement::Panel {
            height: u16::try_from(rows).unwrap_or(u16::MAX),
            hints: Some(vec![
                KeyHint {
                    key: crate::symbols::key::ARROWS_UPDOWN,
                    label: "move",
                },
                KeyHint {
                    key: crate::symbols::key::ENTER,
                    label: "select",
                },
                KeyHint {
                    key: crate::symbols::key::ESC,
                    label: "back",
                },
            ]),
            composer_focused: false,
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        use ratatui::text::Span;
        let state = self.state.lock().unwrap();
        let width = usize::from(area.width);
        let spinner = crate::symbols::SPINNER[state.spinner % crate::symbols::SPINNER.len()];
        let lines = state
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let selected = index == state.cursor;
                let name_style = if selected { theme.key() } else { theme.muted() };
                match row {
                    RunRow::Main { viewing } => crate::overlay::selection_row(
                        *theme,
                        selected,
                        width,
                        vec![
                            Span::styled(crate::symbols::ui::DOT_FULL, theme.accent()),
                            Span::raw(" "),
                            Span::styled("main", name_style),
                        ],
                        viewing.then(|| Span::styled("viewing", theme.accent())),
                    ),
                    RunRow::Subagent {
                        done,
                        kind,
                        label,
                        tools,
                        tokens,
                        started_at,
                        finished_at,
                        viewing,
                    } => {
                        let (marker, style) = match done {
                            None => (spinner, theme.accent()),
                            Some(true) => (crate::symbols::ui::CHECK, theme.success()),
                            Some(false) => (crate::symbols::ui::CROSS, theme.error()),
                        };
                        let mut left = vec![
                            Span::styled(marker, style),
                            Span::raw(" "),
                            Span::styled(kind.clone(), name_style),
                        ];
                        if !label.is_empty() {
                            left.push(Span::styled(crate::symbols::ui::SEPARATOR, theme.muted()));
                            left.push(Span::styled(label.clone(), theme.muted()));
                        }
                        let metrics = if width >= 72 {
                            let mut parts = Vec::new();
                            if *tools > 0 {
                                parts.push(format!("{tools} tools"));
                            }
                            if *tokens > 0 {
                                parts
                                    .push(format!("{} tok", crate::layout::format_tokens(*tokens)));
                            }
                            let finished = finished_at.unwrap_or_else(std::time::Instant::now);
                            parts.push(crate::transcript::format_elapsed(
                                finished.saturating_duration_since(*started_at).as_secs(),
                            ));
                            Some(Span::styled(
                                parts.join(crate::symbols::ui::SEPARATOR),
                                theme.muted(),
                            ))
                        } else {
                            viewing.then(|| Span::styled("viewing", theme.accent()))
                        };
                        crate::overlay::selection_row(*theme, selected, width, left, metrics)
                    }
                    RunRow::Process {
                        id,
                        command,
                        state: process_state,
                        exit_code,
                        viewing,
                    } => {
                        let (marker, style) = match process_state {
                            goat_protocol::ProcessState::Running => (spinner, theme.accent()),
                            goat_protocol::ProcessState::Exited => match exit_code {
                                Some(0) | None => (crate::symbols::ui::CHECK, theme.success()),
                                Some(_) => (crate::symbols::ui::CROSS, theme.error()),
                            },
                        };
                        let flat = command.split_whitespace().collect::<Vec<_>>().join(" ");
                        let command = if flat.chars().count() > 48 {
                            format!(
                                "{}{}",
                                flat.chars().take(48).collect::<String>(),
                                crate::symbols::ui::ELLIPSIS
                            )
                        } else {
                            flat
                        };
                        crate::overlay::selection_row(
                            *theme,
                            selected,
                            width,
                            vec![
                                Span::styled(marker, style),
                                Span::raw(" "),
                                Span::styled(format!("#{id}"), name_style),
                                Span::styled(crate::symbols::ui::SEPARATOR, theme.muted()),
                                Span::styled(command, theme.muted()),
                            ],
                            viewing.then(|| Span::styled("viewing", theme.accent())),
                        )
                    }
                }
            })
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn handle_input(&mut self, event: &InputEvent, session: &mut dyn Session) -> InputOutcome {
        let InputEvent::Key(key) = event else {
            return InputOutcome::Ignored;
        };
        let outcome = match key.code {
            KeyCode::Esc => {
                session.viewport().close_run_selector();
                ScreenOutcome::Close
            }
            KeyCode::Enter => {
                let cursor = self.state.lock().unwrap().cursor;
                session.viewport().open_run(cursor);
                ScreenOutcome::Continue
            }
            KeyCode::Up => {
                let mut state = self.state.lock().unwrap();
                if state.cursor == 0 {
                    drop(state);
                    session.viewport().close_run_selector();
                    ScreenOutcome::Close
                } else {
                    state.cursor -= 1;
                    ScreenOutcome::Continue
                }
            }
            KeyCode::Down => {
                let mut state = self.state.lock().unwrap();
                if state.cursor + 1 < state.rows.len() {
                    state.cursor += 1;
                    ScreenOutcome::Continue
                } else {
                    drop(state);
                    session.viewport().close_run_selector();
                    ScreenOutcome::Close
                }
            }
            KeyCode::PageUp => {
                let viewport = session.viewport();
                viewport.set_scroll(viewport.scroll().saturating_sub(viewport.page_rows()));
                viewport.set_follow(false);
                ScreenOutcome::Continue
            }
            KeyCode::PageDown => {
                let viewport = session.viewport();
                viewport.set_scroll(viewport.scroll().saturating_add(viewport.page_rows()));
                ScreenOutcome::Continue
            }
            _ => ScreenOutcome::Continue,
        };
        InputOutcome::Handled(outcome)
    }

    fn tick(&mut self) -> ScreenOutcome {
        let mut state = self.state.lock().unwrap();
        state.spinner = state.spinner.wrapping_add(1);
        ScreenOutcome::Continue
    }
}

pub struct AskScreen {
    picker: AskPicker,
    task: TaskId,
    call: ToolCallId,
    done: bool,
}

impl AskScreen {
    pub fn new(picker: AskPicker, task: TaskId, call: ToolCallId) -> Self {
        Self {
            picker,
            task,
            call,
            done: false,
        }
    }

    pub fn call(&self) -> ToolCallId {
        self.call
    }

    fn submit(&mut self, answers: Vec<String>) -> ScreenOutcome {
        self.done = true;
        ScreenOutcome::Effect(CommandEffect::Dispatch(vec![Op::Answer {
            id: self.task,
            call: self.call,
            answers,
        }]))
    }
}

impl Screen for AskScreen {
    fn placement(&self) -> Placement {
        Placement::Full {
            reserve_bottom: Some(self.picker.desired_height()),
        }
    }

    fn captures_text(&self) -> bool {
        true
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.picker.render(frame, area, *theme);
    }

    fn handle_input(&mut self, event: &InputEvent, _session: &mut dyn Session) -> InputOutcome {
        if let InputEvent::Paste(text) = event {
            self.picker.insert_str(text);
            return InputOutcome::Handled(ScreenOutcome::Continue);
        }
        let InputEvent::Key(key) = event else {
            return InputOutcome::Ignored;
        };
        if let Some(ch) = goat_command::keymap::ctrl_key(key) {
            let outcome = if ch == 'c' {
                self.done = true;
                ScreenOutcome::Effect(CommandEffect::Dispatch(vec![Op::Interrupt {
                    id: self.task,
                }]))
            } else {
                ScreenOutcome::Continue
            };
            return InputOutcome::Handled(outcome);
        }
        let outcome = match key.code {
            KeyCode::Esc => {
                if self.picker.is_confirming() || self.picker.is_typing() {
                    self.picker.go_back();
                    ScreenOutcome::Continue
                } else {
                    self.done = true;
                    ScreenOutcome::Effect(CommandEffect::Dispatch(vec![Op::Interrupt {
                        id: self.task,
                    }]))
                }
            }
            KeyCode::Up => {
                self.picker.move_up();
                ScreenOutcome::Continue
            }
            KeyCode::Down => {
                self.picker.move_down();
                ScreenOutcome::Continue
            }
            KeyCode::Left => {
                self.picker.go_back();
                ScreenOutcome::Continue
            }
            KeyCode::Right => match self.picker.skip() {
                AskOutcome::Submit(answers) => self.submit(answers),
                AskOutcome::Pending | AskOutcome::NoOp => ScreenOutcome::Continue,
            },
            KeyCode::Backspace => {
                self.picker.backspace();
                ScreenOutcome::Continue
            }
            KeyCode::Enter => match self.picker.choose() {
                AskOutcome::Submit(answers) => self.submit(answers),
                AskOutcome::Pending | AskOutcome::NoOp => ScreenOutcome::Continue,
            },
            KeyCode::Char(ch) => {
                if ch == ' ' && self.picker.wants_toggle() {
                    self.picker.toggle();
                } else {
                    self.picker.on_char(ch);
                }
                ScreenOutcome::Continue
            }
            _ => ScreenOutcome::Continue,
        };
        InputOutcome::Handled(outcome)
    }

    fn on_event(&mut self, event: &Event, _session: &mut dyn Session) -> ScreenOutcome {
        if matches!(event, Event::AskDismissed { call, .. } if *call == self.call) {
            ScreenOutcome::Close
        } else {
            ScreenOutcome::Continue
        }
    }

    fn tick(&mut self) -> ScreenOutcome {
        if self.done {
            ScreenOutcome::Close
        } else {
            ScreenOutcome::Continue
        }
    }
}

pub struct ImageZoomScreen {
    source: Box<ToolImageData>,
    picker: Option<Arc<ratatui_image::picker::Picker>>,
}

impl ImageZoomScreen {
    pub fn new(
        source: Box<ToolImageData>,
        picker: Option<Arc<ratatui_image::picker::Picker>>,
    ) -> Self {
        Self { source, picker }
    }
}

impl Screen for ImageZoomScreen {
    fn placement(&self) -> Placement {
        Placement::Full {
            reserve_bottom: None,
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let [body, hint] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        let image_area = body.inner(Margin {
            horizontal: 2,
            vertical: 1,
        });
        if let Some(picker) = &self.picker {
            crate::screenshot::render_zoom(frame, image_area, picker, &self.source);
        } else {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    " image preview unavailable in this terminal ",
                    theme.muted(),
                ))),
                image_area,
            );
        }
        frame.render_widget(
            Paragraph::new(crate::overlay::hint_line(
                &[(crate::symbols::key::ESC, "close")],
                *theme,
            )),
            hint,
        );
    }

    fn handle_input(&mut self, event: &InputEvent, _session: &mut dyn Session) -> InputOutcome {
        let outcome = match event {
            InputEvent::Key(_) => ScreenOutcome::Close,
            InputEvent::Mouse(mouse)
                if matches!(
                    mouse.kind,
                    crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left)
                ) =>
            {
                ScreenOutcome::Close
            }
            InputEvent::Mouse(_) => ScreenOutcome::Continue,
            _ => return InputOutcome::Ignored,
        };
        InputOutcome::Handled(outcome)
    }
}
