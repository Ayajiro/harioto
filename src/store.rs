//! 本機歌單：存成一份 JSON，不依賴任何線上服務。

use crate::youtube::Video;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub tracks: Vec<Video>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    pub playlists: Vec<Playlist>,
    /// 讀到這份內容時檔案的修改時間，用來察覺別的行程寫過了。
    #[serde(skip)]
    seen_mtime: Option<SystemTime>,
}

pub fn config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let xdg = xdg.trim();
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("harioto");
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("harioto")
}

pub fn path() -> PathBuf {
    config_dir().join("playlists.json")
}

fn file_mtime() -> Option<SystemTime> {
    std::fs::metadata(path()).ok()?.modified().ok()
}

impl Library {
    pub fn load() -> Self {
        let mut library: Library = std::fs::read_to_string(path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        library.seen_mtime = file_mtime();
        library
    }

    /// 先寫暫存檔再換名，別的行程不會讀到寫到一半的內容。
    pub fn save(&mut self) -> Result<()> {
        let path = path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow!("could not create the config directory: {e}"))?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| anyhow!("could not serialise the playlists: {e}"))?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, text).map_err(|e| anyhow!("could not write the playlists: {e}"))?;
        std::fs::rename(&temp, &path)
            .map_err(|e| anyhow!("could not replace the playlist file: {e}"))?;
        self.seen_mtime = file_mtime();
        Ok(())
    }

    /// 檔案在這份內容讀進來之後被改過了嗎。
    pub fn changed_on_disk(&self) -> bool {
        match (file_mtime(), self.seen_mtime) {
            (Some(now), Some(seen)) => now != seen,
            (Some(_), None) => true,
            (None, Some(_)) => true,
            (None, None) => false,
        }
    }

    /// 檔案變了就重讀，回報有沒有換過內容。
    pub fn reload_if_changed(&mut self) -> bool {
        if !self.changed_on_disk() {
            return false;
        }
        *self = Library::load();
        true
    }

    /// 改動前先重讀檔案再套用，避免覆蓋別的行程（命令列、MCP）寫進去的內容。
    pub fn mutate<F>(&mut self, edit: F) -> Result<()>
    where
        F: FnOnce(&mut Library) -> Result<()>,
    {
        let mut fresh = Library::load();
        edit(&mut fresh)?;
        fresh.save()?;
        *self = fresh;
        Ok(())
    }

    /// 依 ID 從歌單移除一首，回傳被移除的曲目。
    pub fn remove_track(&mut self, name: &str, video_id: &str) -> Result<Video> {
        let playlist = self
            .playlists
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| anyhow!("no playlist called {name}"))?;
        let index = playlist
            .tracks
            .iter()
            .position(|t| t.id == video_id)
            .ok_or_else(|| anyhow!("that track is no longer in the playlist"))?;
        Ok(playlist.tracks.remove(index))
    }

    /// 找一份歌單，名稱需完全相同。
    pub fn find(&self, name: &str) -> Option<&Playlist> {
        self.playlists.iter().find(|p| p.name == name)
    }

    /// 把一首歌加進歌單，重複的曲目會略過。回傳加入後的曲目數。
    pub fn add_track(&mut self, name: &str, video: Video) -> Result<usize> {
        let playlist = self
            .playlists
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| anyhow!("no playlist called {name}"))?;
        if !playlist.tracks.iter().any(|t| t.id == video.id) {
            playlist.tracks.push(video);
        }
        Ok(playlist.tracks.len())
    }

    pub fn create(&mut self, name: &str) {
        if self.playlists.iter().any(|p| p.name == name) {
            return;
        }
        self.playlists.push(Playlist {
            name: name.to_string(),
            tracks: Vec::new(),
        });
    }
}

/// 測試會改動 XDG_CONFIG_HOME，一次只能跑一個。
#[cfg(test)]
pub static CONFIG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    fn video(id: &str) -> Video {
        Video {
            id: id.into(),
            title: format!("曲目 {id}"),
            channel: "測試頻道".into(),
            duration_ms: 180_000,
        }
    }

    #[test]
    fn mutate_keeps_what_another_process_wrote() {
        let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("建立暫存目錄");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };

        // 播放器啟動，讀進一份只有「我的」的歌單。
        let mut player = Library::load();
        player.create("我的");
        player.save().expect("寫入");

        // 命令列或 MCP 在這期間加了一份歌單。
        let mut external = Library::load();
        external.create("外部");
        external.add_track("外部", video("ext1")).expect("加入");
        external.save().expect("寫入");

        // 播放器接著加自己的曲目。
        player
            .mutate(|lib| lib.add_track("我的", video("mine1")).map(|_| ()))
            .expect("改動");

        let saved = Library::load();
        assert!(saved.find("外部").is_some(), "外部寫入的歌單被覆蓋了");
        assert_eq!(saved.find("外部").unwrap().tracks.len(), 1);
        assert_eq!(saved.find("我的").unwrap().tracks.len(), 1);
    }

    #[test]
    fn reload_picks_up_external_writes() {
        let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("建立暫存目錄");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };

        let mut player = Library::load();
        player.create("我的");
        player.save().expect("寫入");
        assert!(!player.changed_on_disk());

        let mut external = Library::load();
        external.create("外部");
        external.save().expect("寫入");

        assert!(player.changed_on_disk());
        assert!(player.reload_if_changed());
        assert_eq!(player.playlists.len(), 2);
        assert!(!player.reload_if_changed());
    }

    #[test]
    fn remove_track_finds_by_id() {
        let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("建立暫存目錄");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };

        let mut library = Library::default();
        library.create("我的");
        library.add_track("我的", video("a")).expect("加入");
        library.add_track("我的", video("b")).expect("加入");

        let removed = library.remove_track("我的", "a").expect("移除");
        assert_eq!(removed.id, "a");
        assert_eq!(library.find("我的").unwrap().tracks.len(), 1);
        assert!(library.remove_track("我的", "a").is_err());
    }
}
