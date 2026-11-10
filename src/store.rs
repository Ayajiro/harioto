//! 本機歌單：存成一份 JSON，不依賴任何線上服務。

use crate::youtube::Video;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub tracks: Vec<Video>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    pub playlists: Vec<Playlist>,
}

fn config_dir() -> PathBuf {
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

impl Library {
    pub fn load() -> Self {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| anyhow!("建立設定目錄失敗：{e}"))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| anyhow!("序列化失敗：{e}"))?;
        std::fs::write(&path, text).map_err(|e| anyhow!("寫入歌單失敗：{e}"))?;
        Ok(())
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
