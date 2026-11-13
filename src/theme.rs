use std::sync::LazyLock;

use rune::{Color, Family, Span, TextStyle, oklch};
use rune_kit::Kit;

use crate::highlight::Token;

pub const FAMILIES: (&str, &str) = ("Inter", "ui-monospace, Menlo");

pub const RADIUS: f32 = 6.0;

pub struct Theme {
    pub k: Kit,
    pub modal: Kit,
    pub modal_border: Color,
    pub dark: bool,
    pub background: Color,
    pub foreground: Color,
    pub muted: Color,
    pub faint: Color,
    pub border: Color,
    pub ring: Color,
    pub secondary: Color,
    pub sidebar: Color,
    pub popover: Color,
    pub list_active: Color,
    pub list_active_border: Color,
    pub list_hover: Color,
    pub table_hover: Color,
    pub table_active: Color,
    pub table_stripe: Color,
    pub status_bar: Color,
    pub status_bar_border: Color,
    pub danger: Color,
    pub warning: Color,
    pub success: Color,
    pub blue: Color,
    pub yellow: Color,
    pub cyan: Color,
    pub green: Color,
    pub magenta: Color,
    pub text: TextStyle,
    pub text_sm: TextStyle,
    pub text_xs: TextStyle,
    pub mono: TextStyle,
    pub mono_sm: TextStyle,
}

static LIGHT: LazyLock<Theme> = LazyLock::new(|| Theme::new(false));
static DARK: LazyLock<Theme> = LazyLock::new(|| Theme::new(true));

pub fn theme(dark: bool) -> &'static Theme {
    if dark { &DARK } else { &LIGHT }
}

fn modal_kit(k: &Kit, border: Color) -> Kit {
    let mut modal = k.clone();
    modal.c.layer_01 = k.c.bg_base;
    for shadow in &mut modal.elevation.overlay {
        if shadow.blur == 0.0 {
            shadow.color = border;
            shadow.spread = 1.0;
        }
    }
    modal
}

fn hue(dark: bool, h: f32) -> Color {
    if dark {
        oklch(0.76, 0.13, h)
    } else {
        oklch(0.52, 0.16, h)
    }
}

impl Theme {
    pub fn new(dark: bool) -> Self {
        let k = Kit::neutral(dark);
        let c = &k.c;
        let selection = k.selection();
        let modal_border = if dark {
            k.alpha(c.text_base, 0.14)
        } else {
            c.border_base
        };
        let modal = modal_kit(&k, modal_border);
        Theme {
            dark,
            modal,
            modal_border,
            background: c.bg_base,
            foreground: c.text_base,
            muted: c.text_muted,
            faint: c.text_faint,
            border: c.border_base,
            ring: c.border_focus,
            secondary: c.layer_02,
            sidebar: c.layer_01,
            popover: c.surface_float,
            list_active: selection,
            list_active_border: c.border_focus,
            list_hover: c.hover,
            table_hover: c.hover,
            table_active: selection,
            table_stripe: k.alpha(c.text_base, 0.03),
            status_bar: c.layer_01,
            status_bar_border: c.border_weak,
            danger: c.danger.fg,
            warning: c.warning.fg,
            success: c.success.fg,
            blue: hue(dark, 250.0),
            yellow: hue(dark, 85.0),
            cyan: hue(dark, 200.0),
            green: hue(dark, 145.0),
            magenta: hue(dark, 330.0),
            text: TextStyle::new(13.0, 400.0).lh(18.0),
            text_sm: TextStyle::new(12.0, 400.0).lh(16.0),
            text_xs: TextStyle::new(11.0, 400.0).lh(14.0),
            mono: TextStyle::new(12.0, 400.0).family(Family::Mono).lh(18.0),
            mono_sm: TextStyle::new(11.0, 400.0).family(Family::Mono).lh(16.0),
            k,
        }
    }

    pub fn bold(&self, style: TextStyle) -> TextStyle {
        TextStyle {
            weight: 700.0,
            ..style
        }
    }

    pub fn semibold(&self, style: TextStyle) -> TextStyle {
        TextStyle {
            weight: 600.0,
            ..style
        }
    }

    pub fn medium(&self, style: TextStyle) -> TextStyle {
        TextStyle {
            weight: 500.0,
            ..style
        }
    }

    pub fn token_color(&self, token: Token) -> Option<Color> {
        match token {
            Token::Plain => None,
            Token::Keyword => Some(self.blue),
            Token::Type => Some(self.yellow),
            Token::Function => Some(self.cyan),
            Token::String => Some(self.green),
            Token::Number => Some(self.magenta),
            Token::Comment => Some(self.muted),
            Token::Identifier => Some(self.cyan),
        }
    }

    pub fn token_span(&self, token: Token, start: usize, end: usize) -> Option<Span> {
        let color = self.token_color(token)?;
        let span = Span::new(start, end).color(color);
        Some(match token {
            Token::Keyword => span.bold(),
            Token::Comment => span.italic(),
            _ => span,
        })
    }
}
