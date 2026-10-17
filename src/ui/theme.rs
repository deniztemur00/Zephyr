use iced::theme::Palette;
use iced::widget::{button, container, text};
use iced::{Background, Border, Color, Theme};

use crate::config::ThemeName;

const CARD_RADIUS: f32 = 14.0;
const ROW_RADIUS: f32 = 8.0;

pub struct Swatch {
    pub label: &'static str,
    pub chips: [Color; 3],
    background: Color,
    text: Color,
    accent: Color,
    glow: Color,
    highlight: Color,
    danger: Color,
}

const fn hex(rgb: u32) -> Color {
    Color::from_rgb8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

// Chip colors approximate the Pantone swatches from the user's posters.
const AUTUMN: Swatch = Swatch {
    label: "Autumn Vibes",
    chips: [hex(0x9C4640), hex(0xF3E4CE), hex(0xEBA14E)],
    background: hex(0x241311),
    text: hex(0xF3E4CE),
    accent: hex(0xEBA14E),
    glow: hex(0xF5C27F),
    highlight: hex(0x9C4640),
    danger: hex(0xFF8A70),
};

const LEMONS: Swatch = Swatch {
    label: "Fresh Lemons",
    chips: [hex(0x253A85), hex(0xD8333A), hex(0xFBEFAE)],
    background: hex(0x111A3B),
    text: hex(0xFBEFAE),
    accent: hex(0xE5484D),
    glow: hex(0xFFD86B),
    highlight: hex(0x253A85),
    danger: hex(0xFF8A8A),
};

const BLOOM: Swatch = Swatch {
    label: "Bloom",
    chips: [hex(0xEDD5AE), hex(0x3E1A47), hex(0xE2703A)],
    background: hex(0x1E0F24),
    text: hex(0xEDD5AE),
    accent: hex(0xE2703A),
    glow: hex(0xF29A63),
    highlight: hex(0x5A2A66),
    danger: hex(0xFF8A7A),
};

const FOREST: Swatch = Swatch {
    label: "Forest Walk",
    chips: [hex(0x2F4A37), hex(0xA8B89A), hex(0xD9A93A)],
    background: hex(0x131E18),
    text: hex(0xEFE6CF),
    accent: hex(0xD9A93A),
    glow: hex(0xE9C66B),
    highlight: hex(0x3F5F45),
    danger: hex(0xF07A63),
};

pub fn swatch(name: ThemeName) -> &'static Swatch {
    match name {
        ThemeName::Autumn => &AUTUMN,
        ThemeName::Lemons => &LEMONS,
        ThemeName::Bloom => &BLOOM,
        ThemeName::Forest => &FOREST,
    }
}

pub fn build(name: ThemeName) -> Theme {
    let swatch = swatch(name);
    Theme::custom(
        swatch.label,
        Palette {
            background: swatch.background,
            text: swatch.text,
            primary: swatch.accent,
            success: swatch.glow,
            warning: swatch.highlight,
            danger: swatch.danger,
        },
    )
}

struct Roles {
    background: Color,
    text: Color,
    accent: Color,
    glow: Color,
    highlight: Color,
    danger: Color,
}

impl Roles {
    fn of(theme: &Theme) -> Self {
        let palette = theme.palette();
        Self {
            background: palette.background,
            text: palette.text,
            accent: palette.primary,
            glow: palette.success,
            highlight: palette.warning,
            danger: palette.danger,
        }
    }

    fn tint(&self, amount: f32) -> Color {
        mix(self.background, self.text, amount)
    }

    fn surface(&self) -> Color {
        self.tint(0.05)
    }

    fn raised(&self) -> Color {
        self.tint(0.10)
    }

    fn hover(&self) -> Color {
        self.tint(0.16)
    }

    fn row_alt(&self) -> Color {
        self.tint(0.03)
    }

    fn muted(&self) -> Color {
        mix(self.text, self.background, 0.30)
    }
}

fn mix(from: Color, to: Color, amount: f32) -> Color {
    let blend = |a: f32, b: f32| a + (b - a) * amount;
    Color::from_rgb(
        blend(from.r, to.r),
        blend(from.g, to.g),
        blend(from.b, to.b),
    )
}

pub fn sidebar(theme: &Theme) -> container::Style {
    container::Style::default().background(Roles::of(theme).surface())
}

pub fn now_playing_bar(theme: &Theme) -> container::Style {
    let roles = Roles::of(theme);
    container::Style {
        background: Some(Background::Color(roles.surface())),
        border: Border {
            color: roles.highlight,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn chip(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(color)),
        border: Border::default().rounded(6.0),
        ..container::Style::default()
    }
}

pub fn panel(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Roles::of(theme).raised())),
        border: Border::default().rounded(CARD_RADIUS),
        ..container::Style::default()
    }
}

pub fn placeholder(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Roles::of(theme).hover())),
        border: Border::default().rounded(6.0),
        ..container::Style::default()
    }
}

pub fn muted(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(Roles::of(theme).muted()),
    }
}

pub fn accent(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(Roles::of(theme).accent),
    }
}

pub fn danger(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(Roles::of(theme).danger),
    }
}

pub fn nav(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let roles = Roles::of(theme);
        let (background, text_color) = match (selected, is_hovered(status)) {
            (true, _) => (Some(roles.highlight), roles.text),
            (false, true) => (Some(roles.raised()), roles.text),
            (false, false) => (None, roles.muted()),
        };
        button::Style {
            background: background.map(Background::Color),
            text_color,
            border: Border::default().rounded(ROW_RADIUS),
            ..button::Style::default()
        }
    }
}

pub fn card(theme: &Theme, status: button::Status) -> button::Style {
    card_with(false)(theme, status)
}

pub fn card_with(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let roles = Roles::of(theme);
        let background = match (selected, is_hovered(status)) {
            (true, _) => roles.highlight,
            (false, true) => roles.hover(),
            (false, false) => roles.raised(),
        };
        button::Style {
            background: Some(Background::Color(background)),
            text_color: roles.text,
            border: Border::default().rounded(CARD_RADIUS),
            ..button::Style::default()
        }
    }
}

pub fn track_row(index: usize, playing: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let roles = Roles::of(theme);
        let background = match (playing, is_hovered(status)) {
            (true, _) => roles.highlight.scale_alpha(0.6),
            (false, true) => roles.hover(),
            (false, false) if index % 2 == 1 => roles.row_alt(),
            (false, false) => Color::TRANSPARENT,
        };
        button::Style {
            background: Some(Background::Color(background)),
            text_color: if playing { roles.glow } else { roles.text },
            border: Border::default().rounded(ROW_RADIUS),
            ..button::Style::default()
        }
    }
}

pub fn pill(theme: &Theme, status: button::Status) -> button::Style {
    let roles = Roles::of(theme);
    let (background, text_color) = match status {
        button::Status::Disabled => (roles.raised(), roles.muted()),
        button::Status::Hovered | button::Status::Pressed => (roles.glow, roles.background),
        button::Status::Active => (roles.accent, roles.background),
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color,
        border: Border::default().rounded(18.0),
        ..button::Style::default()
    }
}

pub fn play_button(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        border: Border::default().rounded(26.0),
        ..pill(theme, status)
    }
}

pub fn icon_button(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let roles = Roles::of(theme);
        let text_color = match status {
            button::Status::Disabled => roles.muted().scale_alpha(0.4),
            _ if is_hovered(status) => roles.glow,
            _ if active => roles.accent,
            _ => roles.text,
        };
        button::Style {
            text_color,
            ..button::Style::default()
        }
    }
}

fn is_hovered(status: button::Status) -> bool {
    matches!(status, button::Status::Hovered | button::Status::Pressed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_palette_round_trips_through_the_theme() {
        for name in ThemeName::ALL {
            let roles = Roles::of(&build(name));
            let swatch = swatch(name);
            assert_eq!(roles.accent, swatch.accent);
            assert_eq!(roles.highlight, swatch.highlight);
            assert_ne!(roles.surface(), roles.background);
        }
    }

    #[test]
    fn mix_interpolates_linearly() {
        let black = Color::from_rgb(0.0, 0.0, 0.0);
        let white = Color::from_rgb(1.0, 1.0, 1.0);
        assert_eq!(mix(black, white, 0.25), Color::from_rgb(0.25, 0.25, 0.25));
    }
}
