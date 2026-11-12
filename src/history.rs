//! 連結紀錄：種子與播放歷程。只存影片連結與標題，不存音訊。

use crate::youtube::{Video, watch_url};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 兩份紀錄各自保留的筆數上限，超過就從最舊的捨棄。
const HISTORY_LIMIT: usize = 10_000;
const SEED_LIMIT: usize = 2_000;

/// 每行大約兩百位元組，乘上上限就是觸發修剪的檔案大小。
const BYTES_PER_LINE: u64 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// 按 r 開電台時用的那首歌。
    Seed,
    /// 實際開始播放的每一首。
    History,
}

impl Kind {
    fn file_name(self) -> &'static str {
        match self {
            Kind::Seed => "seeds.jsonl",
            Kind::History => "history.jsonl",
        }
    }

    fn limit(self) -> usize {
        match self {
            Kind::Seed => SEED_LIMIT,
            Kind::History => HISTORY_LIMIT,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    /// UTC，形如 2026-11-11T07:12:33Z。
    pub at: String,
    pub at_unix: i64,
    pub id: String,
    pub url: String,
    pub title: String,
    pub channel: String,
}

pub fn path(kind: Kind) -> PathBuf {
    crate::store::config_dir().join(kind.file_name())
}

/// 寫進一筆紀錄。失敗只記在日誌裡，播放不該因為寫紀錄而中斷。
pub fn record(kind: Kind, video: &Video) {
    if let Err(e) = try_record(kind, video) {
        log::warn!("could not write to {}: {e}", kind.file_name());
    }
}

fn try_record(kind: Kind, video: &Video) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let entry = Entry {
        at: format_utc(now),
        at_unix: now,
        id: video.id.clone(),
        url: watch_url(&video.id),
        title: video.title.clone(),
        channel: video.channel.clone(),
    };

    let path = path(kind);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("could not create the config directory: {e}"))?;
    }

    let line =
        serde_json::to_string(&entry).map_err(|e| anyhow!("could not serialise the entry: {e}"))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| anyhow!("could not open the log: {e}"))?;
    writeln!(file, "{line}").map_err(|e| anyhow!("could not write the log: {e}"))?;
    drop(file);

    trim(kind)
}

/// 檔案長到超過上限的估計大小時，只留最後 N 行。
fn trim(kind: Kind) -> Result<()> {
    let path = path(kind);
    let limit = kind.limit();
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if size <= limit as u64 * BYTES_PER_LINE {
        return Ok(());
    }

    let text =
        std::fs::read_to_string(&path).map_err(|e| anyhow!("could not read the log: {e}"))?;
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() <= limit {
        return Ok(());
    }

    let kept = &lines[lines.len() - limit..];
    let mut out = kept.join("\n");
    out.push('\n');
    std::fs::write(&path, out).map_err(|e| anyhow!("could not trim the log: {e}"))?;
    log::info!("{} trimmed to {limit} entries", kind.file_name());
    Ok(())
}

/// 讀出紀錄，最新的排在最前面。limit 為 0 代表全部。
pub fn read(kind: Kind, limit: usize) -> Vec<Entry> {
    let Ok(text) = std::fs::read_to_string(path(kind)) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    entries.reverse();
    if limit > 0 {
        entries.truncate(limit);
    }
    entries
}

/// 把 Unix 秒數寫成 UTC 的 RFC 3339 字串，不引入日期套件。
fn format_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{year:04}-{month:02}-{day:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Howard Hinnant 的 civil_from_days，days 是距離 1970-01-01 的天數。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_reads_back_history() {
        let _guard = crate::store::CONFIG_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("建立暫存目錄");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };

        let video = Video {
            id: "abc123".into(),
            title: "測試曲目".into(),
            channel: "測試頻道".into(),
            duration_ms: 240_000,
        };
        record(Kind::History, &video);

        let entries = read(Kind::History, 10);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "abc123");
        assert_eq!(entries[0].url, "https://www.youtube.com/watch?v=abc123");
        assert!(path(Kind::History).ends_with("harioto/history.jsonl"));
    }

    #[test]
    fn formats_known_instants() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_762_000_000), "2025-11-01T12:26:40Z");
    }
}
