//! 介面語言。中文、英文、日文三種。
//!
//! 畫面上的字跟著這裡走；命令列說明、MCP 工具說明、日誌與底層錯誤一律英文，
//! 因為那些是給人除錯和給程式讀的，不隨介面切換。
//!
//! 決定順序是 `--lang`、設定檔的 `lang`、環境變數 `LC_ALL`／`LC_MESSAGES`／`LANG`，
//! 都沒有就用英文。設定檔改了會即時生效，不必重開。

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Zh,
    En,
    Ja,
}

/// 可以寫在 `--lang` 與設定檔裡的名稱。
pub const NAMES: [&str; 3] = ["zh", "en", "ja"];

impl Lang {
    /// 認得語言代碼與常見的地區變體，不認得回 None。
    pub fn parse(name: &str) -> Option<Lang> {
        let name = name.trim().to_ascii_lowercase().replace('_', "-");
        let head = name.split(['.', '@']).next().unwrap_or("");
        let base = head.split('-').next().unwrap_or("");
        match base {
            "zh" | "cht" | "chs" => Some(Lang::Zh),
            "en" => Some(Lang::En),
            "ja" | "jp" => Some(Lang::Ja),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Lang::Zh => "zh",
            Lang::En => "en",
            Lang::Ja => "ja",
        }
    }

    fn code(self) -> u8 {
        match self {
            Lang::Zh => 0,
            Lang::En => 1,
            Lang::Ja => 2,
        }
    }

    fn from_code(code: u8) -> Lang {
        match code {
            0 => Lang::Zh,
            2 => Lang::Ja,
            _ => Lang::En,
        }
    }
}

/// 預設英文：環境沒有交代語言時，英文最可能讀得懂。
static CURRENT: AtomicU8 = AtomicU8::new(1);

pub fn current() -> Lang {
    Lang::from_code(CURRENT.load(Ordering::Relaxed))
}

pub fn set(lang: Lang) {
    CURRENT.store(lang.code(), Ordering::Relaxed);
}

/// 環境變數交代的語言。
pub fn from_env() -> Option<Lang> {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        let Ok(value) = std::env::var(key) else {
            continue;
        };
        if let Some(lang) = Lang::parse(&value) {
            return Some(lang);
        }
    }
    None
}

/// 選一個語言並記下來。命令列優先，再來設定檔，最後環境變數。
pub fn choose(flag: Option<&str>, file: Option<&str>) -> Lang {
    let lang = flag
        .and_then(Lang::parse)
        .or_else(|| file.and_then(Lang::parse))
        .or_else(from_env)
        .unwrap_or(Lang::En);
    set(lang);
    lang
}

/// 三種語言的同一句話，挑現在要用的那一句。
macro_rules! t {
    ($zh:expr, $en:expr, $ja:expr $(,)?) => {
        match $crate::lang::current() {
            $crate::lang::Lang::Zh => $zh,
            $crate::lang::Lang::En => $en,
            $crate::lang::Lang::Ja => $ja,
        }
    };
}

/// 同上，但帶參數，三句各自套用自己的格式。
macro_rules! tf {
    ($zh:literal, $en:literal, $ja:literal $(, $arg:expr)* $(,)?) => {
        match $crate::lang::current() {
            $crate::lang::Lang::Zh => format!($zh $(, $arg)*),
            $crate::lang::Lang::En => format!($en $(, $arg)*),
            $crate::lang::Lang::Ja => format!($ja $(, $arg)*),
        }
    };
}

pub(crate) use {t, tf};

/// 測試會改到全域語言，所以彼此排隊。
#[cfg(test)]
pub static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_usual_spellings() {
        assert_eq!(Lang::parse("zh"), Some(Lang::Zh));
        assert_eq!(Lang::parse("zh_TW.UTF-8"), Some(Lang::Zh));
        assert_eq!(Lang::parse("zh-Hant"), Some(Lang::Zh));
        assert_eq!(Lang::parse("en_US.UTF-8"), Some(Lang::En));
        assert_eq!(Lang::parse("ja_JP.UTF-8"), Some(Lang::Ja));
        assert_eq!(Lang::parse(" JA "), Some(Lang::Ja));
        assert_eq!(Lang::parse("fr_FR"), None);
        assert_eq!(Lang::parse(""), None);
    }

    #[test]
    fn every_listed_name_parses_back_to_itself() {
        for name in NAMES {
            let lang = Lang::parse(name).expect("認得自己列出的名稱");
            assert_eq!(lang.name(), name);
        }
    }

    #[test]
    fn the_flag_wins_over_the_file() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = current();
        assert_eq!(choose(Some("ja"), Some("zh")), Lang::Ja);
        assert_eq!(choose(None, Some("zh")), Lang::Zh);
        assert_eq!(choose(Some("nope"), Some("en")), Lang::En);
        set(before);
    }

    #[test]
    fn picks_the_right_sentence() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = current();
        set(Lang::Zh);
        assert_eq!(t!("中", "en", "日"), "中");
        set(Lang::Ja);
        assert_eq!(t!("中", "en", "日"), "日");
        set(Lang::En);
        assert_eq!(tf!("{}首", "{} tracks", "{}曲", 3), "3 tracks");
        set(before);
    }
}
