//! 配色與框線樣式。預設和內建主題一致，有設定檔時以設定檔為準。
//!
//! 設定檔是 `~/.config/harioto/theme.json`，可以只寫想改的那幾個角色：
//!
//! ```json
//! { "preset": "mono", "border": "none", "colors": { "accent": "#7aa2f7" } }
//! ```
//!
//! 存檔之後畫面會在 0.25 秒內跟著變，不必重開播放器。

use ratatui::style::Color;
use ratatui::widgets::BorderType;
use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf, time::SystemTime};

/// 內建主題的名稱，給 `--theme` 與設定檔的 `preset` 用。
pub const PRESETS: [&str; 3] = ["ember", "mono", "light"];

/// 框線樣式。`none` 會連框帶標題一起拿掉，換來四行可用高度。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Border {
    Rounded,
    Plain,
    Thick,
    None,
}

impl Border {
    pub fn kind(self) -> Option<BorderType> {
        match self {
            Border::Rounded => Some(BorderType::Rounded),
            Border::Plain => Some(BorderType::Plain),
            Border::Thick => Some(BorderType::Thick),
            Border::None => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    /// 焦點邊框、面板標題、播放狀態、進度條、狀態列底色。
    pub accent: Color,
    /// 壓在強調色上的文字。
    pub on_accent: Color,
    /// 主要文字：標題與輸入內容。
    pub text: Color,
    /// 次要資訊：頻道、時長、提示列。
    pub muted: Color,
    /// 更弱的一層：未聚焦的邊框與提示文字。
    pub dim: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
    /// 進度條未播到的部分。
    pub gauge_track: Color,
    pub border: Border,
}

impl Default for Theme {
    fn default() -> Self {
        preset("ember")
    }
}

/// 內建主題。名稱不認得時回到 `ember`。
pub fn preset(name: &str) -> Theme {
    match name {
        "mono" => Theme {
            accent: Color::Reset,
            on_accent: Color::Reset,
            text: Color::Reset,
            muted: Color::Gray,
            dim: Color::DarkGray,
            selection_bg: Color::Gray,
            selection_fg: Color::Black,
            gauge_track: Color::DarkGray,
            border: Border::Plain,
        },
        "light" => Theme {
            accent: Color::Rgb(176, 48, 48),
            on_accent: Color::Rgb(250, 250, 250),
            text: Color::Black,
            muted: Color::Rgb(90, 90, 90),
            dim: Color::Rgb(140, 140, 140),
            selection_bg: Color::Rgb(176, 48, 48),
            selection_fg: Color::Rgb(250, 250, 250),
            gauge_track: Color::Rgb(215, 215, 215),
            border: Border::Rounded,
        },
        _ => Theme {
            accent: Color::Rgb(255, 72, 72),
            on_accent: Color::Black,
            text: Color::White,
            muted: Color::Gray,
            dim: Color::DarkGray,
            selection_bg: Color::Rgb(255, 72, 72),
            selection_fg: Color::Black,
            gauge_track: Color::Rgb(40, 40, 40),
            border: Border::Rounded,
        },
    }
}

/// 設定檔的形狀。每個欄位都可以省略。
#[derive(Default, Deserialize)]
struct File {
    preset: Option<String>,
    border: Option<Border>,
    #[serde(default)]
    colors: HashMap<String, String>,
}

pub fn path() -> PathBuf {
    crate::store::config_dir().join("theme.json")
}

/// 解析色值：`#rrggbb`、16 色名稱、256 色索引、`terminal`（繼承終端機）。
fn parse_color(text: &str) -> Option<Color> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
        return None;
    }
    if let Ok(index) = text.parse::<u8>() {
        return Some(Color::Indexed(index));
    }
    Some(match text.to_ascii_lowercase().as_str() {
        "terminal" | "default" | "reset" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" | "white" => Color::Gray,
        "bright_black" | "dark_gray" | "dark_grey" => Color::DarkGray,
        "bright_red" => Color::LightRed,
        "bright_green" => Color::LightGreen,
        "bright_yellow" => Color::LightYellow,
        "bright_blue" => Color::LightBlue,
        "bright_magenta" => Color::LightMagenta,
        "bright_cyan" => Color::LightCyan,
        "bright_white" => Color::White,
        _ => return None,
    })
}

/// 讀出主題。`override_preset` 來自 `--theme`，比設定檔的 `preset` 優先。
pub fn load(override_preset: Option<&str>) -> Theme {
    let file: File = std::fs::read_to_string(path())
        .ok()
        .and_then(|text| match serde_json::from_str(&text) {
            Ok(parsed) => Some(parsed),
            Err(e) => {
                log::warn!("主題檔讀不懂，先用預設：{e}");
                None
            }
        })
        .unwrap_or_default();

    let base = override_preset
        .or(file.preset.as_deref())
        .map(preset)
        .unwrap_or_default();

    let mut theme = base;
    if let Some(border) = file.border {
        theme.border = border;
    }
    for (role, value) in &file.colors {
        let Some(color) = parse_color(value) else {
            log::warn!("主題的 {role} 寫了看不懂的色值：{value}");
            continue;
        };
        match role.as_str() {
            "accent" => theme.accent = color,
            "on_accent" => theme.on_accent = color,
            "text" => theme.text = color,
            "muted" => theme.muted = color,
            "dim" => theme.dim = color,
            "selection_bg" => theme.selection_bg = color,
            "selection_fg" => theme.selection_fg = color,
            "gauge_track" => theme.gauge_track = color,
            other => log::warn!("主題裡有不認得的角色：{other}"),
        }
    }

    // 強調色換了但選取列沒指定時，讓選取列跟著走。
    if file.colors.contains_key("accent") && !file.colors.contains_key("selection_bg") {
        theme.selection_bg = theme.accent;
    }

    theme
}

/// 主題檔的修改時間，用來判斷要不要重讀。
pub fn mtime() -> Option<SystemTime> {
    std::fs::metadata(path()).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_accepted_colour_form() {
        assert_eq!(parse_color("#ff4848"), Some(Color::Rgb(255, 72, 72)));
        assert_eq!(parse_color("244"), Some(Color::Indexed(244)));
        assert_eq!(parse_color("bright_blue"), Some(Color::LightBlue));
        assert_eq!(parse_color("terminal"), Some(Color::Reset));
        assert_eq!(parse_color("#ff48"), None);
        assert_eq!(parse_color("puce"), None);
    }

    #[test]
    fn file_overrides_sit_on_top_of_the_preset() {
        let _guard = crate::store::CONFIG_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("建立暫存目錄");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        std::fs::create_dir_all(crate::store::config_dir()).expect("建立設定目錄");
        std::fs::write(
            path(),
            r##"{ "preset": "light", "border": "none", "colors": { "accent": "#7aa2f7" } }"##,
        )
        .expect("寫入主題檔");

        let theme = load(None);
        assert_eq!(theme.accent, Color::Rgb(122, 162, 247));
        // 沒指定選取列，所以跟著強調色走。
        assert_eq!(theme.selection_bg, theme.accent);
        // 其餘維持 light 的值。
        assert_eq!(theme.text, Color::Black);
        assert_eq!(theme.border, Border::None);
    }

    #[test]
    fn command_line_preset_wins_over_the_file() {
        let _guard = crate::store::CONFIG_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("建立暫存目錄");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        std::fs::create_dir_all(crate::store::config_dir()).expect("建立設定目錄");
        std::fs::write(path(), r#"{ "preset": "light" }"#).expect("寫入主題檔");

        assert_eq!(load(Some("mono")).text, Color::Reset);
    }

    #[test]
    fn a_broken_file_falls_back_to_the_default() {
        let _guard = crate::store::CONFIG_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("建立暫存目錄");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        std::fs::create_dir_all(crate::store::config_dir()).expect("建立設定目錄");
        std::fs::write(path(), "{ 這不是 JSON").expect("寫入主題檔");

        assert_eq!(load(None).accent, preset("ember").accent);
    }
}
