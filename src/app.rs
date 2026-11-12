//! 應用程式狀態與按鍵處理。

use crate::{
    audio::{AudioEvent, AudioPlayer},
    cache, cover,
    loudness::{self, Measurement},
    store::Library,
    theme::Theme,
    youtube::{self, Video},
};
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::Rect,
    widgets::ListState,
};
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::mpsc::UnboundedSender;

const SEEK_STEP_MS: u32 = 5_000;
const VOLUME_STEP: f32 = 0.05;
/// 狀態訊息停留多久，之後換回電台種子資訊。
const STATUS_HOLD: Duration = Duration::from_secs(8);
const SEARCH_LIMIT: usize = 30;
const RADIO_LIMIT: usize = 100;
/// 超過這個長度就先攔下來確認，通常是合輯或直播而不是單曲。
const LONG_TRACK_MS: u32 = 20 * 60 * 1000;
/// 自動接續時最多連續跳過幾首過長的曲目。
const MAX_AUTO_SKIP: usize = 10;

pub enum Msg {
    Key(KeyEvent),
    Audio(AudioEvent),
    Results { title: String, tracks: Vec<Video> },
    Radio {
        title: String,
        tracks: Vec<Video>,
        /// 真：目前這首繼續放完，電台接在後面。
        continue_current: bool,
        /// 這個電台的種子。
        seed: Video,
    },
    Ready {
        video_id: String,
        path: PathBuf,
    },
    /// 種子的風格標籤查回來了。
    SeedTags {
        video_id: String,
        tags: Vec<String>,
    },
    /// 背景抓好並解開了一首的縮圖。
    Cover {
        video_id: String,
        image: Box<cover::Image>,
    },
    /// 背景量測完一首的響度。
    Loudness {
        video_id: String,
        measurement: Measurement,
    },
    /// 外部程式透過控制通道送進來的指令，處理完要回覆。
    Control {
        control: crate::control::Control,
        reply: tokio::sync::oneshot::Sender<serde_json::Value>,
    },
    Error(String),
    Tick,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Mode {
    Browse,
    Search,
    NewPlaylist,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Focus {
    Playlists,
    Tracks,
}

/// 目前右欄顯示的是搜尋結果還是某份歌單。
#[derive(Clone)]
pub enum View {
    Search,
    Playlist(String),
}

pub struct NowPlaying {
    pub video: Video,
    position_ms: u32,
    marked_at: Instant,
    pub paused: bool,
    pub buffering: bool,
}

impl NowPlaying {
    pub fn position_ms(&self) -> u32 {
        if self.paused || self.buffering {
            return self.position_ms;
        }
        let elapsed = self.marked_at.elapsed().as_millis() as u32;
        self.position_ms
            .saturating_add(elapsed)
            .min(self.video.duration_ms.max(1))
    }
}

pub struct App {
    pub audio: AudioPlayer,
    pub tx: UnboundedSender<Msg>,
    pub should_quit: bool,
    pub mode: Mode,
    pub focus: Focus,
    pub input: String,
    pub library: Library,
    pub playlist_state: ListState,
    pub tracks: Vec<Video>,
    pub track_state: ListState,
    pub track_title: String,
    pub view: View,
    pub now: Option<NowPlaying>,
    pub status: String,
    playing_index: Option<usize>,
    /// 已經提示過一次、等待再按一次確認的曲目。
    pending_confirm: Option<usize>,
    /// 響度平衡是否生效。
    pub normalize: bool,
    /// 已知的響度量測結果，開機時從檔案讀回來。
    loudness: HashMap<String, Measurement>,
    /// 目前電台的種子，點播與搜尋結果沒有種子。
    seed: Option<Video>,
    /// 種子的風格標籤，查得到才有。
    seed_tags: Vec<String>,
    /// 狀態訊息寫進來的時間，過一陣子就換成種子資訊。
    status_at: Instant,
    /// 目前的配色與框線。
    pub theme: Theme,
    /// `--theme` 指定的內建主題，重讀設定檔時仍然優先。
    theme_preset: Option<String>,
    /// 主題檔上次讀到的修改時間，用來察覺外部改動。
    theme_mtime: Option<SystemTime>,
    /// 目前這首的縮圖，解開之後放著重複使用。
    cover: Option<cover::Image>,
    /// 使用者要不要看縮圖。設定檔給初值，按鍵臨時改。
    pub cover_on: bool,
    /// 畫面替縮圖留的位置，由版面決定，沒有就是這次不畫。
    pub cover_rect: Option<Rect>,
    /// 已經畫上去的是哪一首、畫在哪裡，用來判斷要不要重送。
    cover_shown: Option<(String, Rect)>,
}

impl App {
    pub fn new(
        mut audio: AudioPlayer,
        tx: UnboundedSender<Msg>,
        note: Option<String>,
        theme_preset: Option<String>,
    ) -> Self {
        audio.set_volume(0.5);
        let theme = crate::theme::load(theme_preset.as_deref());
        let library = Library::load();
        let mut playlist_state = ListState::default();
        if !library.playlists.is_empty() {
            playlist_state.select(Some(0));
        }

        Self {
            audio,
            tx,
            should_quit: false,
            mode: Mode::Browse,
            focus: Focus::Tracks,
            input: String::new(),
            library,
            playlist_state,
            tracks: Vec::new(),
            track_state: ListState::default(),
            track_title: "搜尋結果".into(),
            view: View::Search,
            now: None,
            status: note.unwrap_or_else(|| {
                format!(
                    "按 / 搜尋，按 r 開電台（快取 {:.0} MB）",
                    cache::size_bytes() as f64 / 1_048_576.0
                )
            }),
            playing_index: None,
            pending_confirm: None,
            normalize: true,
            loudness: loudness::load(),
            seed: None,
            seed_tags: Vec::new(),
            status_at: Instant::now(),
            cover_on: theme.cover,
            theme,
            theme_mtime: crate::theme::mtime(),
            theme_preset,
            cover: None,
            cover_rect: None,
            cover_shown: None,
        }
    }

    /// 開關縮圖。終端機畫不出圖時說明原因，免得按了沒反應。
    fn toggle_cover(&mut self) {
        if !cover::supported() {
            self.set_status("這個終端機沒有回應圖形查詢，畫不出縮圖".into());
            return;
        }
        self.cover_on = !self.cover_on;
        let state = if self.cover_on { "開" } else { "關" };
        self.set_status(format!("縮圖：{state}"));
    }

    /// 把縮圖畫到版面留好的位置。每畫完一幀呼叫一次，位置或曲目沒變就什麼都不做。
    pub fn sync_cover(&mut self) {
        if !cover::supported() {
            return;
        }
        let Some(rect) = self.cover_rect else {
            if self.cover_shown.take().is_some() {
                cover::clear();
            }
            return;
        };
        let (Some(image), Some(now)) = (self.cover.as_ref(), self.now.as_ref()) else {
            return;
        };
        let key = (now.video.id.clone(), rect);
        if self.cover_shown.as_ref() == Some(&key) {
            return;
        }
        match cover::place(image, rect) {
            Ok(()) => self.cover_shown = Some(key),
            Err(e) => log::warn!("畫縮圖失敗：{e}"),
        }
    }

    /// 換曲時把上一張收掉，並在背景抓新的一張。
    fn load_cover(&mut self, video_id: String) {
        if !cover::supported() {
            return;
        }
        self.cover = None;
        if self.cover_shown.take().is_some() {
            cover::clear();
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let path = match cover::ensure(&video_id).await {
                Ok(path) => path,
                Err(e) => {
                    log::warn!("抓 {video_id} 的縮圖失敗：{e}");
                    return;
                }
            };
            match tokio::task::spawn_blocking(move || cover::decode(&path)).await {
                Ok(Ok(image)) => {
                    let _ = tx.send(Msg::Cover {
                        video_id,
                        image: Box::new(image),
                    });
                }
                Ok(Err(e)) => log::warn!("解 {video_id} 的縮圖失敗：{e}"),
                Err(e) => log::warn!("解 {video_id} 的縮圖中斷：{e}"),
            }
        });
    }

    /// 寫狀態訊息，並記下時間：訊息擺一陣子之後換成電台種子資訊。
    fn set_status(&mut self, text: String) {
        self.status = text;
        self.status_at = Instant::now();
    }

    /// 記住這份清單的電台種子，並在背景查它的風格標籤。
    fn remember_seed(&mut self, seed: Option<Video>) {
        self.seed_tags.clear();
        self.seed = seed.clone();
        let Some(seed) = seed else {
            return;
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match youtube::tags(&seed.id, &seed.title, &seed.channel).await {
                Ok(tags) if !tags.is_empty() => {
                    let _ = tx.send(Msg::SeedTags {
                        video_id: seed.id,
                        tags,
                    });
                }
                Ok(_) => {}
                Err(e) => log::warn!("查種子標籤失敗：{e}"),
            }
        });
    }

    /// 響度平衡的狀態描述，開關與控制通道共用。
    fn normalize_status(&self) -> String {
        if !self.normalize {
            "響度平衡：關".into()
        } else if self.gain_ready() {
            format!("響度平衡：開（這首 {:+.1} dB）", self.gain_db())
        } else {
            "響度平衡：開（這首還在量）".into()
        }
    }

    /// 狀態列左側要顯示的字。訊息擺幾秒就讓位給按鍵提示，平常這一行只有提示。
    pub fn context_line(&self) -> String {
        if self.status_at.elapsed() < STATUS_HOLD {
            return self.status.clone();
        }
        String::new()
    }

    /// 主題檔被改過就重讀，讓調色不必重開播放器。
    fn refresh_theme(&mut self) {
        let mtime = crate::theme::mtime();
        if mtime == self.theme_mtime {
            return;
        }
        self.theme_mtime = mtime;
        self.theme = crate::theme::load(self.theme_preset.as_deref());
        // 設定檔是這件事的主張，重讀之後按鍵的臨時改動就讓位。
        self.cover_on = self.theme.cover;
        self.status = "已套用新的主題設定".into();
    }

    pub fn volume_percent(&self) -> u16 {
        (self.audio.volume() * 100.0).round() as u16
    }

    // ---------- 查詢 ----------

    fn run_search(&mut self) {
        let query = self.input.trim().to_string();
        if query.is_empty() {
            return;
        }
        let tx = self.tx.clone();
        self.status = format!("正在搜尋「{query}」…");
        tokio::spawn(async move {
            let msg = match youtube::search(&query, SEARCH_LIMIT).await {
                Ok(tracks) => Msg::Results {
                    title: format!("搜尋：{query}"),
                    tracks,
                },
                Err(e) => Msg::Error(e.to_string()),
            };
            let _ = tx.send(msg);
        });
    }

    /// 以目前播放中的曲目為種子開電台，沒有在播就用右欄選取的那首。
    ///
    /// 正在播的那首會留在清單第一位並繼續播完，電台接在它後面。
    fn start_radio(&mut self) {
        let playing = self.now.as_ref().map(|n| n.video.clone());
        let seed = playing.clone().or_else(|| {
            self.track_state
                .selected()
                .and_then(|i| self.tracks.get(i).cloned())
        });

        let Some(seed) = seed else {
            self.status = "請先選一首曲目，或先播一首再開電台".into();
            return;
        };

        // 已經在播種子的話就不要重新載入，讓它放完。
        let continue_current = playing.map(|p| p.id == seed.id).unwrap_or(false);

        let tx = self.tx.clone();
        let title = format!("電台：{}", seed.title);
        self.status = format!("正在建立電台：{}…", seed.title);
        tokio::spawn(async move {
            // 種子固定排在第一位、並寫進種子紀錄，都在 api 層處理。
            let msg = match crate::api::radio_with_seed(&seed, RADIO_LIMIT).await {
                Ok(tracks) => Msg::Radio {
                    title,
                    tracks,
                    continue_current,
                    seed,
                },
                Err(e) => Msg::Error(e.to_string()),
            };
            let _ = tx.send(msg);
        });
    }

    fn open_selected_playlist(&mut self) {
        let Some(index) = self.playlist_state.selected() else {
            return;
        };
        let Some(playlist) = self.library.playlists.get(index) else {
            return;
        };
        self.track_title = playlist.name.clone();
        self.tracks = playlist.tracks.clone();
        self.view = View::Playlist(playlist.name.clone());
        self.playing_index = None;
        self.track_state
            .select(if self.tracks.is_empty() { None } else { Some(0) });
        self.focus = Focus::Tracks;
        self.status = format!("清單共 {} 首", self.tracks.len());
    }

    // ---------- 控制通道 ----------

    /// 目前狀態，回給控制通道的呼叫端。
    pub fn status_json(&self) -> serde_json::Value {
        serde_json::json!({
            "playing": self.now.as_ref().map(|n| serde_json::json!({
                "id": n.video.id,
                "url": crate::youtube::watch_url(&n.video.id),
                "title": n.video.title,
                "channel": n.video.channel,
                "duration_ms": n.video.duration_ms,
                "position_ms": n.position_ms(),
                "paused": n.paused,
                "buffering": n.buffering,
            })),
            "list": self.track_title,
            "queue_len": self.tracks.len(),
            "index": self.playing_index,
            "volume": self.volume_percent(),
            "normalize": self.normalize,
            "seed": self.seed.as_ref().map(|seed| serde_json::json!({
                "id": seed.id,
                "title": seed.title,
                "channel": seed.channel,
                "tags": self.seed_tags,
            })),
            "gain_db": (self.gain_db() * 10.0).round() / 10.0,
        })
    }

    /// 擋掉長度未知或過長的項目，回傳留下來的清單與被擋掉的數量。
    fn playable_only(tracks: Vec<Video>) -> (Vec<Video>, usize) {
        let total = tracks.len();
        let kept: Vec<Video> = tracks
            .into_iter()
            .filter(|v| Self::too_long_reason(v).is_none())
            .collect();
        let dropped = total - kept.len();
        (kept, dropped)
    }

    fn control(&mut self, control: crate::control::Control) -> serde_json::Value {
        use crate::control::Control;

        // 緩衝中的曲目不接受暫停與繼續，要讓呼叫端知道指令沒有生效。
        let buffering = self.now.as_ref().is_some_and(|n| n.buffering);
        let mut note: Option<String> = None;

        let action = match control {
            Control::Status => return self.status_json(),
            Control::Toggle | Control::Pause | Control::Resume if buffering => {
                note = Some("曲目還在緩衝，暫停與繼續這時不會生效".into());
                "buffering"
            }
            Control::Toggle => {
                self.toggle_pause();
                "toggle"
            }
            Control::Pause => {
                if self.now.as_ref().is_some_and(|n| !n.paused) {
                    self.toggle_pause();
                }
                "pause"
            }
            Control::Resume => {
                if self.now.as_ref().is_some_and(|n| n.paused) {
                    self.toggle_pause();
                }
                "resume"
            }
            Control::Next => {
                self.play_offset(1);
                "next"
            }
            Control::Prev => {
                self.play_offset(-1);
                "prev"
            }
            Control::Stop => {
                self.audio.stop();
                self.now = None;
                self.status = "已停止（來自控制通道）".into();
                "stop"
            }
            Control::Remove(target) => {
                let index = match target.as_deref() {
                    None | Some("") => self.playing_index,
                    Some(id) => self.tracks.iter().position(|track| track.id == id),
                };
                if index.is_none() {
                    return serde_json::json!({ "error": "這首不在目前的清單裡" });
                }
                self.drop_from_queue(index);
                "remove"
            }
            Control::Normalize(wanted) => {
                self.normalize = wanted.unwrap_or(!self.normalize);
                self.apply_gain();
                self.status = self.normalize_status();
                self.status_at = Instant::now();
                "normalize"
            }
            Control::Volume(level) => {
                self.audio.set_volume(level as f32 / 100.0);
                self.status = format!("音量 {level}%（來自控制通道）");
                "volume"
            }
            Control::Replace {
                title,
                tracks,
                start,
                seed,
            } => {
                let (tracks, dropped) = Self::playable_only(tracks);
                if tracks.is_empty() {
                    return serde_json::json!({ "error": "清單裡沒有可播的曲目" });
                }
                self.track_title = title;
                self.tracks = tracks;
                self.view = View::Search;
                self.focus = Focus::Tracks;
                self.pending_confirm = None;
                let start = start.min(self.tracks.len() - 1);
                self.track_state.select(Some(start));
                self.play_index(start);
                self.remember_seed(seed);
                self.status = if dropped > 0 {
                    format!("清單共 {} 首（略過 {dropped} 首過長項目）", self.tracks.len())
                } else {
                    format!("清單共 {} 首", self.tracks.len())
                };
                "replace"
            }
            Control::Queue(tracks) => {
                let (tracks, dropped) = Self::playable_only(tracks);
                if tracks.is_empty() {
                    return serde_json::json!({ "error": "沒有可加入的曲目" });
                }
                let added = tracks.len();
                self.tracks.extend(tracks);
                self.status = if dropped > 0 {
                    format!("已排入 {added} 首（略過 {dropped} 首過長項目）")
                } else {
                    format!("已排入 {added} 首")
                };
                "queue"
            }
        };

        let mut answer = self.status_json();
        if let Some(object) = answer.as_object_mut() {
            object.insert("action".into(), serde_json::Value::from(action));
            if let Some(note) = note {
                object.insert("note".into(), serde_json::Value::from(note));
            }
        }
        answer
    }

    /// 把右欄的內容換成歌單在檔案裡的現況，選取與播放位置盡量留在原處。
    fn sync_open_playlist(&mut self) {
        let View::Playlist(name) = self.view.clone() else {
            return;
        };
        let Some(playlist) = self.library.find(&name) else {
            // 歌單被別的行程刪掉了，右欄留著目前內容，只改提示。
            self.status = format!("歌單「{name}」已經不在檔案裡");
            return;
        };

        let playing_id = self.now.as_ref().map(|n| n.video.id.clone());
        let selected_id = self
            .track_state
            .selected()
            .and_then(|i| self.tracks.get(i))
            .map(|t| t.id.clone());

        self.tracks = playlist.tracks.clone();
        self.track_title = playlist.name.clone();

        self.playing_index = playing_id
            .as_ref()
            .and_then(|id| self.tracks.iter().position(|t| &t.id == id));

        let next_selection = selected_id
            .as_ref()
            .and_then(|id| self.tracks.iter().position(|t| &t.id == id))
            .or(if self.tracks.is_empty() { None } else { Some(0) });
        self.track_state.select(next_selection);
        self.pending_confirm = None;
    }

    /// 檔案被別的行程改過就重讀，左欄立刻反映，開著的歌單也跟著更新。
    fn refresh_library(&mut self) {
        if !self.library.reload_if_changed() {
            return;
        }

        if self.library.playlists.is_empty() {
            self.playlist_state.select(None);
        } else {
            let index = self
                .playlist_state
                .selected()
                .unwrap_or(0)
                .min(self.library.playlists.len() - 1);
            self.playlist_state.select(Some(index));
        }

        if matches!(self.view, View::Playlist(_)) {
            self.sync_open_playlist();
        }
        log::info!("歌單檔案被外部改動，已重新載入");
    }

    // ---------- 歌單編輯 ----------

    fn add_to_playlist(&mut self) {
        let (Some(track_index), Some(playlist_index)) =
            (self.track_state.selected(), self.playlist_state.selected())
        else {
            self.status = "請先選好左欄的歌單與右欄的曲目".into();
            return;
        };
        let Some(track) = self.tracks.get(track_index).cloned() else {
            return;
        };
        let Some(name) = self
            .library
            .playlists
            .get(playlist_index)
            .map(|p| p.name.clone())
        else {
            return;
        };

        let already = self
            .library
            .playlists
            .get(playlist_index)
            .map(|p| p.tracks.iter().any(|t| t.id == track.id))
            .unwrap_or(false);
        if already {
            self.status = format!("「{}」已經在這份歌單裡", track.title);
            return;
        }

        let target = name.clone();
        let video = track.clone();
        // 先重讀檔案再寫回，命令列或 MCP 期間加進去的曲目不會被蓋掉。
        let result = self
            .library
            .mutate(move |lib| lib.add_track(&target, video).map(|_| ()));
        match result {
            Ok(()) => {
                self.status = format!("已加入「{name}」：{}", track.title);
                self.sync_open_playlist();
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    /// `d` 的入口：歌單檢視從歌單刪掉，電台或搜尋結果則從目前的清單拿掉。
    fn remove_selected(&mut self) {
        match self.view {
            View::Playlist(_) => self.remove_from_playlist(),
            View::Search => {
                let index = self.track_state.selected();
                self.drop_from_queue(index);
            }
        }
    }

    /// 從目前清單拿掉一首。正在播的那首被拿掉時，原位就是下一首，直接接著播。
    fn drop_from_queue(&mut self, index: Option<usize>) {
        let Some(index) = index else {
            self.set_status("沒有選到曲目".into());
            return;
        };
        let Some(track) = self.tracks.get(index).cloned() else {
            return;
        };
        self.tracks.remove(index);

        match self.playing_index {
            Some(current) if current == index => {
                if index < self.tracks.len() {
                    self.play_index(index);
                } else {
                    self.audio.stop();
                    self.now = None;
                    self.playing_index = None;
                }
            }
            Some(current) if current > index => self.playing_index = Some(current - 1),
            _ => {}
        }

        if self.tracks.is_empty() {
            self.track_state.select(None);
        } else {
            self.track_state
                .select(Some(index.min(self.tracks.len() - 1)));
        }
        self.set_status(format!("已從清單移除：{}", track.title));
    }

    fn remove_from_playlist(&mut self) {
        let View::Playlist(name) = self.view.clone() else {
            self.status = "只有在歌單檢視裡才能移除曲目".into();
            return;
        };
        let Some(track_index) = self.track_state.selected() else {
            return;
        };
        let Some(track) = self.tracks.get(track_index).cloned() else {
            return;
        };

        let target = name.clone();
        let video_id = track.id.clone();
        let result = self
            .library
            .mutate(move |lib| lib.remove_track(&target, &video_id).map(|_| ()));
        match result {
            Ok(()) => {
                self.tracks.remove(track_index);
                if self.tracks.is_empty() {
                    self.track_state.select(None);
                } else {
                    self.track_state
                        .select(Some(track_index.min(self.tracks.len() - 1)));
                }
                self.status = format!("已移除：{}", track.title);
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    fn create_playlist(&mut self) {
        let name = self.input.trim().to_string();
        if name.is_empty() {
            return;
        }
        let target = name.clone();
        let result = self.library.mutate(move |lib| {
            lib.create(&target);
            Ok(())
        });
        match result {
            Ok(()) => {
                self.status = format!("已建立歌單「{name}」");
                let index = self
                    .library
                    .playlists
                    .iter()
                    .position(|p| p.name == name)
                    .unwrap_or_else(|| self.library.playlists.len().saturating_sub(1));
                self.playlist_state.select(Some(index));
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    // ---------- 播放 ----------

    /// 判斷這首是不是長到不像單曲，回傳攔下來的理由。
    fn too_long_reason(video: &Video) -> Option<String> {
        if video.duration_ms == 0 {
            return Some("長度未知，可能是直播".into());
        }
        if video.duration_ms > LONG_TRACK_MS {
            return Some(format!(
                "長度 {}，可能是合輯或直播",
                format_duration(video.duration_ms)
            ));
        }
        None
    }

    /// 使用者主動要播某一首。過長的曲目會先提示，再按一次同一首才真的播。
    fn request_play(&mut self, index: usize) {
        let Some(video) = self.tracks.get(index).cloned() else {
            return;
        };
        if let Some(reason) = Self::too_long_reason(&video)
            && self.pending_confirm != Some(index) {
                self.pending_confirm = Some(index);
                self.status = format!("{reason}。再按一次 Enter 確認播放。");
                return;
            }
        self.pending_confirm = None;
        self.play_index(index);
    }

    fn play_index(&mut self, index: usize) {
        let Some(video) = self.tracks.get(index).cloned() else {
            return;
        };
        self.playing_index = Some(index);
        crate::history::record(crate::history::Kind::History, &video);
        self.now = Some(NowPlaying {
            video: video.clone(),
            position_ms: 0,
            marked_at: Instant::now(),
            paused: false,
            buffering: true,
        });
        self.apply_gain();
        self.load_cover(video.id.clone());

        let tx = self.tx.clone();
        let id = video.id.clone();
        tokio::spawn(async move {
            let msg = match cache::ensure(&id).await {
                Ok(path) => Msg::Ready { video_id: id, path },
                Err(e) => Msg::Error(e.to_string()),
            };
            let _ = tx.send(msg);
        });
    }

    /// 背景把下一首也先抓下來，換曲時就不必等待。抓完順便量響度，
    /// 這樣輪到它播的時候增益已經備妥，不會在開頭調整音量。
    fn prefetch_next(&self) {
        let Some(current) = self.playing_index else {
            return;
        };
        let Some(next) = self.tracks.get(current + 1) else {
            return;
        };
        let id = next.id.clone();
        let known = self.loudness.contains_key(&id);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let Ok(path) = cache::ensure(&id).await else {
                return;
            };
            if !known {
                spawn_measure(tx, id, path);
            }
        });
    }

    /// 依目前這首的量測結果設定增益。沒量過、或平衡關掉時回到原音量。
    fn apply_gain(&mut self) {
        let factor = if self.normalize {
            self.now
                .as_ref()
                .and_then(|now| self.loudness.get(&now.video.id))
                .map(|m| m.gain_factor())
                .unwrap_or(1.0)
        } else {
            1.0
        };
        self.audio.set_gain(factor);
    }

    /// 目前套用的增益，單位 dB。
    pub fn gain_db(&self) -> f64 {
        20.0 * (self.audio.gain() as f64).log10()
    }

    /// 這一首有沒有量測結果可用。
    pub fn gain_ready(&self) -> bool {
        self.now
            .as_ref()
            .is_some_and(|now| self.loudness.contains_key(&now.video.id))
    }

    fn toggle_normalize(&mut self) {
        self.normalize = !self.normalize;
        self.apply_gain();
        let text = self.normalize_status();
        self.set_status(text);
    }

    /// 往前或往後換曲。自動接續時會跳過過長的曲目，避免卡在數小時的合輯上。
    fn play_offset(&mut self, offset: isize) {
        let Some(current) = self.playing_index else {
            return;
        };
        let mut candidate = current as isize;
        let mut skipped = 0usize;

        loop {
            candidate += offset;
            if candidate < 0 || candidate as usize >= self.tracks.len() {
                self.status = "清單已經到底了".into();
                return;
            }
            let index = candidate as usize;
            let Some(video) = self.tracks.get(index) else {
                return;
            };
            if Self::too_long_reason(video).is_none() {
                if skipped > 0 {
                    log::info!("自動接續時跳過 {skipped} 首過長的曲目");
                }
                self.play_index(index);
                return;
            }
            skipped += 1;
            if skipped >= MAX_AUTO_SKIP {
                self.status = format!("連續 {skipped} 首都過長，已停止自動接續");
                return;
            }
        }
    }

    fn toggle_pause(&mut self) {
        let Some(now) = self.now.as_mut() else {
            return;
        };
        if now.buffering {
            return;
        }
        if now.paused {
            now.paused = false;
            now.marked_at = Instant::now();
            self.audio.play();
        } else {
            now.position_ms = now.position_ms();
            now.paused = true;
            self.audio.pause();
        }
    }

    fn seek_by(&mut self, delta_ms: i64) {
        let Some(now) = self.now.as_mut() else {
            return;
        };
        if now.buffering {
            return;
        }
        let target = (now.position_ms() as i64 + delta_ms).clamp(0, now.video.duration_ms as i64);
        now.position_ms = target as u32;
        now.marked_at = Instant::now();
        self.audio.seek(target as u32);
    }

    fn change_volume(&mut self, up: bool) {
        let next = if up {
            self.audio.volume() + VOLUME_STEP
        } else {
            self.audio.volume() - VOLUME_STEP
        };
        self.audio.set_volume(next);
        self.status = format!("音量 {}%", self.volume_percent());
    }

    // ---------- 清單移動 ----------

    fn move_selection(&mut self, delta: isize) {
        let (len, state) = match self.focus {
            Focus::Playlists => (self.library.playlists.len(), &mut self.playlist_state),
            Focus::Tracks => (self.tracks.len(), &mut self.track_state),
        };
        if len == 0 {
            return;
        }
        let current = state.selected().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, len as isize - 1);
        state.select(Some(next as usize));
        self.pending_confirm = None;
    }

    fn jump_selection(&mut self, to_end: bool) {
        let (len, state) = match self.focus {
            Focus::Playlists => (self.library.playlists.len(), &mut self.playlist_state),
            Focus::Tracks => (self.tracks.len(), &mut self.track_state),
        };
        if len == 0 {
            return;
        }
        state.select(Some(if to_end { len - 1 } else { 0 }));
    }

    // ---------- 訊息處理 ----------

    pub fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Tick => {
                self.refresh_library();
                self.refresh_theme();
            }
            Msg::Control { control, reply } => {
                let answer = self.control(control);
                let _ = reply.send(answer);
            }
            Msg::Key(key) => self.handle_key(key),
            Msg::Audio(event) => self.handle_audio(event),
            Msg::Results { title, tracks } => {
                self.status = format!("找到 {} 筆", tracks.len());
                self.track_title = title;
                self.tracks = tracks;
                self.view = View::Search;
                self.playing_index = None;
                self.track_state
                    .select(if self.tracks.is_empty() { None } else { Some(0) });
                self.focus = Focus::Tracks;
            }
            Msg::Radio {
                title,
                tracks,
                continue_current,
                seed,
            } => {
                if tracks.len() < 2 {
                    self.status = "電台沒有回傳其他曲目".into();
                    return;
                }
                self.set_status(format!("電台接著還有 {} 首", tracks.len() - 1));
                self.remember_seed(Some(seed));
                self.track_title = title;
                self.tracks = tracks;
                self.view = View::Search;
                self.track_state.select(Some(0));
                self.focus = Focus::Tracks;

                if continue_current {
                    // 這首已經在放了，只要把它對應到新清單的第一位，放完自然接下一首。
                    self.playing_index = Some(0);
                    self.prefetch_next();
                } else {
                    self.play_index(0);
                }
            }
            Msg::Ready { video_id, path } => {
                let path_for_measure = path.clone();
                let matches_current = self
                    .now
                    .as_ref()
                    .map(|n| n.video.id == video_id)
                    .unwrap_or(false);
                if !matches_current {
                    return;
                }
                if let Some(now) = self.now.as_mut() {
                    now.buffering = false;
                    now.position_ms = 0;
                    now.marked_at = Instant::now();
                }
                self.audio.load(path);
                self.apply_gain();
                if !self.loudness.contains_key(&video_id) {
                    spawn_measure(self.tx.clone(), video_id, path_for_measure);
                }
                self.prefetch_next();
            }
            Msg::SeedTags { video_id, tags } => {
                if self.seed.as_ref().is_some_and(|seed| seed.id == video_id) {
                    self.seed_tags = tags;
                }
            }
            Msg::Loudness {
                video_id,
                measurement,
            } => {
                log::info!(
                    "{video_id}：{:.1} LUFS，真實峰值 {:.2} dBTP，增益 {:+.1} dB",
                    measurement.lufs,
                    measurement.peak_dbtp,
                    measurement.gain_db()
                );
                self.loudness.insert(video_id.clone(), measurement);
                if self
                    .now
                    .as_ref()
                    .is_some_and(|now| now.video.id == video_id)
                {
                    self.apply_gain();
                }
            }
            Msg::Cover { video_id, image } => {
                if self
                    .now
                    .as_ref()
                    .is_some_and(|now| now.video.id == video_id)
                {
                    self.cover = Some(*image);
                }
            }
            Msg::Error(text) => {
                self.status = format!("錯誤：{text}");
                if let Some(now) = self.now.as_mut() {
                    now.buffering = false;
                }
            }
        }
    }

    fn handle_audio(&mut self, event: AudioEvent) {
        match event {
            AudioEvent::Position(position_ms) => {
                if let Some(now) = self.now.as_mut() {
                    now.position_ms = position_ms;
                    now.marked_at = Instant::now();
                    now.paused = false;
                }
            }
            AudioEvent::Ended => {
                log::info!("播放結束，換下一首");
                self.play_offset(1);
            }
            AudioEvent::Error(text) => self.status = format!("播放錯誤：{text}"),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.should_quit = true;
            return;
        }

        if self.mode != Mode::Browse {
            match key.code {
                KeyCode::Esc => {
                    self.mode = Mode::Browse;
                    self.status = "已取消輸入".into();
                }
                KeyCode::Enter => {
                    let mode = self.mode;
                    self.mode = Mode::Browse;
                    match mode {
                        Mode::Search => self.run_search(),
                        Mode::NewPlaylist => self.create_playlist(),
                        Mode::Browse => {}
                    }
                }
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Char(c) => self.input.push(c),
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('/') => {
                self.mode = Mode::Search;
                self.input.clear();
                self.status = "輸入關鍵字後按 Enter 搜尋".into();
            }
            KeyCode::Char('N') => {
                self.mode = Mode::NewPlaylist;
                self.input.clear();
                self.status = "輸入新歌單名稱後按 Enter".into();
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Playlists => Focus::Tracks,
                    Focus::Tracks => Focus::Playlists,
                };
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::Home => self.jump_selection(false),
            KeyCode::End => self.jump_selection(true),
            KeyCode::Enter => match self.focus {
                Focus::Playlists => self.open_selected_playlist(),
                Focus::Tracks => {
                    if let Some(index) = self.track_state.selected() {
                        self.request_play(index);
                    }
                }
            },
            KeyCode::Char(' ') => self.toggle_pause(),
            KeyCode::Char('n') => self.play_offset(1),
            KeyCode::Char('b') => self.play_offset(-1),
            KeyCode::Right => self.seek_by(SEEK_STEP_MS as i64),
            KeyCode::Left => self.seek_by(-(SEEK_STEP_MS as i64)),
            KeyCode::Char('g') => self.toggle_normalize(),
            KeyCode::Char('i') => self.toggle_cover(),
            KeyCode::Char('+') | KeyCode::Char('=') => self.change_volume(true),
            KeyCode::Char('-') => self.change_volume(false),
            KeyCode::Char('r') => self.start_radio(),
            KeyCode::Char('a') => self.add_to_playlist(),
            KeyCode::Char('d') => self.remove_selected(),
            KeyCode::Char('s') => {
                self.audio.stop();
                self.now = None;
                self.status = "已停止".into();
            }
            _ => {}
        }
    }
}

/// 毫秒轉成 m:ss。
pub fn format_duration(ms: u32) -> String {
    let total = Duration::from_millis(ms as u64).as_secs();
    format!("{}:{:02}", total / 60, total % 60)
}

/// 在背景量一首的響度，量完寫檔並回報。量測會卡住執行緒，所以放在 blocking 池。
fn spawn_measure(tx: UnboundedSender<Msg>, video_id: String, path: PathBuf) {
    tokio::task::spawn_blocking(move || match loudness::measure(&path) {
        Ok(measurement) => {
            loudness::save(&video_id, measurement);
            let _ = tx.send(Msg::Loudness {
                video_id,
                measurement,
            });
        }
        Err(e) => log::warn!("量測 {video_id} 的響度失敗：{e}"),
    });
}
