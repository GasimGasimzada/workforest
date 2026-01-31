use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const APP_NAME: &str = "workforest";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub name: String,
    pub path: PathBuf,
    pub tools: Vec<String>,
    pub default_tool: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RepoConfigFile {
    #[serde(default)]
    pub repos: Vec<RepoConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TerminalSnapshot {
    pub alt_screen: bool,
    pub mouse_tracking: bool,
    pub mouse_button_tracking: bool,
    pub mouse_any_event: bool,
    pub mouse_sgr: bool,
    pub cursor_visible: bool,
    pub cursor_shape: CursorShape,
    pub origin_mode: bool,
    pub wrap_mode: bool,
    pub insert_mode: bool,
    pub scroll_region: Option<ScrollRegion>,
    pub attributes: TerminalAttributes,
    pub saved_cursor_main: Option<CursorPosition>,
    pub saved_cursor_alt: Option<CursorPosition>,
    pub dec_private_modes: Vec<ModeEntry>,
    pub terminal_modes: Vec<ModeEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TerminalAttributes {
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    pub intensity: TerminalIntensity,
    pub underline: TerminalUnderline,
    pub blink: TerminalBlink,
    pub inverse: bool,
    pub italic: bool,
    pub hidden: bool,
    pub strikethrough: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScrollRegion {
    pub top: usize,
    pub bottom: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CursorPosition {
    pub x: usize,
    pub y: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModeEntry {
    pub code: u16,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum CursorShape {
    #[default]
    Default,
    BlinkingBlock,
    SteadyBlock,
    BlinkingUnderline,
    SteadyUnderline,
    BlinkingBar,
    SteadyBar,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum TerminalColor {
    #[default]
    Default,
    Ansi(u8),
    Rgb {
        r: u8,
        g: u8,
        b: u8,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum TerminalIntensity {
    #[default]
    Normal,
    Bold,
    Faint,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum TerminalUnderline {
    #[default]
    None,
    Single,
    Double,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum TerminalBlink {
    #[default]
    None,
    Slow,
    Rapid,
}

pub fn config_dir() -> PathBuf {
    let project_dirs =
        directories::ProjectDirs::from("", "", APP_NAME).expect("project dirs available");
    project_dirs.config_dir().to_path_buf()
}

pub fn data_dir() -> PathBuf {
    let project_dirs =
        directories::ProjectDirs::from("", "", APP_NAME).expect("project dirs available");
    project_dirs.data_dir().to_path_buf()
}

pub fn repos_config_path() -> PathBuf {
    config_dir().join("repos.toml")
}
