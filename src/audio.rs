//! 音訊播放：獨立執行緒持有 rodio 的輸出與播放佇列。
//!
//! 解碼交給 rodio 內建的 symphonia，支援 m4a、mp3、flac、vorbis、wav。

use anyhow::{Result, anyhow};
use std::{
    path::PathBuf,
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread,
    time::Duration,
};
use tokio::sync::mpsc::UnboundedSender;

/// 位置回報的間隔。
const TICK: Duration = Duration::from_millis(200);

pub enum AudioCommand {
    Load(PathBuf),
    Play,
    Pause,
    Seek(u32),
    Volume(f32),
    Stop,
}

#[derive(Debug)]
pub enum AudioEvent {
    /// 目前播放位置，單位毫秒。
    Position(u32),
    /// 這一首放完了。
    Ended,
    Error(String),
}

pub struct AudioPlayer {
    tx: Sender<AudioCommand>,
    volume: f32,
}

impl AudioPlayer {
    /// 開啟預設輸出裝置並啟動播放執行緒。
    pub fn new(events: UnboundedSender<AudioEvent>) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<AudioCommand>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

        thread::spawn(move || {
            let stream = match rodio::OutputStreamBuilder::open_default_stream() {
                Ok(stream) => stream,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("開啟音訊輸出失敗：{e}")));
                    return;
                }
            };
            let sink = rodio::Sink::connect_new(stream.mixer());
            let _ = ready_tx.send(Ok(()));

            // 載入過曲目之後才需要判斷播放結束，避免一啟動就誤報。
            let mut loaded = false;

            loop {
                match rx.recv_timeout(TICK) {
                    Ok(AudioCommand::Load(path)) => {
                        sink.clear();
                        match std::fs::File::open(&path)
                            .map_err(|e| e.to_string())
                            .and_then(|f| rodio::Decoder::try_from(f).map_err(|e| e.to_string()))
                        {
                            Ok(decoder) => {
                                sink.append(decoder);
                                sink.play();
                                loaded = true;
                            }
                            Err(e) => {
                                loaded = false;
                                let _ = events.send(AudioEvent::Error(format!("解碼失敗：{e}")));
                            }
                        }
                    }
                    Ok(AudioCommand::Play) => sink.play(),
                    Ok(AudioCommand::Pause) => sink.pause(),
                    Ok(AudioCommand::Seek(ms)) => {
                        if sink.try_seek(Duration::from_millis(ms as u64)).is_err() {
                            let _ =
                                events.send(AudioEvent::Error("這個音訊格式不支援跳轉".into()));
                        }
                    }
                    Ok(AudioCommand::Volume(v)) => sink.set_volume(v),
                    Ok(AudioCommand::Stop) => {
                        sink.clear();
                        loaded = false;
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if loaded {
                            if sink.empty() {
                                loaded = false;
                                let _ = events.send(AudioEvent::Ended);
                            } else if !sink.is_paused() {
                                let position = sink.get_pos().as_millis() as u32;
                                let _ = events.send(AudioEvent::Position(position));
                            }
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self { tx, volume: 0.5 }),
            Ok(Err(e)) => Err(anyhow!(e)),
            Err(_) => Err(anyhow!("音訊輸出啟動逾時")),
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
        self.send(AudioCommand::Volume(self.volume));
    }
}
