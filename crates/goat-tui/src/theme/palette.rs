use ratatui::style::Color;

use crate::layout::{METER_HIGH, METER_WARN};

#[derive(Debug, Clone, Copy)]
pub struct CodePalette {
    pub bg: Color,
    pub keyword: Color,
    pub string: Color,
    pub comment: Color,
    pub number: Color,
    pub type_: Color,
    pub function: Color,
    pub variable: Color,
    pub operator: Color,
    pub macro_: Color,
    pub property: Color,
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub id: u8,
    pub bg: Color,
    pub fg: Color,
    pub dark: bool,
    pub user: Color,
    pub user_panel: Color,
    pub agent: Color,
    pub tool: Color,
    pub error: Color,
    pub muted: Color,
    pub accent: Color,
    pub success: Color,
    pub border: Color,
    pub border_dim: Color,
    pub panel: Color,
    pub shell: Color,
    pub shell_dim: Color,
    pub selection: Color,
    pub code: CodePalette,
}

impl Palette {
    pub const fn dark() -> Self {
        Self {
            id: 1,
            bg: Color::Reset,
            dark: true,
            fg: Color::Rgb(0xd7, 0xda, 0xe0),
            user: Color::Rgb(0x7d, 0x9b, 0xd4),
            user_panel: Color::Rgb(0x20, 0x20, 0x20),
            agent: Color::Rgb(0x6f, 0xb3, 0xa8),
            tool: Color::Rgb(0xcf, 0x9b, 0x6b),
            error: Color::Rgb(0xc9, 0x7a, 0x7a),
            muted: Color::Rgb(0x7a, 0x80, 0x8c),
            accent: Color::Rgb(0xa9, 0x8f, 0xd0),
            success: Color::Rgb(0x8f, 0xb9, 0x8a),
            border: Color::Rgb(0x2a, 0x2c, 0x32),
            border_dim: Color::Rgb(0x22, 0x24, 0x29),
            panel: Color::Rgb(0x1b, 0x1b, 0x1e),
            shell: Color::Rgb(0xdb, 0x4b, 0x4b),
            shell_dim: Color::Rgb(0x54, 0x29, 0x2e),
            selection: Color::Rgb(0x2d, 0x3c, 0x52),
            code: CodePalette {
                bg: Color::Reset,
                keyword: Color::Rgb(0x56, 0x9c, 0xd6),
                string: Color::Rgb(0xce, 0x91, 0x78),
                comment: Color::Rgb(0x6a, 0x99, 0x55),
                number: Color::Rgb(0xb5, 0xce, 0xa8),
                type_: Color::Rgb(0x4e, 0xc9, 0xb0),
                function: Color::Rgb(0xdc, 0xdc, 0xaa),
                variable: Color::Rgb(0x9c, 0xda, 0xfe),
                operator: Color::Rgb(0xd4, 0xd4, 0xd4),
                macro_: Color::Rgb(0xc5, 0x86, 0xc0),
                property: Color::Rgb(0x9c, 0xda, 0xfe),
            },
        }
    }

    pub const fn light() -> Self {
        Self {
            id: 2,
            bg: Color::Reset,
            dark: false,
            fg: Color::Rgb(0x12, 0x14, 0x18),
            user: Color::Rgb(0x1a, 0x56, 0xd6),
            user_panel: Color::Rgb(0xec, 0xed, 0xf0),
            agent: Color::Rgb(0x1b, 0x6e, 0x3c),
            tool: Color::Rgb(0x9a, 0x5a, 0x00),
            error: Color::Rgb(0xb4, 0x1c, 0x1c),
            muted: Color::Rgb(0x5c, 0x63, 0x6e),
            accent: Color::Rgb(0x5b, 0x21, 0xb6),
            success: Color::Rgb(0x1b, 0x6e, 0x3c),
            border: Color::Rgb(0xc8, 0xcc, 0xd4),
            border_dim: Color::Rgb(0xdd, 0xe0, 0xe6),
            panel: Color::Rgb(0xf0, 0xf1, 0xf4),
            shell: Color::Rgb(0xa3, 0x15, 0x45),
            shell_dim: Color::Rgb(0xf0, 0xd4, 0xdc),
            selection: Color::Rgb(0xc7, 0xdd, 0xf5),
            code: CodePalette {
                bg: Color::Reset,
                keyword: Color::Rgb(0x00, 0x3d, 0xb8),
                string: Color::Rgb(0x8b, 0x12, 0x12),
                comment: Color::Rgb(0x0d, 0x6b, 0x0d),
                number: Color::Rgb(0x0a, 0x6b, 0x47),
                type_: Color::Rgb(0x1a, 0x6b, 0x85),
                function: Color::Rgb(0x5c, 0x4a, 0x1a),
                variable: Color::Rgb(0x00, 0x1a, 0x72),
                operator: Color::Rgb(0x2a, 0x2a, 0x2a),
                macro_: Color::Rgb(0x8b, 0x00, 0x9e),
                property: Color::Rgb(0x00, 0x1a, 0x72),
            },
        }
    }
}

fn mix(base: Color, tint: Color, t: f32) -> Color {
    let (Color::Rgb(r0, g0, b0), Color::Rgb(r1, g1, b1)) = (base, tint) else {
        return base;
    };
    let lerp = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Color::Rgb(lerp(r0, r1), lerp(g0, g1), lerp(b0, b1))
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    palette: Palette,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    pub const fn dark() -> Self {
        Self {
            palette: Palette::dark(),
        }
    }

    pub const fn light() -> Self {
        Self {
            palette: Palette::light(),
        }
    }

    #[must_use]
    pub fn with_base(self, base: Option<Color>) -> Self {
        let Some(base) = base else {
            return self;
        };
        let mut palette = self.palette;
        let fg = palette.fg;
        palette.panel = mix(base, fg, 0.12);
        palette.user_panel = mix(base, fg, 0.15);
        palette.border_dim = mix(base, fg, 0.17);
        palette.border = mix(base, fg, 0.20);
        Self { palette }
    }

    pub fn base(self) -> ratatui::style::Style {
        let p = self.palette;
        ratatui::style::Style::new().fg(p.fg).bg(p.bg)
    }

    pub fn text(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.fg)
    }

    pub fn muted(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.muted)
    }

    pub fn key(self) -> ratatui::style::Style {
        ratatui::style::Style::new()
            .fg(self.palette.fg)
            .add_modifier(ratatui::style::Modifier::BOLD)
    }

    pub fn accent(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.accent)
    }

    pub fn selection(self) -> ratatui::style::Style {
        ratatui::style::Style::new().bg(self.palette.selection)
    }

    pub fn border(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.border)
    }

    pub fn border_dim(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.border_dim)
    }

    pub fn shell(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.shell)
    }

    pub fn shell_dim(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.shell_dim)
    }

    pub fn role_user(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.user)
    }

    pub fn user_panel(self) -> ratatui::style::Style {
        ratatui::style::Style::new().bg(self.palette.user_panel)
    }

    pub fn role_agent(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.agent)
    }

    pub fn role_tool(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.tool)
    }

    pub fn tool_fn(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.accent)
    }

    pub fn tool_arg_value(self) -> ratatui::style::Style {
        self.muted()
    }

    pub fn error(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.error)
    }

    pub fn success(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.success)
    }

    pub fn error_body(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.error)
    }

    pub fn meter(self, pct: f32) -> ratatui::style::Style {
        if pct >= METER_HIGH {
            self.error()
        } else if pct >= METER_WARN {
            self.role_tool()
        } else {
            self.muted()
        }
    }

    pub fn code_plain(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.fg)
    }

    pub fn inline_code(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.accent)
    }

    pub fn fg_color(self) -> Color {
        self.palette.fg
    }

    pub fn accent_color(self) -> Color {
        self.palette.accent
    }

    pub fn hint_key(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.fg)
    }

    pub fn panel(self) -> ratatui::style::Style {
        let p = self.palette;
        ratatui::style::Style::new().fg(p.fg).bg(p.panel)
    }

    pub fn is_dark(self) -> bool {
        self.palette.dark
    }

    pub fn id(self) -> u8 {
        self.palette.id
    }

    pub fn code(self) -> CodePalette {
        self.palette.code
    }
}
