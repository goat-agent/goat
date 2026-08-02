use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{layout::LIST_MAX, overlay, theme::Theme};

const RESULT_CAP: usize = 200;

pub struct FileMenu {
    entries: Vec<String>,
    matches: Vec<String>,
    cursor: usize,
    loading: bool,
}

impl FileMenu {
    pub fn new(entries: Vec<String>, loading: bool, query: &str) -> Self {
        let mut menu = Self {
            entries,
            matches: Vec::new(),
            cursor: 0,
            loading,
        };
        menu.refilter(query);
        menu
    }

    pub fn update(&mut self, query: &str) {
        self.refilter(query);
    }

    pub fn fill(&mut self, entries: Vec<String>, query: &str) {
        self.entries = entries;
        self.loading = false;
        self.cursor = 0;
        self.refilter(query);
    }

    fn refilter(&mut self, query: &str) {
        let needle = query.to_lowercase();
        self.matches = self
            .entries
            .iter()
            .filter(|e| needle.is_empty() || e.to_lowercase().contains(&needle))
            .take(RESULT_CAP)
            .cloned()
            .collect();
        if self.cursor >= self.matches.len() {
            self.cursor = self.matches.len().saturating_sub(1);
        }
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if self.cursor + 1 < self.matches.len() {
            self.cursor += 1;
        }
    }

    pub fn selected(&self) -> Option<String> {
        self.matches.get(self.cursor).cloned()
    }

    pub fn desired_height(&self) -> u16 {
        let rows = self.matches.len().clamp(1, LIST_MAX);
        u16::try_from(rows).unwrap_or(u16::MAX)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let width = usize::from(area.width);
        let lines = if self.matches.is_empty() {
            let note = if self.loading {
                " listing files…"
            } else {
                " no files match"
            };
            vec![Line::from(Span::styled(note, theme.muted()))]
        } else {
            let visible = usize::from(area.height).max(1);
            let start = if self.cursor >= visible {
                self.cursor + 1 - visible
            } else {
                0
            };
            self.matches
                .iter()
                .enumerate()
                .skip(start)
                .take(visible)
                .map(|(idx, entry)| {
                    let selected = idx == self.cursor;
                    let style = if selected { theme.key() } else { theme.base() };
                    overlay::selection_row(
                        theme,
                        selected,
                        width,
                        vec![Span::styled(entry.clone(), style)],
                        None,
                    )
                })
                .collect()
        };
        frame.render_widget(Paragraph::new(lines), area);
    }
}

#[cfg(test)]
mod tests {
    use super::FileMenu;

    fn entries() -> Vec<String> {
        vec![
            "src/".to_owned(),
            "src/main.rs".to_owned(),
            "README.md".to_owned(),
        ]
    }

    #[test]
    fn an_empty_query_matches_everything() {
        let menu = FileMenu::new(entries(), false, "");
        assert_eq!(menu.selected().as_deref(), Some("src/"));
    }

    #[test]
    fn the_query_filters_case_insensitively() {
        let menu = FileMenu::new(entries(), false, "readme");
        assert_eq!(menu.selected().as_deref(), Some("README.md"));
    }

    #[test]
    fn filling_replaces_a_loading_menu() {
        let mut menu = FileMenu::new(Vec::new(), true, "main");
        assert_eq!(menu.selected(), None);
        menu.fill(entries(), "main");
        assert_eq!(menu.selected().as_deref(), Some("src/main.rs"));
    }
}
