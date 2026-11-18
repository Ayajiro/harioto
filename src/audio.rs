//! 音訊播放：獨立執行緒持有 rodio 的輸出與播放佇列。
//!
//! 解碼交給 rodio 內建的 symphonia，支援 m4a、mp3、flac、vorbis、wav。
//!
//! 輸出裝置會在播放途中消失——藍牙耳機走遠、有線耳機被拔掉、使用者在系統設定裡
//! 改了預設輸出。這個模組每秒問一次預設輸出是誰，順便收下 cpal 自己回報的錯誤，
//! 換裝置時把同一首接回原本的位置並停在暫停。

use anyhow::{Result, anyhow};
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use std::{
    path::PathBuf,
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread,
    time::Duration,
};
use tokio::sync::mpsc::UnboundedSender;

/// 位置回報的間隔。
const TICK: Duration = Duration::from_millis(200);

/// 多久確認一次預設輸出裝置。這是問作業系統而不是讀記憶體，所以比位置回報稀疏。
/// 耳機斷線到暫停之間最多差這麼久。
const DEVICE_CHECK: Duration = Duration::from_secs(1);

pub enum AudioCommand {
    Load(PathBuf),
    Play,
    Pause,
    Seek(u32),
    Volume(f32),
    Stop,
    /// cpal 從它自己的執行緒回報輸出出事了，通常是裝置被拔掉。
    DeviceLost,
}

#[derive(Debug)]
pub enum AudioEvent {
    /// 目前播放位置，單位毫秒。
    Position(u32),
    /// 這一首放完了。
    Ended,
    Error(String),
    /// 換到別的輸出裝置了，播放已經停在原處。帶的是新裝置的名字。
    OutputChanged(Option<String>),
}

/// 目前預設輸出裝置的名字。問不到就回 None。
fn default_output_name() -> Option<String> {
    rodio::cpal::default_host()
        .default_output_device()
        .and_then(|device| device.name().ok())
}

/// 值不值得為了這個答案換一條串流。
///
/// 問不到新裝置的名字時不動手：那通常表示系統正在換，這一刻開串流只會失敗，
/// 而且每秒重試一次。
fn should_switch(before: &Option<String>, after: &Option<String>) -> bool {
    after.is_some() && before != after
}

/// 音訊執行緒握著的東西。換裝置時整組要一起重建，所以放在一起。
struct Output {
    stream: rodio::OutputStream,
    sink: rodio::Sink,
    /// 開這條串流時的預設輸出裝置名字，用來比對有沒有換。
    device: Option<String>,
    /// 送進 sink 的音量。重建之後 sink 回到 1.0，要自己補回去。
    volume: f32,
    /// 正在播的檔案，換裝置時要接回去。
    current: Option<PathBuf>,
    /// 載入過曲目之後才需要判斷播放結束，避免一啟動就誤報。
    loaded: bool,
}

impl Output {
    /// 在目前的預設輸出裝置上開一條串流。
    fn open(errors: &Sender<AudioCommand>) -> Result<Self, String> {
        let device = default_output_name();
        let notify = errors.clone();
        let mut stream = rodio::OutputStreamBuilder::from_default_device()
            .map_err(|e| e.to_string())?
            .with_error_callback(move |e| {
                // 這個回呼跑在 cpal 自己的執行緒上，所以只轉達，不在這裡動手。
                log::warn!("the audio output reported an error: {e}");
                let _ = notify.send(AudioCommand::DeviceLost);
            })
            .open_stream()
            .map_err(|e| e.to_string())?;
        // 丟掉串流時 rodio 會往終端機印一行字，那會把 TUI 的畫面弄花。
        stream.log_on_drop(false);
        let sink = rodio::Sink::connect_new(stream.mixer());
        Ok(Self {
            stream,
            sink,
            device,
            volume: 1.0,
            current: None,
            loaded: false,
        })
    }

    /// 換到現在的預設輸出裝置，把同一首接回原本的位置，然後停在暫停。
    ///
    /// 停下來而不是繼續播，是因為耳機離開耳朵之後聲音會從別的地方出來；
    /// 接回來也不自動續播，那一刻通常正在做別的事。
    fn swap(&mut self, errors: &Sender<AudioCommand>) -> Result<(), String> {
        let position = self.sink.get_pos();
        let resume = if self.loaded {
            self.current.clone()
        } else {
            None
        };
        let fresh = Self::open(errors)?;

        self.sink = fresh.sink;
        self.stream = fresh.stream;
        self.device = fresh.device;
        self.sink.set_volume(self.volume);
        self.loaded = false;

        let Some(path) = resume else {
            self.current = None;
            return Ok(());
        };

        let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        let decoder = rodio::Decoder::try_from(file).map_err(|e| e.to_string())?;
        self.sink.append(decoder);
        self.sink.pause();
        if self.sink.try_seek(position).is_err() {
            log::warn!("the new output could not pick the track up at {position:?}");
        }
        self.loaded = true;
        Ok(())
    }
}

pub struct AudioPlayer {
    tx: Sender<AudioCommand>,
    /// 使用者設定的音量，介面顯示的就是這個值。
    volume: f32,
    /// 響度平衡算出來的倍率，乘在音量上送進輸出，不影響顯示。
    gain: f32,
}

impl AudioPlayer {
    /// 開啟預設輸出裝置並啟動播放執行緒。
    pub fn new(events: UnboundedSender<AudioEvent>) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<AudioCommand>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let errors = tx.clone();

        thread::spawn(move || {
            let mut out = match Output::open(&errors) {
                Ok(out) => out,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("could not open the audio output: {e}")));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));

            // 距離上次確認輸出裝置過了多久。
            let mut waited = Duration::ZERO;

            // 換裝置的收尾：成功就回報新的名字，失敗就說一聲並留著舊的串流。
            let announce = |out: &Output, result: Result<(), String>| match result {
                Ok(()) => {
                    log::info!("the output moved to {:?}", out.device);
                    AudioEvent::OutputChanged(out.device.clone())
                }
                Err(e) => AudioEvent::Error(format!("could not follow the audio output: {e}")),
            };

            loop {
                match rx.recv_timeout(TICK) {
                    Ok(AudioCommand::Load(path)) => {
                        out.sink.clear();
                        match std::fs::File::open(&path)
                            .map_err(|e| e.to_string())
                            .and_then(|f| rodio::Decoder::try_from(f).map_err(|e| e.to_string()))
                        {
                            Ok(decoder) => {
                                out.sink.append(decoder);
                                out.sink.play();
                                out.loaded = true;
                                out.current = Some(path);
                            }
                            Err(e) => {
                                out.loaded = false;
                                out.current = None;
                                let _ = events
                                    .send(AudioEvent::Error(format!("could not decode: {e}")));
                            }
                        }
                    }
                    Ok(AudioCommand::Play) => out.sink.play(),
                    Ok(AudioCommand::Pause) => out.sink.pause(),
                    Ok(AudioCommand::Seek(ms)) => {
                        if out.sink.try_seek(Duration::from_millis(ms as u64)).is_err() {
                            let _ = events.send(AudioEvent::Error(
                                "this format does not support seeking".into(),
                            ));
                        }
                    }
                    Ok(AudioCommand::Volume(v)) => {
                        out.volume = v;
                        out.sink.set_volume(v);
                    }
                    Ok(AudioCommand::Stop) => {
                        out.sink.clear();
                        out.loaded = false;
                        out.current = None;
                    }
                    Ok(AudioCommand::DeviceLost) => {
                        // 裝置出事時名字不一定已經變了，所以不比對就換。
                        let result = out.swap(&errors);
                        let _ = events.send(announce(&out, result));
                        waited = Duration::ZERO;
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        waited += TICK;
                        if waited >= DEVICE_CHECK {
                            waited = Duration::ZERO;
                            let found = default_output_name();
                            if should_switch(&out.device, &found) {
                                log::info!(
                                    "the default output changed from {:?} to {found:?}",
                                    out.device
                                );
                                let result = out.swap(&errors);
                                let _ = events.send(announce(&out, result));
                            }
                        }
                        if out.loaded {
                            if out.sink.empty() {
                                out.loaded = false;
                                out.current = None;
                                let _ = events.send(AudioEvent::Ended);
                            } else if !out.sink.is_paused() {
                                let position = out.sink.get_pos().as_millis() as u32;
                                let _ = events.send(AudioEvent::Position(position));
                            }
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                tx,
                volume: 0.5,
                gain: 1.0,
            }),
            Ok(Err(e)) => Err(anyhow!(e)),
            Err(_) => Err(anyhow!("the audio output did not start in time")),
        }
    }

    fn send(&self, command: AudioCommand) {
        let _ = self.tx.send(command);
    }

    pub fn load(&self, path: PathBuf) {
        self.send(AudioCommand::Load(path));
    }

    pub fn play(&self) {
        self.send(AudioCommand::Play);
    }

    pub fn pause(&self) {
        self.send(AudioCommand::Pause);
    }

    pub fn stop(&self) {
        self.send(AudioCommand::Stop);
    }

    pub fn seek(&self, position_ms: u32) {
        self.send(AudioCommand::Seek(position_ms));
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        self.push_volume();
    }

    /// 這一首的響度平衡倍率。1.0 等於不調整。
    pub fn gain(&self) -> f32 {
        self.gain
    }

    pub fn set_gain(&mut self, gain: f32) {
        self.gain = gain.clamp(0.05, 8.0);
        self.push_volume();
    }

    /// 輸出端拿到的是兩者相乘的結果，上限 1.0 以免溢出。
    fn push_volume(&self) {
        self.send(AudioCommand::Volume((self.volume * self.gain).min(1.0)));
    }
}

#[cfg(test)]
mod output_tests {
    use super::should_switch;

    fn name(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    #[test]
    fn the_same_device_is_not_a_change() {
        assert!(!should_switch(
            &name("MacBook Air Speakers"),
            &name("MacBook Air Speakers")
        ));
    }

    #[test]
    fn another_device_is_a_change() {
        assert!(should_switch(
            &name("QCY Melobuds ANC"),
            &name("MacBook Air Speakers")
        ));
    }

    #[test]
    fn a_nameless_answer_is_not_worth_switching_for() {
        // 系統正在換裝置的那一刻問不到名字，這時候開串流只會失敗，而且每秒再試一次。
        assert!(!should_switch(&name("QCY Melobuds ANC"), &None));
        assert!(!should_switch(&None, &None));
    }

    #[test]
    fn the_first_name_after_silence_is_a_change() {
        assert!(should_switch(&None, &name("MacBook Air Speakers")));
    }
}
