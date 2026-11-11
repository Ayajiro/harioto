//! 對外共用的操作層。終端機介面、命令列子指令與 MCP 伺服器都呼叫這裡，
//! 確保三個入口的行為與輸出結構一致。

use crate::history::{self, Kind};
use crate::store::Library;
use crate::youtube::{self, Video, watch_url};
use anyhow::{Result, anyhow};
use serde::Serialize;

/// 輸出用的曲目，比內部的 Video 多一個可直接點開的網址。
#[derive(Clone, Debug, Serialize)]
pub struct TrackOut {
    pub id: String,
    pub url: String,
    pub title: String,
    pub channel: String,
    pub duration_ms: u32,
    pub duration: String,
}

impl From<&Video> for TrackOut {
    fn from(v: &Video) -> Self {
        TrackOut {
            id: v.id.clone(),
            url: watch_url(&v.id),
            title: v.title.clone(),
            channel: v.channel.clone(),
            duration_ms: v.duration_ms,
            duration: crate::app::format_duration(v.duration_ms),
        }
    }
}

pub fn tracks_out(videos: &[Video]) -> Vec<TrackOut> {
    videos.iter().map(TrackOut::from).collect()
}

#[derive(Debug, Serialize)]
pub struct PlaylistSummary {
    pub name: String,
    pub count: usize,
}

/// 搜尋 YouTube。
pub async fn search(query: &str, limit: usize) -> Result<Vec<Video>> {
    if query.trim().is_empty() {
        return Err(anyhow!("搜尋關鍵字是空的"));
    }
    youtube::search(query, limit.clamp(1, 100)).await
}

/// 以一首歌為種子建立電台，種子排在第一筆，同時寫進種子紀錄。
pub async fn radio(seed_input: &str, limit: usize) -> Result<Vec<Video>> {
    let id = youtube::extract_id(seed_input).ok_or_else(|| anyhow!("無法解析影片 ID"))?;
    let mut tracks = youtube::radio(&id, limit.clamp(1, 100)).await?;

    let seed = match tracks.iter().find(|t| t.id == id).cloned() {
        Some(seed) => seed,
        None => youtube::lookup(&id).await?,
    };
    history::record(Kind::Seed, &seed);

    tracks.retain(|t| t.id != id);
    tracks.insert(0, seed);
    Ok(tracks)
}

/// 電台由終端機介面發起時，種子已經在手上，直接記錄並取回推薦。
pub async fn radio_with_seed(seed: &Video, limit: usize) -> Result<Vec<Video>> {
    history::record(Kind::Seed, seed);
    let mut tracks = youtube::radio(&seed.id, limit.clamp(1, 100)).await?;
    tracks.retain(|t| t.id != seed.id);
    tracks.insert(0, seed.clone());
    Ok(tracks)
}

pub fn playlists() -> Vec<PlaylistSummary> {
    Library::load()
        .playlists
        .iter()
        .map(|p| PlaylistSummary {
            name: p.name.clone(),
            count: p.tracks.len(),
        })
        .collect()
}

pub fn playlist_show(name: &str) -> Result<Vec<Video>> {
    Library::load()
        .find(name)
        .map(|p| p.tracks.clone())
        .ok_or_else(|| anyhow!("找不到歌單：{name}"))
}

/// 建立歌單。名稱已存在時回報，不覆蓋既有內容。
pub fn playlist_create(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("歌單名稱是空的"));
    }
    let mut library = Library::load();
    if library.find(name).is_some() {
        return Err(anyhow!("歌單已經存在：{name}"));
    }
    library.create(name);
    library.save()
}

/// 把一批影片加進歌單，沒有這份歌單就先建立。回傳加入後的曲目數。
pub async fn playlist_add(name: &str, inputs: &[String]) -> Result<usize> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("歌單名稱是空的"));
    }
    if inputs.is_empty() {
        return Err(anyhow!("沒有要加入的影片"));
    }

    let mut videos = Vec::with_capacity(inputs.len());
    for input in inputs {
        let id = youtube::extract_id(input)
            .ok_or_else(|| anyhow!("無法解析影片 ID：{input}"))?;
        videos.push(youtube::lookup(&id).await?);
    }

    let mut library = Library::load();
    if library.find(name).is_none() {
        library.create(name);
    }
    let mut count = 0;
    for video in videos {
        count = library.add_track(name, video)?;
    }
    library.save()?;
    Ok(count)
}

pub fn history(limit: usize) -> Vec<history::Entry> {
    history::read(Kind::History, limit)
}

pub fn seeds(limit: usize) -> Vec<history::Entry> {
    history::read(Kind::Seed, limit)
}
