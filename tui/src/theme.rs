use ratatui::style::Color;

pub const ICON_IDLE: &str = "󰒲";
pub const ICON_ERROR: &str = "󰅚";
pub const ICON_ACTIVE: &str = "●";

pub const THEME: Theme = Theme {
    bg: Color::Rgb(12, 12, 14),
    bg_alt: Color::Rgb(17, 17, 20),
    bg_alt2: Color::Rgb(22, 22, 27),
    fg: Color::Rgb(255, 255, 255),
    fg_mid: Color::Rgb(184, 184, 184),
    fg_dim: Color::Rgb(107, 107, 107),
    green: Color::Rgb(95, 255, 135),
    green_dim: Color::Rgb(63, 166, 106),
    orange: Color::Rgb(255, 175, 95),
    orange_dim: Color::Rgb(201, 138, 68),
    yellow: Color::Rgb(255, 215, 95),
    yellow_dim: Color::Rgb(230, 193, 90),
    blue: Color::Rgb(95, 175, 255),
    magenta: Color::Rgb(215, 135, 255),
    red: Color::Rgb(255, 95, 95),
    border: Color::Rgb(26, 26, 31),
    visual: Color::Rgb(42, 42, 42),
};

#[allow(dead_code)]
pub struct Theme {
    pub bg: Color,
    pub bg_alt: Color,
    pub bg_alt2: Color,
    pub fg: Color,
    pub fg_mid: Color,
    pub fg_dim: Color,
    pub green: Color,
    pub green_dim: Color,
    pub orange: Color,
    pub orange_dim: Color,
    pub yellow: Color,
    pub yellow_dim: Color,
    pub blue: Color,
    pub magenta: Color,
    pub red: Color,
    pub border: Color,
    pub visual: Color,
}
