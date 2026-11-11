//! 應用程式狀態與按鍵處理。

use crate::{
    audio::{AudioEvent, AudioPlayer},
    cache,
    store::Library,
    youtube::{self, Video},
};
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    widgets::ListState,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::sync::mpsc::UnboundedSender;

const SEEK_STEP_MS: u32 = 5_000;
const VOLUME_STEP: f32 = 0.05;
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
    },
    Ready { video_id: String, path: PathBuf },
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
    Playlist(usize),
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
}

impl App {
    pub fn new(mut audio: AudioPlayer, tx: UnboundedSender<Msg>, note: Option<String>) -> Self {
        audio.set_volume(0.5);
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
        }
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
        self.view = View::Playlist(index);
        self.playing_index = None;
        self.track_state
            .select(if self.tracks.is_empty() { None } else { Some(0) });
        self.focus = Focus::Tracks;
        self.status = format!("{}：{} 首", self.track_title, self.tracks.len());
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
        let Some(playlist) = self.library.playlists.get_mut(playlist_index) else {
            return;
        };
        if playlist.tracks.iter().any(|t| t.id == track.id) {
            self.status = format!("「{}」已經在這份歌單裡", track.title);
            return;
        }
        playlist.tracks.push(track.clone());
        let name = playlist.name.clone();
        match self.library.save() {
            Ok(()) => self.status = format!("已加入「{name}」：{}", track.title),
            Err(e) => self.status = e.to_string(),
        }
    }

    fn remove_from_playlist(&mut self) {
        let View::Playlist(playlist_index) = self.view.clone() else {
            self.status = "只有在歌單檢視裡才能移除曲目".into();
            return;
        };
        let Some(track_index) = self.track_state.selected() else {
            return;
        };
        let Some(playlist) = self.library.playlists.get_mut(playlist_index) else {
            return;
        };
        if track_index >= playlist.tracks.len() {
            return;
        }
        let removed = playlist.tracks.remove(track_index);
        let _ = self.library.save();
        self.tracks.remove(track_index);
        if self.tracks.is_empty() {
            self.track_state.select(None);
        } else {
            self.track_state
                .select(Some(track_index.min(self.tracks.len() - 1)));
        }
        self.status = format!("已移除：{}", removed.title);
    }

    fn create_playlist(&mut self) {
        let name = self.input.trim().to_string();
        if name.is_empty() {
            return;
        }
        self.library.create(&name);
        match self.library.save() {
            Ok(()) => {
                self.status = format!("已建立歌單「{name}」");
                let last = self.library.playlists.len().saturating_sub(1);
                self.playlist_state.select(Some(last));
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
        self.status = format!("緩衝中：{}", video.title);
        crate::history::record(crate::history::Kind::History, &video);
        self.now = Some(NowPlaying {
            video: video.clone(),
            position_ms: 0,
            marked_at: Instant::now(),
            paused: false,
            buffering: true,
        });

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

    /// 背景把下一首也先抓下來，換曲時就不必等待。
    fn prefetch_next(&self) {
        let Some(current) = self.playing_index else {
            return;
        };
        let Some(next) = self.tracks.get(current + 1) else {
            return;
        };
        let id = next.id.clone();
        tokio::spawn(async move {
            let _ = cache::ensure(&id).await;
        });
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
            Msg::Tick => {}
            Msg::Key(key) => self.handle_key(key),
            Msg::Audio(event) => self.handle_audio(event),
            Msg::Results { title, tracks } => {
                self.status = format!("{title}：{} 筆", tracks.len());
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
            } => {
                if tracks.len() < 2 {
                    self.status = "電台沒有回傳其他曲目".into();
                    return;
                }
                self.status = format!("{title}：接著還有 {} 首", tracks.len() - 1);
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
                    self.status = format!("播放：{} — {}", now.video.channel, now.video.title);
                }
                self.audio.load(path);
                self.prefetch_next();
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
            KeyCode::Char('+') | KeyCode::Char('=') => self.change_volume(true),
            KeyCode::Char('-') => self.change_volume(false),
            KeyCode::Char('r') => self.start_radio(),
            KeyCode::Char('a') => self.add_to_playlist(),
            KeyCode::Char('d') => self.remove_from_playlist(),
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
