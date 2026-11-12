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
    /// 終端機主題檔：路徑，或放在已知目錄裡的名稱。
    palette: Option<String>,
    #[serde(default)]
    colors: HashMap<String, String>,
}

/// 從終端機主題檔讀出來的調色盤。
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub background: Color,
    pub foreground: Color,
    pub selection: Option<Color>,
    pub ansi: [Color; 16],
}

/// Herdr 的主題檔：background、foreground、selection 與 16 色 ansi 陣列。
#[derive(Deserialize)]
struct JsonPalette {
    background: String,
    foreground: String,
    selection: Option<String>,
    ansi: Vec<String>,
}

/// 找出主題檔。接受路徑，或是放在終端機主題目錄裡的名稱。
fn resolve_palette(name: &str) -> Option<PathBuf> {
    let expanded = if let Some(rest) = name.strip_prefix("~/") {
        dirs::home_dir()?.join(rest)
    } else {
        PathBuf::from(name)
    };
    if expanded.is_file() {
        return Some(expanded);
    }

    let home = dirs::home_dir()?;
    let candidates = [
        crate::store::config_dir().join("themes").join(name),
        crate::store::config_dir()
            .join("themes")
            .join(format!("{name}.json")),
        home.join(".config/herdr-app/themes")
            .join(format!("{name}.json")),
        home.join(".config/ghostty/themes").join(name),
        PathBuf::from("/Applications/Ghostty.app/Contents/Resources/ghostty/themes").join(name),
    ];
    candidates.into_iter().find(|path| path.is_file())
}

/// 讀主題檔。先當 JSON 解析，不成就當 ghostty 的 `鍵 = 值` 設定檔。
fn read_palette(path: &std::path::Path) -> Option<Palette> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_palette(&text)
}

fn parse_palette(text: &str) -> Option<Palette> {
    if let Ok(json) = serde_json::from_str::<JsonPalette>(text) {
        if json.ansi.len() < 16 {
            log::warn!("主題檔的 ansi 只有 {} 色，需要 16 色", json.ansi.len());
            return None;
        }
        let mut ansi = [Color::Reset; 16];
        for (slot, value) in ansi.iter_mut().zip(json.ansi.iter()) {
            *slot = parse_color(value)?;
        }
        return Some(Palette {
            background: parse_color(&json.background)?,
            foreground: parse_color(&json.foreground)?,
            selection: json.selection.as_deref().and_then(parse_color),
            ansi,
        });
    }

    // ghostty 格式：`palette = 4=#61afef`、`background = 282c34`，`#` 開頭是註解。
    let mut ansi = [None; 16];
    let mut background = None;
    let mut foreground = None;
    let mut selection = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let hex = |v: &str| parse_color(&with_hash(v));
        match key {
            "background" => background = hex(value),
            "foreground" => foreground = hex(value),
            "selection-background" => selection = hex(value),
            "palette" => {
                if let Some((index, colour)) = value.split_once('=')
                    && let Ok(index) = index.trim().parse::<usize>()
                    && index < 16
                {
                    ansi[index] = hex(colour.trim());
                }
            }
            _ => {}
        }
    }

    let filled: Vec<Color> = ansi.iter().flatten().copied().collect();
    if filled.len() < 16 {
        return None;
    }
    let mut slots = [Color::Reset; 16];
    for (slot, colour) in slots.iter_mut().zip(filled) {
        *slot = colour;
    }
    Some(Palette {
        background: background?,
        foreground: foreground?,
        selection,
        ansi: slots,
    })
}

/// ghostty 的色值可以省略 `#`，補回來再交給共用的解析。
fn with_hash(value: &str) -> String {
    let value = value.trim();
    if value.starts_with('#') || value.len() != 6 {
        value.to_string()
    } else {
        format!("#{value}")
    }
}

/// 把調色盤攤在主題上：文字、次要色、強調色都從終端機主題取。
fn apply_palette(theme: &mut Theme, palette: &Palette) {
    theme.text = palette.foreground;
    theme.muted = palette.ansi[7];
    theme.dim = palette.ansi[8];
    theme.accent = palette.ansi[1];
    theme.on_accent = palette.background;
    theme.selection_bg = palette.selection.unwrap_or(palette.ansi[1]);
    theme.selection_fg = palette.background;
    theme.gauge_track = palette.ansi[0];
}

pub fn path() -> PathBuf {
    crate::store::config_dir().join("theme.json")
}

/// 角色的色值，除了一般寫法外還能引用載入的調色盤：`ansi:4`、`fg`、`bg`、`selection`。
fn parse_role_color(text: &str, palette: Option<&Palette>) -> Option<Color> {
    let text = text.trim();
    if let Some(palette) = palette {
        if let Some(index) = text.strip_prefix("ansi:") {
            let index: usize = index.trim().parse().ok()?;
            return palette.ansi.get(index).copied();
        }
        match text {
            "fg" | "foreground" => return Some(palette.foreground),
            "bg" | "background" => return Some(palette.background),
            "selection" => return palette.selection.or(Some(palette.ansi[1])),
            _ => {}
        }
    }
    parse_color(text)
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

    // 終端機主題檔先鋪底，個別角色的設定再疊上去。
    let palette = file.palette.as_deref().and_then(|name| {
        let Some(path) = resolve_palette(name) else {
            log::warn!("找不到終端機主題檔：{name}");
            return None;
        };
        let found = read_palette(&path);
        if found.is_none() {
            log::warn!("終端機主題檔看不懂：{}", path.display());
        }
        found
    });
    if let Some(palette) = palette.as_ref() {
        apply_palette(&mut theme, palette);
    }

    if let Some(border) = file.border {
        theme.border = border;
    }
    for (role, value) in &file.colors {
        let Some(color) = parse_role_color(value, palette.as_ref()) else {
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

    const GHOSTTY: &str = "\
# Atom One Dark
background = 282c34
foreground = abb2bf
selection-background = 3e4451
palette = 0=#3f4451
palette = 1=#e05561
palette = 2=#8cc265
palette = 3=#d18f52
palette = 4=#4aa5f0
palette = 5=#c162de
palette = 6=#42b3c2
palette = 7=#e6e6e6
palette = 8=#4f5666
palette = 9=#ff616e
palette = 10=#a5e075
palette = 11=#f0a45d
palette = 12=#4dc4ff
palette = 13=#de73ff
palette = 14=#4cd1e0
palette = 15=#d7dae0
";

    #[test]
    fn reads_a_ghostty_theme_file() {
        let palette = parse_palette(GHOSTTY).expect("解析 ghostty 主題");
        assert_eq!(palette.background, Color::Rgb(0x28, 0x2c, 0x34));
        assert_eq!(palette.foreground, Color::Rgb(0xab, 0xb2, 0xbf));
        assert_eq!(palette.selection, Some(Color::Rgb(0x3e, 0x44, 0x51)));
        assert_eq!(palette.ansi[4], Color::Rgb(0x4a, 0xa5, 0xf0));
        assert_eq!(palette.ansi[15], Color::Rgb(0xd7, 0xda, 0xe0));
    }

    #[test]
    fn reads_a_terminal_theme_in_json() {
        let json = r##"{
            "background": "#21252b",
            "foreground": "#abb2bf",
            "selection": "#323844",
            "ansi": ["#454c59", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd",
                     "#56b6c2", "#abb2bf", "#767676", "#e06c75", "#98c379", "#e5c07b",
                     "#61afef", "#c678dd", "#56b6c2", "#abb2bf"]
        }"##;
        let palette = parse_palette(json).expect("解析 JSON 主題");
        assert_eq!(palette.background, Color::Rgb(0x21, 0x25, 0x2b));
        assert_eq!(palette.ansi[1], Color::Rgb(0xe0, 0x6c, 0x75));
    }

    #[test]
    fn a_palette_fills_the_roles_and_overrides_still_win() {
        let _guard = crate::store::CONFIG_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("建立暫存目錄");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        std::fs::create_dir_all(crate::store::config_dir()).expect("建立設定目錄");

        let theme_file = crate::store::config_dir().join("one-dark.conf");
        std::fs::write(&theme_file, GHOSTTY).expect("寫入終端機主題");
        std::fs::write(
            path(),
            format!(
                r##"{{ "palette": "{}", "colors": {{ "accent": "ansi:4", "gauge_track": "#101010" }} }}"##,
                theme_file.display()
            ),
        )
        .expect("寫入主題檔");

        let theme = load(None);
        // 角色從調色盤填滿。
        assert_eq!(theme.text, Color::Rgb(0xab, 0xb2, 0xbf));
        assert_eq!(theme.dim, Color::Rgb(0x4f, 0x56, 0x66));
        assert_eq!(theme.selection_fg, Color::Rgb(0x28, 0x2c, 0x34));
        // 明寫的角色蓋過調色盤。
        assert_eq!(theme.accent, Color::Rgb(0x4a, 0xa5, 0xf0));
        assert_eq!(theme.gauge_track, Color::Rgb(0x10, 0x10, 0x10));
    }

    #[test]
    fn an_unreadable_palette_leaves_the_preset_alone() {
        let _guard = crate::store::CONFIG_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("建立暫存目錄");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        std::fs::create_dir_all(crate::store::config_dir()).expect("建立設定目錄");
        std::fs::write(path(), r#"{ "palette": "沒有這個檔案" }"#).expect("寫入主題檔");

        assert_eq!(load(None).accent, preset("ember").accent);
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
