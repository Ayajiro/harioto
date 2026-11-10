//! 音訊檔快取：把解析出來的串流先存成本機檔案再播放。
//!
//! 先下載完整檔案再播放，換來的是跳轉與重播都即時，而且同一首再聽不必重新下載。

use anyhow::{Result, anyhow};
use std::path::PathBuf;

/// 快取目錄。可用環境變數 `HARIOTO_CACHE_DIR` 覆寫，測試時很方便。
pub fn dir() -> PathBuf {
    if let Ok(custom) = std::env::var("HARIOTO_CACHE_DIR") {
        let custom = custom.trim();
        if !custom.is_empty() {
            return PathBuf::from(custom);
        }
    }
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("harioto")
        .join("audio")
}

fn path_for(video_id: &str) -> PathBuf {
    dir().join(format!("{video_id}.m4a"))
}

/// 快取容量上限。超過之後從最久沒播的開始刪。
pub const LIMIT_BYTES: u64 = 500 * 1024 * 1024;

/// 把檔案的時間戳更新成現在，用來記錄「最近播過」。
fn touch(path: &std::path::Path) {
    let Ok(file) = std::fs::File::options().write(true).open(path) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let times = std::fs::FileTimes::new().set_accessed(now).set_modified(now);
    let _ = file.set_times(times);
}

/// 超過上限時，從最久沒播的開始刪到低於上限，回傳釋出的位元組數。
///
/// 正在播放的檔案即使被刪掉也不影響，因為解碼器握著已開啟的描述子。
pub fn prune(limit: u64) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir()) else {
        return 0;
    };

    let mut files: Vec<(std::path::PathBuf, u64, std::time::SystemTime)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let time = meta.modified().ok()?;
            Some((path, meta.len(), time))
        })
        .collect();

    let mut current: u64 = files.iter().map(|(_, size, _)| size).sum();
    if current <= limit {
        return 0;
    }

    // 最久沒動過的排前面。
    files.sort_by_key(|(_, _, time)| *time);

    let mut freed = 0;
    for (path, size, _) in files {
        if current <= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            current -= size;
            freed += size;
        }
    }

    if freed > 0 {
        log::info!("快取清理釋出 {:.0} MB", freed as f64 / 1_048_576.0);
    }
    freed
}

/// 取得本機音訊檔，必要時先下載。
pub async fn ensure(video_id: &str) -> Result<PathBuf> {
    let final_path = path_for(video_id);
    if tokio::fs::metadata(&final_path)
        .await
        .map(|m| m.len() > 0)
        .unwrap_or(false)
    {
        // 記下這次使用，清理時才知道哪些是最近聽過的。
        let touched = final_path.clone();
        tokio::task::spawn_blocking(move || touch(&touched));
        return Ok(final_path);
    }

    tokio::fs::create_dir_all(dir())
        .await
        .map_err(|e| anyhow!("建立快取目錄失敗：{e}"))?;

    // 下載到獨立的暫存目錄再搬進快取，避免中斷留下半截檔案被當成完整快取。
    let temp_dir = tempfile::tempdir().map_err(|e| anyhow!("建立暫存目錄失敗：{e}"))?;
    let produced = crate::youtube::download(video_id, temp_dir.path()).await?;

    if tokio::fs::rename(&produced, &final_path).await.is_err() {
        // 跨檔案系統時改用複製。
        tokio::fs::copy(&produced, &final_path)
            .await
            .map_err(|e| anyhow!("寫入快取失敗：{e}"))?;
    }

    tokio::task::spawn_blocking(|| prune(LIMIT_BYTES));

    Ok(final_path)
}

/// 目前快取佔用的空間，單位位元組。
pub fn size_bytes() -> u64 {
    let Ok(entries) = std::fs::read_dir(dir()) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}
