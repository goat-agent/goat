use goat_protocol::{AccountChoice, ModelEntry, ModelTarget};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    layout::{LIST_MAX, OVERLAY_CHROME, OVERLAY_W},
    overlay::{
        centered_rect, clamp_u16, hint_line, overflow_hint, overlay_frame, overlay_layout,
        selection_row,
    },
    symbols,
    theme::Theme,
};

pub enum PickerOutcome {
    NoOp,
    Selected(ModelTarget),
}

struct AccountPicker {
    choices: Vec<AccountChoice>,
    cursor: usize,
}

pub struct Picker {
    entries: Vec<ModelEntry>,
    query: String,
    matches: Vec<ModelEntry>,
    cursor: usize,
    current: Option<ModelTarget>,
    account: Option<AccountPicker>,
    loading: bool,
}

impl Picker {
    pub fn new(entries: Vec<ModelEntry>, current: Option<ModelTarget>, loading: bool) -> Self {
        let mut picker = Self {
            entries,
            query: String::new(),
            matches: Vec::new(),
            cursor: 0,
            current,
            account: None,
            loading,
        };
        picker.refilter();
        picker.cursor = picker.current_index().unwrap_or(0);
        picker
    }

    fn current_index(&self) -> Option<usize> {
        let current = self.current.as_ref()?;
        self.matches
            .iter()
            .position(|e| e.provider == current.provider && e.model == current.model)
    }

    pub fn set_entries(&mut self, entries: Vec<ModelEntry>) {
        self.entries = entries;
        self.loading = false;
        self.refilter();
    }

    fn refilter(&mut self) {
        let needle = self.query.to_lowercase();
        self.matches = self
            .entries
            .iter()
            .filter(|entry| {
                format!("{}/{}", entry.provider, entry.model)
                    .to_lowercase()
                    .contains(&needle)
            })
            .cloned()
            .collect();
        if self.cursor >= self.matches.len() {
            self.cursor = self.matches.len().saturating_sub(1);
        }
    }

    pub fn on_char(&mut self, ch: char) {
        if self.account.is_some() {
            return;
        }
        self.query.push(ch);
        self.refilter();
    }

    pub fn backspace(&mut self) {
        if self.account.is_some() {
            return;
        }
        self.query.pop();
        self.refilter();
    }

    pub fn move_up(&mut self) {
        if let Some(account) = &mut self.account {
            account.cursor = account.cursor.saturating_sub(1);
        } else {
            self.cursor = self.cursor.saturating_sub(1);
        }
    }

    pub fn move_down(&mut self) {
        if let Some(account) = &mut self.account {
            if account.cursor + 1 < account.choices.len() {
                account.cursor += 1;
            }
        } else if self.cursor + 1 < self.matches.len() {
            self.cursor += 1;
        }
    }

    pub fn choose(&mut self) -> PickerOutcome {
        if let Some(account) = &self.account {
            return account
                .choices
                .get(account.cursor)
                .map_or(PickerOutcome::NoOp, |choice| {
                    PickerOutcome::Selected(choice.target.clone())
                });
        }
        let Some(entry) = self.matches.get(self.cursor) else {
            return PickerOutcome::NoOp;
        };
        match entry.accounts.as_slice() {
            [] => PickerOutcome::NoOp,
            [single] => PickerOutcome::Selected(single.target.clone()),
            _ => {
                self.account = Some(AccountPicker {
                    choices: entry.accounts.clone(),
                    cursor: 0,
                });
                PickerOutcome::NoOp
            }
        }
    }

    pub fn desired_height(&self) -> u16 {
        let rows = match &self.account {
            Some(account) => account.choices.len().max(1),
            None => self.matches.len().max(1),
        };
        clamp_u16(rows.min(LIST_MAX)).saturating_add(OVERLAY_CHROME)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let rect = centered_rect(area, OVERLAY_W, self.desired_height());
        let Some(inner) = overlay_frame(frame, rect, theme) else {
            return;
        };

        if let Some(account) = &self.account {
            render_account(frame, inner, theme, account);
            return;
        }

        let (input_area, list_area, hint_area) = overlay_layout(inner);

        let input_line = if self.query.is_empty() {
            Line::from(Span::styled(" search models", theme.muted()))
        } else {
            Line::from(Span::styled(format!(" {}", self.query), theme.base()))
        };
        frame.render_widget(Paragraph::new(input_line), input_area);

        let width = usize::from(list_area.width);
        let rows = usize::from(list_area.height);
        let mut lines: Vec<Line> = Vec::new();
        if self.matches.is_empty() {
            let message = if self.loading && self.query.is_empty() {
                format!(" loading models {}", symbols::ui::ELLIPSIS)
            } else {
                format!(
                    " no models yet {} run /config to connect a provider",
                    symbols::ui::ELLIPSIS
                )
            };
            lines.push(Line::from(Span::styled(message, theme.muted())));
        } else {
            let start = if self.cursor >= rows {
                self.cursor + 1 - rows
            } else {
                0
            };
            let shown = rows.min(self.matches.len().saturating_sub(start));
            let (hint_above, hint_below) = overflow_hint(start, shown, self.matches.len());
            if let Some(ref above) = hint_above {
                lines.push(Line::from(Span::styled(format!(" {above}"), theme.muted())));
            }
            for (idx, entry) in self.matches.iter().enumerate().skip(start).take(rows) {
                let selected = idx == self.cursor;
                let is_current = self
                    .current
                    .as_ref()
                    .is_some_and(|c| c.model == entry.model && c.provider == entry.provider);
                let ctx = entry.context_window.map_or_else(String::new, |w| {
                    let k = w / 1000;
                    if k > 0 {
                        format!("{k}k")
                    } else {
                        format!("{w}")
                    }
                });
                let name = format!("{}/{}", entry.provider, entry.model);
                let name_style = if selected {
                    theme.key()
                } else if is_current {
                    theme.accent()
                } else {
                    theme.base()
                };
                let right = if ctx.is_empty() {
                    None
                } else {
                    Some(Span::styled(ctx, theme.muted()))
                };
                lines.push(selection_row(
                    theme,
                    selected,
                    width,
                    vec![Span::styled(name, name_style)],
                    right,
                ));
            }
            if let Some(ref below) = hint_below {
                lines.push(Line::from(Span::styled(format!(" {below}"), theme.muted())));
            }
        }
        frame.render_widget(Paragraph::new(lines), list_area);

        frame.render_widget(
            Paragraph::new(hint_line(
                &[
                    (symbols::key::ARROWS_UPDOWN, "navigate"),
                    (symbols::key::ENTER, "select"),
                    (symbols::key::ESC, "close"),
                ],
                theme,
            )),
            hint_area,
        );

        let col = 1 + self.query.width();
        let x = input_area.x + clamp_u16(col);
        frame.set_cursor_position((x.min(input_area.right().saturating_sub(1)), input_area.y));
    }
}

fn render_account(frame: &mut Frame, inner: Rect, theme: Theme, account: &AccountPicker) {
    let (context_area, list_area, hint_area) = overlay_layout(inner);

    let model_label = account.choices.first().map_or_else(String::new, |c| {
        format!("{}/{}", c.target.provider, c.target.model)
    });
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {model_label}"),
            theme.muted(),
        ))),
        context_area,
    );

    let width = usize::from(list_area.width);
    let rows = usize::from(list_area.height);
    let lines: Vec<Line> = account
        .choices
        .iter()
        .take(rows)
        .enumerate()
        .map(|(index, choice)| {
            let selected = index == account.cursor;
            let name_style = if selected { theme.key() } else { theme.base() };
            selection_row(
                theme,
                selected,
                width,
                vec![Span::styled(choice.display.clone(), name_style)],
                None,
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list_area);

    frame.render_widget(
        Paragraph::new(hint_line(
            &[
                (symbols::key::ARROWS_UPDOWN, "navigate"),
                (symbols::key::ENTER, "select"),
                (symbols::key::ESC, "back"),
            ],
            theme,
        )),
        hint_area,
    );
}

#[cfg(test)]
mod tests {
    use goat_protocol::{AccountChoice, ModelEntry, ModelTarget};

    use super::{Picker, PickerOutcome};

    fn entry(provider: &str, model: &str, accounts: usize) -> ModelEntry {
        let choices = (0..accounts)
            .map(|i| {
                let id = format!("acct-{i}");
                AccountChoice {
                    id: id.clone(),
                    display: id.clone(),
                    target: ModelTarget {
                        provider: provider.to_owned(),
                        model: model.to_owned(),
                        account: id,
                        effort: None,
                    },
                }
            })
            .collect();
        ModelEntry {
            provider: provider.to_owned(),
            model: model.to_owned(),
            accounts: choices,
            context_window: None,
            supports_images: true,
            efforts: Vec::new(),
        }
    }

    #[test]
    fn single_account_selects_directly() {
        let mut picker = Picker::new(vec![entry("openai", "gpt", 1)], None, false);
        match picker.choose() {
            PickerOutcome::Selected(target) => {
                assert_eq!(target.provider, "openai");
                assert_eq!(target.model, "gpt");
            }
            PickerOutcome::NoOp => panic!("expected direct selection"),
        }
    }

    #[test]
    fn multiple_accounts_open_interstitial() {
        let mut picker = Picker::new(vec![entry("openai", "gpt", 2)], None, false);
        assert!(matches!(picker.choose(), PickerOutcome::NoOp));
        picker.move_down();
        match picker.choose() {
            PickerOutcome::Selected(target) => assert_eq!(target.account, "acct-1"),
            PickerOutcome::NoOp => panic!("expected account selection"),
        }
    }

    #[test]
    fn filter_narrows_matches() {
        let mut picker = Picker::new(
            vec![entry("openai", "gpt", 1), entry("anthropic", "claude", 1)],
            None,
            false,
        );
        for ch in "claude".chars() {
            picker.on_char(ch);
        }
        match picker.choose() {
            PickerOutcome::Selected(target) => assert_eq!(target.provider, "anthropic"),
            PickerOutcome::NoOp => panic!("expected filtered selection"),
        }
    }

    #[test]
    fn empty_choose_is_noop() {
        let mut picker = Picker::new(vec![], None, false);
        assert!(matches!(picker.choose(), PickerOutcome::NoOp));
    }
}
