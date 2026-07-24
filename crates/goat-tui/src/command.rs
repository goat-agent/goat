use goat_commands::{
    BranchSpec, ChoiceSpec, CommandRegistry, CommandShape, CommandSpec, ParameterSpec,
    ParameterValue,
};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    layout::LIST_MAX,
    overlay::{selection_row, truncate_to_width},
    theme::Theme,
};

fn subsequence_match(query: &str, target: &str) -> Option<Vec<usize>> {
    if query.is_empty() {
        return Some(Vec::new());
    }
    let mut positions = Vec::new();
    let mut target_chars = target.char_indices();
    for qc in query.chars() {
        loop {
            match target_chars.next() {
                Some((i, tc)) if tc.eq_ignore_ascii_case(&qc) => {
                    positions.push(i);
                    break;
                }
                Some(_) => {}
                None => return None,
            }
        }
    }
    Some(positions)
}

fn alias_label(aliases: &[String]) -> String {
    if aliases.is_empty() {
        String::new()
    } else {
        format!(" ({})", aliases.join(", "))
    }
}

#[derive(Clone)]
pub struct Completion {
    start: usize,
    end: usize,
    replacement: String,
    submit: bool,
}

#[derive(Clone)]
struct Row {
    label: String,
    aliases: Vec<String>,
    description: String,
    positions: Vec<usize>,
    completion: Option<Completion>,
}

enum Mode {
    Commands(Vec<Row>),
    Context(Vec<Row>),
}

#[derive(Clone)]
pub struct RuntimeChoice {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

pub struct RuntimeChoiceGroup<'a> {
    pub command: &'a str,
    pub parameter: &'a str,
    pub options: &'a [RuntimeChoice],
    pub empty_hint: &'a str,
}

#[derive(Clone, Copy)]
pub struct CommandMenuContext<'a> {
    pub choices: &'a [RuntimeChoiceGroup<'a>],
}

impl CommandMenuContext<'_> {
    fn lookup(&self, command: &str, parameter: &str) -> Option<&RuntimeChoiceGroup<'_>> {
        self.choices
            .iter()
            .find(|group| group.command == command && group.parameter == parameter)
    }
}

pub struct CommandMenu {
    cursor: usize,
    mode: Mode,
}

struct SlashParts<'a> {
    command: &'a str,
    args: &'a str,
    command_start: usize,
    command_end: usize,
    args_start: usize,
    has_space: bool,
}

impl CommandMenu {
    pub fn new(registry: &CommandRegistry, input: &str, ctx: &CommandMenuContext<'_>) -> Self {
        Self {
            cursor: 0,
            mode: Self::compute_mode(registry, input, *ctx),
        }
    }

    fn compute_mode(registry: &CommandRegistry, input: &str, ctx: CommandMenuContext<'_>) -> Mode {
        let Some(parts) = slash_parts(input) else {
            return Mode::Context(vec![hint("type a slash command")]);
        };
        if !parts.has_space {
            return Mode::Commands(Self::compute_command_matches(registry, &parts));
        }
        let Some(spec) = registry.spec(parts.command) else {
            return Mode::Context(vec![hint(format!("unknown command: /{}", parts.command))]);
        };
        Mode::Context(context_rows(&spec, &parts, ctx))
    }

    fn compute_command_matches(registry: &CommandRegistry, parts: &SlashParts<'_>) -> Vec<Row> {
        let query = parts.command.to_lowercase();
        registry
            .specs()
            .into_iter()
            .filter_map(|spec| {
                let name_positions = subsequence_match(&query, &spec.name);
                let alias_hit = name_positions.is_none()
                    && spec
                        .aliases
                        .iter()
                        .any(|alias| subsequence_match(&query, alias).is_some());
                (name_positions.is_some() || alias_hit).then(|| Row {
                    completion: Some(Completion {
                        start: parts.command_start.saturating_sub(1),
                        end: parts.command_end,
                        replacement: format!("/{} ", spec.name),
                        submit: false,
                    }),
                    label: format!("/{}", spec.name),
                    aliases: spec.aliases,
                    description: spec.description,
                    positions: name_positions.unwrap_or_default(),
                })
            })
            .collect()
    }

    pub fn update(
        &mut self,
        registry: &CommandRegistry,
        input: &str,
        ctx: &CommandMenuContext<'_>,
    ) {
        self.mode = Self::compute_mode(registry, input, *ctx);
        let len = self.rows().len();
        if self.cursor >= len {
            self.cursor = len.saturating_sub(1);
        }
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if self.cursor + 1 < self.rows().len() {
            self.cursor += 1;
        }
    }

    pub fn selected_completion(&self) -> Option<Completion> {
        self.rows()
            .get(self.cursor)
            .and_then(|row| row.completion.clone())
    }

    pub fn selected_command_completion(&self) -> Option<Completion> {
        matches!(self.mode, Mode::Commands(_))
            .then(|| self.selected_completion())
            .flatten()
    }

    pub fn selected_submit_completion(&self) -> Option<Completion> {
        self.selected_completion()
            .filter(|completion| completion.submit)
    }

    pub fn desired_height(&self) -> u16 {
        let rows = self.rows().len().clamp(1, LIST_MAX);
        u16::try_from(rows).unwrap_or(u16::MAX)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let width = usize::from(area.width);
        let rows = self.rows();
        let visible = usize::from(area.height).max(1);
        let start = if self.cursor >= visible {
            self.cursor + 1 - visible
        } else {
            0
        };
        let lines: Vec<Line> = rows
            .iter()
            .enumerate()
            .skip(start)
            .take(visible)
            .map(|(pos, entry)| render_row(pos == self.cursor, width, entry, theme))
            .collect();
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn rows(&self) -> &[Row] {
        match &self.mode {
            Mode::Commands(rows) | Mode::Context(rows) => rows,
        }
    }
}

fn render_row(selected: bool, width: usize, entry: &Row, theme: Theme) -> Line<'static> {
    let name_style = if selected { theme.key() } else { theme.base() };
    let mut name_spans = Vec::new();
    for (byte_i, ch) in entry.label.char_indices() {
        let style = if entry.positions.contains(&byte_i) {
            name_style.add_modifier(ratatui::style::Modifier::BOLD)
        } else {
            name_style
        };
        name_spans.push(Span::styled(ch.to_string(), style));
    }
    if !entry.aliases.is_empty() {
        name_spans.push(Span::styled(alias_label(&entry.aliases), theme.muted()));
    }
    let desc_style = theme.muted();
    let left_w: usize = name_spans.iter().map(|span| span.content.width()).sum();
    let desc_width = width.saturating_sub(left_w + 6);
    let right = (desc_width > 3 && !entry.description.is_empty()).then(|| {
        Span::styled(
            truncate_to_width(&entry.description, desc_width),
            desc_style,
        )
    });
    selection_row(theme, selected, width, name_spans, right)
}

impl Completion {
    pub fn apply(&self, text: &str) -> String {
        let mut result = text.to_owned();
        if self.start <= self.end
            && self.end <= result.len()
            && result.is_char_boundary(self.start)
            && result.is_char_boundary(self.end)
        {
            result.replace_range(self.start..self.end, &self.replacement);
        }
        result
    }
}

fn slash_parts(input: &str) -> Option<SlashParts<'_>> {
    let leading = input.len() - input.trim_start().len();
    let trimmed = &input[leading..];
    if !trimmed.starts_with('/') {
        return None;
    }
    let body_start = leading + 1;
    let body = &input[body_start..];
    let command_offset = body
        .find(|ch: char| !ch.is_whitespace())
        .unwrap_or(body.len());
    let command_start = body_start + command_offset;
    let after_leading = &input[command_start..];
    let command_len = after_leading
        .find(char::is_whitespace)
        .unwrap_or(after_leading.len());
    let command_end = command_start + command_len;
    let command = &input[command_start..command_end];
    let has_space = command_end < input.len();
    let args_start = if has_space {
        command_end
            + input[command_end..]
                .find(|ch: char| !ch.is_whitespace())
                .unwrap_or(input.len() - command_end)
    } else {
        input.len()
    };
    let args = if has_space { &input[args_start..] } else { "" };
    Some(SlashParts {
        command,
        args,
        command_start,
        command_end,
        args_start,
        has_space,
    })
}

fn context_rows(
    spec: &CommandSpec,
    parts: &SlashParts<'_>,
    ctx: CommandMenuContext<'_>,
) -> Vec<Row> {
    match &spec.shape {
        CommandShape::Empty => vec![hint(spec.usage())],
        CommandShape::Parameters(parameters) => {
            parameter_rows(&spec.name, parameters, parts.args, parts.args_start, ctx)
        }
        CommandShape::Branches(branches) => {
            let first = first_token(parts.args);
            if let Some((token, token_start, token_end)) = first {
                let absolute_start = parts.args_start + token_start;
                let absolute_end = parts.args_start + token_end;
                if let Some(branch) = branches.iter().find(|branch| branch.name == token) {
                    let rest_start = token_end + parts.args[token_end..].len()
                        - parts.args[token_end..].trim_start().len();
                    let rest = &parts.args[rest_start..];
                    return parameter_rows(
                        &spec.name,
                        &branch.parameters,
                        rest,
                        parts.args_start + rest_start,
                        ctx,
                    );
                }
                branch_rows(branches, token, absolute_start, absolute_end)
            } else {
                branch_rows(branches, "", parts.args_start, parts.args_start)
            }
        }
    }
}

fn parameter_rows(
    command: &str,
    parameters: &[ParameterSpec],
    args: &str,
    absolute_start: usize,
    ctx: CommandMenuContext<'_>,
) -> Vec<Row> {
    let tokens = token_spans(args);
    let mut index = 0usize;
    for parameter in parameters {
        if let Some(group) = ctx.lookup(command, &parameter.name) {
            let (query, start, end) = token_slot(args, absolute_start, tokens.get(index).copied());
            return runtime_choice_rows(group, query, start, end);
        }
        match &parameter.value {
            ParameterValue::TextTail => {
                return vec![hint(parameter_label(parameter))];
            }
            ParameterValue::Word | ParameterValue::Integer => {
                if tokens.get(index).is_none() {
                    return vec![hint(parameter_label(parameter))];
                }
                index += 1;
            }
            ParameterValue::Choice(choices) => {
                let (query, start, end) =
                    token_slot(args, absolute_start, tokens.get(index).copied());
                return choice_rows(choices, query, start, end);
            }
        }
    }
    vec![hint("Enter to run")]
}

fn token_slot(
    args: &str,
    absolute_start: usize,
    token: Option<(usize, usize)>,
) -> (&str, usize, usize) {
    match token {
        Some((start, end)) => (
            &args[start..end],
            absolute_start + start,
            absolute_start + end,
        ),
        None => ("", absolute_start + args.len(), absolute_start + args.len()),
    }
}

fn runtime_choice_rows(
    group: &RuntimeChoiceGroup<'_>,
    query: &str,
    start: usize,
    end: usize,
) -> Vec<Row> {
    if group.options.is_empty() {
        return vec![hint(group.empty_hint.to_owned())];
    }
    let rows = matching_runtime_rows(group.options, query, start, end);
    if rows.is_empty() {
        matching_runtime_rows(group.options, "", start, end)
    } else {
        rows
    }
}

fn matching_runtime_rows(
    options: &[RuntimeChoice],
    query: &str,
    start: usize,
    end: usize,
) -> Vec<Row> {
    options
        .iter()
        .filter_map(|option| {
            let positions = subsequence_match(query, &option.label)?;
            Some(Row {
                label: option.label.clone(),
                aliases: Vec::new(),
                description: option.description.clone().unwrap_or_default(),
                positions,
                completion: Some(Completion {
                    start,
                    end,
                    replacement: option.value.clone(),
                    submit: true,
                }),
            })
        })
        .collect()
}

fn branch_rows(branches: &[BranchSpec], query: &str, start: usize, end: usize) -> Vec<Row> {
    branches
        .iter()
        .filter_map(|branch| {
            let positions = subsequence_match(query, &branch.name)?;
            Some(Row {
                label: branch.name.clone(),
                aliases: Vec::new(),
                description: branch.description.clone(),
                positions,
                completion: Some(Completion {
                    start,
                    end,
                    replacement: format!("{} ", branch.name),
                    submit: false,
                }),
            })
        })
        .collect()
}

fn choice_rows(choices: &[ChoiceSpec], query: &str, start: usize, end: usize) -> Vec<Row> {
    choices
        .iter()
        .filter_map(|choice| {
            let positions = subsequence_match(query, &choice.value)?;
            Some(Row {
                label: choice.value.clone(),
                aliases: Vec::new(),
                description: choice.description.clone().unwrap_or_default(),
                positions,
                completion: Some(Completion {
                    start,
                    end,
                    replacement: choice.value.clone(),
                    submit: true,
                }),
            })
        })
        .collect()
}

fn parameter_label(parameter: &ParameterSpec) -> String {
    let body = match parameter.value {
        ParameterValue::Word | ParameterValue::Integer | ParameterValue::Choice(_) => {
            parameter.name.clone()
        }
        ParameterValue::TextTail => format!("{}...", parameter.name),
    };
    let wrapped = if parameter.required {
        format!("<{body}>")
    } else {
        format!("[{body}]")
    };
    format!("{wrapped} {}", parameter.description)
}

fn hint(text: impl Into<String>) -> Row {
    Row {
        label: text.into(),
        aliases: Vec::new(),
        description: String::new(),
        positions: Vec::new(),
        completion: None,
    }
}

fn first_token(args: &str) -> Option<(&str, usize, usize)> {
    let leading = args.len() - args.trim_start().len();
    let rest = &args[leading..];
    if rest.is_empty() {
        return None;
    }
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    Some((&rest[..end], leading, leading + end))
}

fn token_spans(args: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = None;
    for (index, ch) in args.char_indices() {
        if ch.is_whitespace() {
            if let Some(token_start) = start.take() {
                spans.push((token_start, index));
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(token_start) = start {
        spans.push((token_start, args.len()));
    }
    spans
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use super::{CommandMenu, CommandMenuContext, RuntimeChoice, RuntimeChoiceGroup};
    use crate::overlay::truncate_to_width;
    use goat_commands::CommandRegistry;

    fn empty_ctx() -> CommandMenuContext<'static> {
        CommandMenuContext { choices: &[] }
    }

    #[test]
    fn command_completion_replaces_prefix() {
        let registry = CommandRegistry::builtin();
        let menu = CommandMenu::new(&registry, "/eff", &empty_ctx());
        let completion = menu.selected_completion().unwrap();
        assert_eq!(completion.apply("/eff"), "/effort ");
    }

    #[test]
    fn choice_completion_replaces_argument_token() {
        let registry = CommandRegistry::builtin();
        let options = [
            RuntimeChoice {
                value: "low".to_owned(),
                label: "low".to_owned(),
                description: None,
            },
            RuntimeChoice {
                value: "high".to_owned(),
                label: "high".to_owned(),
                description: None,
            },
        ];
        let group = RuntimeChoiceGroup {
            command: "effort",
            parameter: "level",
            options: &options,
            empty_hint: "this model does not support reasoning effort",
        };
        let menu = CommandMenu::new(
            &registry,
            "/effort h",
            &CommandMenuContext { choices: &[group] },
        );
        let completion = menu.selected_submit_completion().unwrap();
        assert_eq!(completion.apply("/effort h"), "/effort high");
    }

    #[test]
    fn truncate_short_text_keeps_text() {
        assert_eq!(truncate_to_width("short", 10), "short");
    }

    #[test]
    fn truncate_long_text_fits_width() {
        let truncated = truncate_to_width("very long skill description", 12);
        assert!(truncated.width() <= 12);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn truncate_zero_width_is_empty() {
        assert_eq!(truncate_to_width("text", 0), "");
    }
}
