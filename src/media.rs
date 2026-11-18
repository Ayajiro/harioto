//! 多媒體鍵：鍵盤上的 ⏯ ⏭ ⏮，以及藍牙耳機上的按鍵。
//!
//! 動作本身在 `control::Control` 裡已經有了，這裡只負責把系統送來的按鍵變成
//! 那些指令，順便把正在播的曲名交給系統，讓控制中心的「播放中」那一欄有東西。
//!
//! 目前只做 macOS。三個平台的多媒體鍵是三套互不相干的機制——macOS 的
//! `MPRemoteCommandCenter`、Linux 的 MPRIS、Windows 的 `SystemMediaTransportControls`
//! ——沒有一個像 cpal 那樣能一次涵蓋，所以另外兩個分開再做。其他平台上
//! [`host`] 直接把播放器跑起來，[`connect`] 回傳一個什麼都不做的把手，
//! 呼叫端不必分平台。

use crate::app::{Msg, NowPlaying};
use std::sync::OnceLock;
use std::time::Instant;
use tokio::sync::mpsc::UnboundedSender;

/// 位置和系統自己推算出來的差這麼多，就補送一次。只有跳轉會跨過這條線：
/// 正常播放時系統按播放速率自己往前走，不需要我們每秒提醒。
const DRIFT_MS: u32 = 2_000;

/// 按鍵要送去的地方。播放器起來之後才有，所以放著等 [`connect`] 填。
static EVENTS: OnceLock<UnboundedSender<Msg>> = OnceLock::new();

/// 狀態要送去的地方，另一端在主執行緒上。沒接上多媒體鍵時這裡是空的。
static UPDATES: OnceLock<std::sync::mpsc::Sender<State>> = OnceLock::new();

/// 要讓系統顯示的曲目。欄位就是控制中心會畫出來的那幾個。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Track {
    pub title: String,
    pub channel: String,
    pub duration_ms: u32,
    pub position_ms: u32,
}

/// 送到主執行緒上的狀態。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum State {
    Stopped,
    Playing(Track),
    Paused(Track),
}

/// 上一次送出去的東西，用來判斷這一次還要不要送。
struct Mark {
    id: String,
    paused: bool,
    position_ms: u32,
    at: Instant,
}

impl Mark {
    /// 真的要再送一次嗎。換歌、暫停與否變了，或位置和系統推算出來的對不上。
    fn worth_sending(&self, fresh: &Mark) -> bool {
        if self.id != fresh.id || self.paused != fresh.paused {
            return true;
        }
        let expected = if self.paused {
            self.position_ms
        } else {
            let elapsed = fresh.at.duration_since(self.at).as_millis() as u32;
            self.position_ms.saturating_add(elapsed)
        };
        expected.abs_diff(fresh.position_ms) > DRIFT_MS
    }
}

/// 多媒體鍵的把手。接不上或關掉時是一個不做事的空殼。
pub struct Keys {
    sender: Option<std::sync::mpsc::Sender<State>>,
    last: Option<Mark>,
}

impl Keys {
    /// 把現在的狀態交給系統。重複的狀態不送。
    pub fn publish(&mut self, now: Option<&NowPlaying>) {
        let Some(sender) = self.sender.as_ref() else {
            return;
        };

        let Some(now) = now else {
            if self.last.take().is_some() {
                let _ = sender.send(State::Stopped);
            }
            return;
        };

        // 緩衝中的曲目畫面上是停著的，系統看到的也應該一樣。
        let paused = now.paused || now.buffering;
        let position_ms = now.position_ms();
        let fresh = Mark {
            id: now.video.id.clone(),
            paused,
            position_ms,
            at: Instant::now(),
        };

        if let Some(last) = self.last.as_ref()
            && !last.worth_sending(&fresh)
        {
            return;
        }

        let track = Track {
            title: now.video.title.clone(),
            channel: now.video.channel.clone(),
            duration_ms: now.video.duration_ms,
            position_ms,
        };
        let state = if paused {
            State::Paused(track)
        } else {
            State::Playing(track)
        };
        if sender.send(state).is_ok() {
            self.last = Some(fresh);
        }
    }
}

/// 播放器起來之後接上多媒體鍵。回傳的把手負責讓系統的「播放中」跟上。
pub fn connect(events: UnboundedSender<Msg>) -> Keys {
    let _ = EVENTS.set(events);
    Keys {
        sender: UPDATES.get().cloned(),
        last: None,
    }
}

/// 設定檔裡的 `media_keys: false` 把這件事整個關掉。
///
/// 值得有這個開關，是因為接上多媒體鍵同時會把系統「播放中」那一欄接過來：
/// 原本停在 Music 或瀏覽器上的那一欄會變成 harioto，而那不是每個人都想要的。
fn allowed_by_file() -> Option<bool> {
    #[derive(serde::Deserialize)]
    struct OnlyThis {
        media_keys: Option<bool>,
    }
    let text = std::fs::read_to_string(crate::theme::path()).ok()?;
    serde_json::from_str::<OnlyThis>(&text).ok()?.media_keys
}

/// 把主執行緒交給系統，播放器搬到另一條執行緒上跑。
///
/// 順序只能是這樣。macOS 只把多媒體鍵送到**主執行緒**的執行迴圈上：同一套
/// 程式碼放到別條執行緒去開，`MPRemoteCommandCenter` 照樣回報註冊成功，按鍵
/// 卻一個都不會到，而且不會有任何錯誤。這是拿一支獨立的小程式把兩種擺法都
/// 跑過才確定的。
///
/// 接不上、關掉、或平台還沒做的時候，播放器就在原本的主執行緒上跑。
pub fn host<F>(turned_off_by_flag: bool, player: F) -> anyhow::Result<()>
where
    F: FnOnce() -> anyhow::Result<()> + Send + 'static,
{
    if turned_off_by_flag || allowed_by_file() == Some(false) {
        return player();
    }
    platform::host(player)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{EVENTS, State, UPDATES};
    use crate::app::Msg;
    use crate::control::Control;
    use souvlaki::{
        MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition,
        PlatformConfig,
    };
    use std::ffi::{CStr, c_char, c_void};
    use std::time::Duration;

    /// 主執行緒每一趟在執行迴圈裡停留這麼久。按鍵是系統透過執行迴圈送進來的，
    /// 這個數字就是按下去到有反應之間的上限。
    const TICK: f64 = 0.1;

    pub fn host<F>(player: F) -> anyhow::Result<()>
    where
        F: FnOnce() -> anyhow::Result<()> + Send + 'static,
    {
        init_app();

        let mut controls = match MediaControls::new(PlatformConfig {
            display_name: "harioto",
            dbus_name: "harioto",
            hwnd: None,
        }) {
            Ok(controls) => controls,
            Err(e) => {
                log::warn!("the media controls would not open: {e}");
                return player();
            }
        };

        if let Err(e) = controls.attach(|event| {
            let Some(control) = translate(event) else {
                return;
            };
            if let Some(events) = EVENTS.get() {
                let _ = events.send(Msg::Media(control));
            }
        }) {
            log::warn!("the media keys would not attach: {e}");
            return player();
        }

        let (tx, rx) = std::sync::mpsc::channel();
        if UPDATES.set(tx).is_err() {
            log::warn!("the media keys were already attached once");
        }

        let thread = std::thread::Builder::new()
            .name("player".into())
            .spawn(player)?;

        // 問執行緒本人結束了沒，而不是讓它自己回報：播放器萬一恐慌，回報那一行
        // 就不會跑到，主執行緒會在這裡轉到天荒地老。
        while !thread.is_finished() {
            // 事件是系統透過執行迴圈送進來的，所以主執行緒要一直讓它跑。
            pump();
            while let Ok(state) = rx.try_recv() {
                apply(&mut controls, state);
            }
        }

        match thread.join() {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!("the player thread stopped unexpectedly")),
        }
    }

    // NSApplication 與執行迴圈不是 souvlaki 的一部分，要自己開，所以這裡直接
    // 叫 Objective-C 執行期。只有四個呼叫，不值得為它們多一個套件。
    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {}
    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {}

    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        /// 真正的型別由每個訊息自己決定，所以每次呼叫前都要轉成對的那一個。
        /// arm64 沒有可變參數版本可用，轉錯型別就是未定義行為。
        fn objc_msgSend();
    }

    fn class(name: &CStr) -> *mut c_void {
        unsafe { objc_getClass(name.as_ptr()) }
    }

    fn selector(name: &CStr) -> *mut c_void {
        unsafe { sel_registerName(name.as_ptr()) }
    }

    /// 不帶參數、回傳物件的訊息。
    unsafe fn send(target: *mut c_void, selector: *mut c_void) -> *mut c_void {
        unsafe {
            let send: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
                std::mem::transmute(objc_msgSend as *const ());
            send(target, selector)
        }
    }

    /// `MPRemoteCommandCenter` 只對有 NSApplication 的行程送事件，沒有的話按鍵
    /// 會越過 harioto 直接落到 Music。這裡用 Prohibited 政策開一個：不進 Dock、
    /// 沒有選單列，但系統認得這個行程。
    fn init_app() {
        // NSApplicationActivationPolicyProhibited
        const PROHIBITED: isize = 2;
        unsafe {
            let app = send(class(c"NSApplication"), selector(c"sharedApplication"));
            let set: unsafe extern "C" fn(*mut c_void, *mut c_void, isize) -> i8 =
                std::mem::transmute(objc_msgSend as *const ());
            set(app, selector(c"setActivationPolicy:"), PROHIBITED);
        }
    }

    /// 讓主執行緒的執行迴圈跑一下，系統才送得出事件。
    fn pump() {
        unsafe {
            let run_loop = send(class(c"NSRunLoop"), selector(c"currentRunLoop"));

            let with_seconds: unsafe extern "C" fn(*mut c_void, *mut c_void, f64) -> *mut c_void =
                std::mem::transmute(objc_msgSend as *const ());
            let until = with_seconds(
                class(c"NSDate"),
                selector(c"dateWithTimeIntervalSinceNow:"),
                TICK,
            );

            let run: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) =
                std::mem::transmute(objc_msgSend as *const ());
            run(run_loop, selector(c"runUntilDate:"), until);
        }
    }

    /// 系統的事件換成播放器本來就懂的指令。不認得的就不動作。
    fn translate(event: MediaControlEvent) -> Option<Control> {
        let (name, control) = match event {
            MediaControlEvent::Play => ("play", Control::Resume),
            MediaControlEvent::Pause => ("pause", Control::Pause),
            MediaControlEvent::Toggle => ("toggle", Control::Toggle),
            MediaControlEvent::Next => ("next", Control::Next),
            MediaControlEvent::Previous => ("previous", Control::Prev),
            MediaControlEvent::Stop => ("stop", Control::Stop),
            _ => return None,
        };
        log::info!("media key: {name}");
        Some(control)
    }

    fn apply(controls: &mut MediaControls, state: State) {
        let track = match &state {
            State::Stopped => None,
            State::Playing(track) | State::Paused(track) => Some(track),
        };

        if let Some(track) = track {
            let _ = controls.set_metadata(MediaMetadata {
                title: Some(&track.title),
                artist: Some(&track.channel),
                // 長度不明的時候不要送 0，那會讓控制中心畫出一條走完的進度條。
                duration: (track.duration_ms > 0)
                    .then(|| Duration::from_millis(u64::from(track.duration_ms))),
                ..Default::default()
            });
        }

        let progress =
            track.map(|track| MediaPosition(Duration::from_millis(u64::from(track.position_ms))));
        let _ = controls.set_playback(match state {
            State::Stopped => MediaPlayback::Stopped,
            State::Playing(_) => MediaPlayback::Playing { progress },
            State::Paused(_) => MediaPlayback::Paused { progress },
        });
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn host<F>(player: F) -> anyhow::Result<()>
    where
        F: FnOnce() -> anyhow::Result<()> + Send + 'static,
    {
        player()
    }
}

#[cfg(test)]
mod tests {
    use super::{DRIFT_MS, Mark};
    use std::time::{Duration, Instant};

    fn mark(id: &str, paused: bool, position_ms: u32, at: Instant) -> Mark {
        Mark {
            id: id.into(),
            paused,
            position_ms,
            at,
        }
    }

    #[test]
    fn a_new_track_is_worth_sending() {
        let now = Instant::now();
        let before = mark("aaa", false, 10_000, now);
        let after = mark("bbb", false, 0, now);
        assert!(before.worth_sending(&after));
    }

    #[test]
    fn pausing_is_worth_sending() {
        let now = Instant::now();
        let before = mark("aaa", false, 10_000, now);
        let after = mark("aaa", true, 10_000, now);
        assert!(before.worth_sending(&after));
    }

    #[test]
    fn the_same_track_running_on_is_not_worth_sending() {
        let now = Instant::now();
        let before = mark("aaa", false, 10_000, now - Duration::from_secs(5));
        // 系統自己會從 10 秒往前走五秒，所以 15 秒正是它算出來的那個數字。
        let after = mark("aaa", false, 15_000, now);
        assert!(!before.worth_sending(&after));
    }

    #[test]
    fn a_seek_is_worth_sending() {
        let now = Instant::now();
        let before = mark("aaa", false, 10_000, now - Duration::from_secs(5));
        let after = mark("aaa", false, 15_000 + DRIFT_MS + 1, now);
        assert!(before.worth_sending(&after));
    }

    #[test]
    fn a_paused_track_staying_put_is_not_worth_sending() {
        let now = Instant::now();
        let before = mark("aaa", true, 10_000, now - Duration::from_secs(30));
        let after = mark("aaa", true, 10_000, now);
        assert!(!before.worth_sending(&after));
    }
}
