//! 版本檢查：問 crates.io 有沒有比手上這份新的 harioto。
//!
//! 一天最多問一次，答案存在設定目錄裡，所以一天重開十次播放器只會有一次請求。
//! 走 `curl` 而不是在程式裡內建 HTTP 客戶端，理由和縮圖那邊一樣：一次 JSON
//! 請求不值得把整套 TLS 與 HTTP/2 連進執行檔。

use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

/// 兩次檢查之間至少隔這麼久。
const EVERY: u64 = 24 * 60 * 60;

/// crates.io 上這個套件的位置。
const ENDPOINT: &str = "https://crates.io/api/v1/crates/harioto";

/// 這份執行檔的版本。
pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn path() -> PathBuf {
    crate::store::config_dir().join("update.json")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 上一次問到的答案。存起來是為了不要每次啟動都連外。
#[derive(Serialize, Deserialize)]
struct Memo {
    /// 問的時間，自 1970-01-01 起算的秒數。
    checked_at: u64,
    /// crates.io 當時說的最新穩定版。
    latest: String,
}

fn read_memo() -> Option<Memo> {
    let text = std::fs::read_to_string(path()).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_memo(memo: &Memo) {
    let path = path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(memo) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                log::warn!("could not remember the version check: {e}");
            }
        }
        Err(e) => log::warn!("could not write down the version check: {e}"),
    }
}

/// 把 `1.2.3` 拆成三個數字。修訂號後面接 `-rc.1` 這類字尾時只取前面的數字。
fn parts(version: &str) -> Option<(u32, u32, u32)> {
    let mut fields = version.trim().split('.');
    let major = fields.next()?.parse().ok()?;
    let minor = fields.next()?.parse().ok()?;
    let patch = fields
        .next()?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    Some((major, minor, patch))
}

/// 新的那個真的比較新才算數。版本號看不懂時當作沒有新版，因為猜錯會天天叫。
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parts(latest), parts(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

/// 問 crates.io 最新的穩定版是哪一版。
async fn ask() -> Option<String> {
    #[derive(Deserialize)]
    struct Answer {
        #[serde(rename = "crate")]
        package: Package,
    }
    #[derive(Deserialize)]
    struct Package {
        max_stable_version: String,
    }

    // crates.io 要求請求標明客戶端。這裡只報程式名、版本與專案網址，
    // 不帶任何關於使用者或這台機器的東西。
    let agent = format!("harioto/{} (https://github.com/Ayajiro/harioto)", current());
    let output = tokio::process::Command::new("curl")
        .args(["--silent", "--show-error", "--fail", "--location"])
        .args(["--max-time", "8", "--user-agent", &agent])
        .arg(ENDPOINT)
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        log::info!(
            "the version check did not get through: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return None;
    }

    match serde_json::from_slice::<Answer>(&output.stdout) {
        Ok(answer) => Some(answer.package.max_stable_version),
        Err(e) => {
            log::info!("crates.io answered in a shape this version does not know: {e}");
            None
        }
    }
}

/// 設定檔裡的 `update_check`。只讀這一個鍵，所以主題那邊改了也不會互相影響。
fn allowed_by_file() -> Option<bool> {
    #[derive(Deserialize)]
    struct OnlyThis {
        update_check: Option<bool>,
    }
    let text = std::fs::read_to_string(crate::theme::path()).ok()?;
    serde_json::from_str::<OnlyThis>(&text).ok()?.update_check
}

/// 這次要不要查。命令列關掉就是關掉，否則看設定檔，沒寫就查。
pub fn wanted(turned_off_by_flag: bool) -> bool {
    if turned_off_by_flag {
        return false;
    }
    allowed_by_file().unwrap_or(true)
}

/// 有新版就回一句提醒。沒有新版、問不到、或答案看不懂都回 None。
pub async fn check() -> Option<String> {
    let latest = match read_memo() {
        // 一天之內問過就用上次的答案，連線一次就夠了。
        Some(memo) if now().saturating_sub(memo.checked_at) < EVERY => memo.latest,
        _ => {
            let latest = ask().await?;
            write_memo(&Memo {
                checked_at: now(),
                latest: latest.clone(),
            });
            latest
        }
    };

    if !is_newer(&latest, current()) {
        return None;
    }
    Some(format!(
        "harioto {latest} is out, this is {}; cargo install harioto",
        current()
    ))
}

#[cfg(test)]
mod tests {
    use super::{is_newer, parts};

    #[test]
    fn a_higher_number_anywhere_counts_as_newer() {
        assert!(is_newer("0.4.0", "0.3.2"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.3.3", "0.3.2"));
    }

    #[test]
    fn the_same_version_is_not_newer() {
        assert!(!is_newer("0.3.2", "0.3.2"));
    }

    #[test]
    fn an_older_answer_is_not_newer() {
        // 本機跑在發布版前面時會是這樣，例如從 git 建置。
        assert!(!is_newer("0.3.2", "0.4.0"));
    }

    #[test]
    fn a_version_we_cannot_read_is_treated_as_no_news() {
        // 猜錯的代價是每天提醒一次更新，而那個更新並不存在。
        assert!(!is_newer("latest", "0.3.2"));
        assert!(!is_newer("0.4", "0.3.2"));
        assert!(!is_newer("0.4.0", ""));
    }

    #[test]
    fn a_suffix_on_the_patch_number_still_reads() {
        assert_eq!(parts("0.4.0-rc.1"), Some((0, 4, 0)));
        assert_eq!(parts(" 1.2.3 "), Some((1, 2, 3)));
    }
}
