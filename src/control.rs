//! 控制通道：讓外部程式操作正在執行中的播放器。
//!
//! 播放器啟動時在設定目錄開一個 Unix socket，每行一個 JSON 請求、回一行 JSON。
//! 需要連網的部分（搜尋、取電台）在這一層先做完，送進畫面的是現成的曲目清單。

use crate::api;
use crate::app::Msg;
use crate::lang::tf;
use crate::youtube::{self, Video};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

const RADIO_LIMIT: usize = 100;

/// 送進畫面的控制指令，資料都已經備好。
pub enum Control {
    Status,
    Toggle,
    Pause,
    Resume,
    Next,
    Prev,
    Stop,
    Volume(u8),
    /// 響度平衡：Some(true) 開、Some(false) 關、None 切換。
    Normalize(Option<bool>),
    /// 從目前清單拿掉一首：給影片 ID，或不給表示正在播的那首。
    Remove(Option<String>),
    /// 換掉右欄的清單並從 start 開始播。
    Replace {
        title: String,
        tracks: Vec<Video>,
        start: usize,
        /// 這份清單的電台種子，沒有就是點播或搜尋結果。
        seed: Option<Video>,
    },
    /// 接在目前清單後面。
    Queue(Vec<Video>),
}

pub fn socket_path() -> PathBuf {
    crate::store::config_dir().join("ctl.sock")
}

/// 開啟控制通道。已經有別的播放器在聽就回報錯誤，不搶走它的 socket。
pub async fn serve(tx: UnboundedSender<Msg>) -> Result<()> {
    let path = socket_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("could not create the config directory: {e}"))?;
    }

    if UnixStream::connect(&path).await.is_ok() {
        return Err(anyhow!("another player already holds the control socket"));
    }
    let _ = std::fs::remove_file(&path);

    let listener =
        UnixListener::bind(&path).map_err(|e| anyhow!("could not open the control socket: {e}"))?;
    log::info!("control socket: {}", path.display());

    loop {
        let (stream, _) = listener.accept().await?;
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, tx).await {
                log::warn!("control connection ended: {e}");
            }
        });
    }
}

/// 播放器結束時把 socket 檔案收掉。
pub fn cleanup() {
    let _ = std::fs::remove_file(socket_path());
}

async fn handle_connection(stream: UnixStream, tx: UnboundedSender<Msg>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(request) => dispatch(request, &tx).await,
            Err(e) => json!({ "error": format!("could not parse the request: {e}") }),
        };
        writer
            .write_all(serde_json::to_string(&reply)?.as_bytes())
            .await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    Ok(())
}

async fn dispatch(request: Value, tx: &UnboundedSender<Msg>) -> Value {
    let command = request
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let target = request
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let control = match command.as_str() {
        "status" => Control::Status,
        "toggle" => Control::Toggle,
        "pause" => Control::Pause,
        "resume" => Control::Resume,
        "next" => Control::Next,
        "prev" => Control::Prev,
        "stop" => Control::Stop,
        "volume" => match request.get("level").and_then(Value::as_u64) {
            Some(level) => Control::Volume(level.min(100) as u8),
            None => return json!({ "error": "volume takes a number from 0 to 100" }),
        },
        "remove" => Control::Remove(Some(target.trim().to_string())),
        "normalize" => match target.trim().to_ascii_lowercase().as_str() {
            "" | "toggle" => Control::Normalize(None),
            "on" | "1" | "true" => Control::Normalize(Some(true)),
            "off" | "0" | "false" => Control::Normalize(Some(false)),
            other => {
                return json!({ "error": format!("normalize takes on, off or toggle, not \"{other}\"") });
            }
        },
        "radio" => match resolve(&target).await {
            Ok(seed) => match api::radio_with_seed(&seed, RADIO_LIMIT).await {
                Ok(tracks) => Control::Replace {
                    title: tf!("電台：{}", "Radio: {}", "ラジオ：{}", seed.title),
                    tracks,
                    start: 0,
                    seed: Some(seed),
                },
                Err(e) => return json!({ "error": e.to_string() }),
            },
            Err(e) => return json!({ "error": e.to_string() }),
        },
        "play" => match resolve(&target).await {
            Ok(video) => Control::Replace {
                title: tf!("點播：{}", "Playing: {}", "再生：{}", video.title),
                tracks: vec![video],
                start: 0,
                seed: None,
            },
            Err(e) => return json!({ "error": e.to_string() }),
        },
        "queue" => match resolve(&target).await {
            Ok(video) => Control::Queue(vec![video]),
            Err(e) => return json!({ "error": e.to_string() }),
        },
        other => return json!({ "error": format!("unknown command: {other}") }),
    };

    let (reply_tx, reply_rx) = oneshot::channel();
    if tx
        .send(Msg::Control {
            control,
            reply: reply_tx,
        })
        .is_err()
    {
        return json!({ "error": "the player has exited" });
    }

    match reply_rx.await {
        Ok(value) => value,
        Err(_) => json!({ "error": "the player did not answer" }),
    }
}

/// 把使用者給的字串變成一首曲目：影片 ID 或網址就直接查，其餘當成搜尋關鍵字。
async fn resolve(target: &str) -> Result<Video> {
    let target = target.trim();
    if target.is_empty() {
        return Err(anyhow!("needs a video id, a url or search words"));
    }

    let looks_like_link = target.contains("youtu");
    let looks_like_id = !target.contains(char::is_whitespace) && target.len() == 11;

    if (looks_like_link || looks_like_id)
        && let Some(id) = youtube::extract_id(target)
    {
        return youtube::lookup(&id).await;
    }

    api::search(target, 5)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("nothing found for: {target}"))
}

/// 客戶端：連上播放器、送一個請求、取回一行回應。
pub async fn request(payload: Value) -> Result<Value> {
    let path = socket_path();
    let stream = UnixStream::connect(&path)
        .await
        .map_err(|_| anyhow!("no running player at {}", path.display()))?;

    let (reader, mut writer) = stream.into_split();
    writer
        .write_all(serde_json::to_string(&payload)?.as_bytes())
        .await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    let mut lines = BufReader::new(reader).lines();
    match lines.next_line().await? {
        Some(line) => Ok(serde_json::from_str(&line)?),
        None => Err(anyhow!("the player did not answer")),
    }
}
