//! YouTube 音源：透過 yt-dlp 搜尋與解析音訊串流網址。
//!
//! 搜尋與解析都不需要申請任何金鑰。

use crate::lang::t;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// 一筆可播放的曲目。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub duration_ms: u32,
}

#[derive(Deserialize)]
struct Entry {
    id: Option<String>,
    title: Option<String>,
    channel: Option<String>,
    uploader: Option<String>,
    duration: Option<f64>,
}

#[derive(Deserialize)]
struct SearchResult {
    entries: Option<Vec<Entry>>,
}

/// 確認 yt-dlp 存在，順便回報版本。
pub async fn version() -> Result<String> {
    let output = Command::new("yt-dlp")
        .arg("--version")
        .output()
        .await
        .map_err(|_| anyhow!("yt-dlp is not installed; try brew install yt-dlp"))?;

    if !output.status.success() {
        return Err(anyhow!("yt-dlp would not run"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// yt-dlp 的版本號是發布日期，格式 `YYYY.MM.DD`。算出它距今幾天。
///
/// YouTube 會定期更換 JavaScript 挑戰，過舊的 yt-dlp 會靜默失敗，所以值得提醒。
pub fn version_age_days(version: &str) -> Option<i64> {
    let mut parts = version.split('.');
    let year: i64 = parts.next()?.trim().parse().ok()?;
    let month: u32 = parts.next()?.trim().parse().ok()?;
    let day: u32 = parts.next()?.trim().parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let released = days_from_civil(year, month, day);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64
        / 86_400;

    Some(now - released)
}

/// 把年月日換算成自 1970-01-01 起算的天數。
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = month as i64;
    let d = day as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 搜尋曲目。只取清單資訊，不解析串流網址，所以很快。
pub async fn search(query: &str, limit: usize) -> Result<Vec<Video>> {
    let output = Command::new("yt-dlp")
        .args([
            "--flat-playlist",
            "--no-warnings",
            "--ignore-config",
            "-J",
            &format!("ytsearch{limit}:{query}"),
        ])
        .output()
        .await
        .map_err(|_| anyhow!("yt-dlp is not installed; try brew install yt-dlp"))?;

    if !output.status.success() {
        return Err(anyhow!(
            "search failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .next()
                .unwrap_or("unknown error")
        ));
    }

    parse_entries(&output.stdout)
}

/// 取得某首歌的 YouTube Mix，也就是 YouTube 針對這首歌產生的電台。
///
/// 回傳的清單會把種子本身排在第一筆，呼叫端自行決定要不要保留。
pub async fn radio(video_id: &str, limit: usize) -> Result<Vec<Video>> {
    let url = format!("https://www.youtube.com/watch?v={video_id}&list=RD{video_id}");
    let output = Command::new("yt-dlp")
        .args([
            "--flat-playlist",
            "--no-warnings",
            "--ignore-config",
            "--playlist-end",
            &limit.to_string(),
            "-J",
            &url,
        ])
        .output()
        .await
        .map_err(|_| anyhow!("yt-dlp is not installed; try brew install yt-dlp"))?;

    if !output.status.success() {
        return Err(anyhow!(
            "could not build the radio: {}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .last()
                .unwrap_or("unknown error")
        ));
    }

    parse_entries(&output.stdout)
}

/// 單一影片的觀看網址。紀錄與對外輸出都用這個形式。
pub fn watch_url(video_id: &str) -> String {
    format!("https://www.youtube.com/watch?v={video_id}")
}

/// 查一支影片的標題、頻道與長度。加入歌單時用得到。
pub async fn lookup(video_id: &str) -> Result<Video> {
    let output = Command::new("yt-dlp")
        .args([
            "--flat-playlist",
            "--no-warnings",
            "--ignore-config",
            "--no-playlist",
            "-J",
            &watch_url(video_id),
        ])
        .output()
        .await
        .map_err(|_| anyhow!("yt-dlp is not installed; try brew install yt-dlp"))?;

    if !output.status.success() {
        return Err(anyhow!(
            "could not look up the video: {}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .last()
                .unwrap_or("unknown error")
        ));
    }

    let entry: Entry = serde_json::from_slice(&output.stdout)
        .map_err(|e| anyhow!("could not parse the result: {e}"))?;
    let id = entry.id.unwrap_or_else(|| video_id.to_string());
    Ok(Video {
        id,
        title: entry
            .title
            .unwrap_or_else(|| t!("未知標題", "Untitled", "タイトル不明").into()),
        channel: entry
            .channel
            .or(entry.uploader)
            .unwrap_or_else(|| t!("未知頻道", "Unknown channel", "チャンネル不明").into()),
        duration_ms: entry.duration.map(|d| (d * 1000.0) as u32).unwrap_or(0),
    })
}

/// 從使用者給的字串取出影片 ID，接受裸 ID、youtu.be 短網址與 watch 網址。
pub fn extract_id(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if !input.contains('/') && !input.contains('?') && !input.contains('&') {
        return Some(input.to_string());
    }
    if let Some(rest) = input.split("v=").nth(1) {
        let id: String = rest.chars().take_while(|c| *c != '&').collect();
        if !id.is_empty() {
            return Some(id);
        }
    }
    if let Some(rest) = input.rsplit('/').next() {
        let id: String = rest.chars().take_while(|c| *c != '?').collect();
        if !id.is_empty() {
            return Some(id);
        }
    }
    None
}

fn parse_entries(stdout: &[u8]) -> Result<Vec<Video>> {
    let parsed: SearchResult =
        serde_json::from_slice(stdout).map_err(|e| anyhow!("could not parse the result: {e}"))?;

    Ok(parsed
        .entries
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| {
            let id = e.id?;
            Some(Video {
                id,
                title: e
                    .title
                    .unwrap_or_else(|| t!("未知標題", "Untitled", "タイトル不明").into()),
                channel: e
                    .channel
                    .or(e.uploader)
                    .unwrap_or_else(|| t!("未知頻道", "Unknown channel", "チャンネル不明").into()),
                duration_ms: e.duration.map(|d| (d * 1000.0) as u32).unwrap_or(0),
            })
        })
        .collect())
}

/// 把音訊下載到指定目錄，回傳實際產生的檔案路徑。
///
/// 交給 yt-dlp 下載而不是自己取網址再抓，因為 googlevideo 的網址綁定請求者的標頭，
/// 換一個用戶端去取會收到 403。
pub async fn download(video_id: &str, dest_dir: &std::path::Path) -> Result<std::path::PathBuf> {
    let output = Command::new("yt-dlp")
        .args([
            "--no-warnings",
            "--ignore-config",
            "--no-part",
            "--no-playlist",
            "-f",
            "bestaudio[ext=m4a]/bestaudio[acodec^=mp4a]/bestaudio",
            "-P",
        ])
        .arg(dest_dir)
        .args(["-o", "%(id)s.%(ext)s"])
        .arg(format!("https://www.youtube.com/watch?v={video_id}"))
        .output()
        .await
        .map_err(|_| anyhow!("yt-dlp is not installed"))?;

    if !output.status.success() {
        return Err(anyhow!(
            "download failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .last()
                .unwrap_or("unknown error")
        ));
    }

    let mut entries = std::fs::read_dir(dest_dir)
        .map_err(|e| anyhow!("could not read the download directory: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file());

    entries
        .next()
        .ok_or_else(|| anyhow!("yt-dlp produced no file"))
}

/// 影片的標籤，給電台種子標示風格用。回傳已經過濾、最多三個。
///
/// 標籤是上傳者自己填的，常混著「Official Music Video」這類和風格無關的字串，
/// 也常把曲名與頻道名再寫一次，所以這裡只留下畫面上還沒出現過的那些。
pub async fn tags(video_id: &str, title: &str, channel: &str) -> Result<Vec<String>> {
    let output = Command::new("yt-dlp")
        .args([
            "--no-warnings",
            "--ignore-config",
            "--no-playlist",
            "--skip-download",
            "-J",
            &watch_url(video_id),
        ])
        .output()
        .await
        .map_err(|_| anyhow!("yt-dlp is not installed; try brew install yt-dlp"))?;

    if !output.status.success() {
        return Err(anyhow!("could not look up the tags"));
    }

    #[derive(serde::Deserialize)]
    struct Details {
        #[serde(default)]
        tags: Vec<String>,
    }

    let details: Details = serde_json::from_slice(&output.stdout)
        .map_err(|e| anyhow!("could not parse the result: {e}"))?;
    Ok(pick_tags(&details.tags, title, channel))
}

/// 擋掉和曲名、頻道重複的標籤，以及純粹描述影片型態的字眼。
fn pick_tags(tags: &[String], title: &str, channel: &str) -> Vec<String> {
    const NOISE: [&str; 8] = [
        "official",
        "music video",
        "mv",
        "lyric",
        "audio",
        "hd",
        "live",
        "full album",
    ];
    let title = title.to_lowercase();
    let channel = channel.to_lowercase();

    tags.iter()
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty() && tag.chars().count() <= 24)
        .filter(|tag| {
            let lower = tag.to_lowercase();
            if title.contains(&lower) || channel.contains(&lower) || lower.contains(&channel) {
                return false;
            }
            !NOISE.iter().any(|noise| lower.contains(noise))
        })
        .take(3)
        .map(|tag| tag.to_string())
        .collect()
}

#[cfg(test)]
mod tag_tests {
    use super::pick_tags;

    #[test]
    fn keeps_style_words_and_drops_the_rest() {
        let tags = [
            "plastic love",
            "mariya takeuchi",
            "city pop",
            "Official Music Video",
            "Tatsuro Yamashita",
            "竹内まりや",
        ]
        .map(String::from);
        let picked = pick_tags(&tags, "竹内まりや - Plastic Love", "Mariya Takeuchi");
        assert_eq!(picked, vec!["city pop", "Tatsuro Yamashita"]);
    }

    #[test]
    fn an_empty_list_stays_empty() {
        assert!(pick_tags(&[], "曲名", "頻道").is_empty());
    }
}
