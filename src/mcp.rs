//! MCP 伺服器：以 stdio 上的 JSON-RPC 2.0 對 AI 客戶端提供工具。
//!
//! 自己處理協定而不引入 SDK，因為需要的只有 initialize、tools/list、tools/call 三個方法，
//! 這樣執行檔大小與相依套件都不變。

use crate::api;
use anyhow::Result;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const PROTOCOL_VERSION: &str = "2024-11-05";

pub async fn serve() -> Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        let id = request.get("id").cloned();
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

        let response = match method {
            "initialize" => id.map(|id| {
                success(
                    id,
                    json!({
                        "protocolVersion": PROTOCOL_VERSION,
                        "capabilities": { "tools": {} },
                        "serverInfo": {
                            "name": "harioto",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                    }),
                )
            }),
            "tools/list" => id.map(|id| success(id, json!({ "tools": tools() }))),
            "tools/call" => match id {
                Some(id) => Some(call(id, params).await),
                None => None,
            },
            "ping" => id.map(|id| success(id, json!({}))),
            _ => id.map(|id| error(id, -32601, format!("不支援的方法：{method}"))),
        };

        if let Some(response) = response {
            let line = serde_json::to_string(&response)?;
            stdout.write_all(line.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }

    Ok(())
}

fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i32, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// 工具呼叫的結果一律是一段 JSON 文字，失敗時 isError 為 true。
fn content(payload: Value, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": payload.to_string() }],
        "isError": is_error,
    })
}

fn tools() -> Value {
    json!([
        {
            "name": "harioto_search",
            "description": "搜尋 YouTube 上的歌曲，回傳曲目清單，含影片 ID、標題、頻道與長度。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "搜尋關鍵字" },
                    "limit": { "type": "integer", "description": "筆數上限，預設 20，最多 100" },
                },
                "required": ["query"],
            },
        },
        {
            "name": "harioto_radio",
            "description": "以一首歌為種子建立電台，回傳 YouTube 針對它產生的推薦清單，種子排在第一筆。呼叫會寫進種子紀錄。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video": { "type": "string", "description": "影片 ID 或 YouTube 網址" },
                    "limit": { "type": "integer", "description": "筆數上限，預設 50，最多 100" },
                },
                "required": ["video"],
            },
        },
        {
            "name": "harioto_playlist_list",
            "description": "列出本機歌單與各自的曲目數。",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "harioto_playlist_show",
            "description": "列出一份歌單裡的所有曲目。",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string", "description": "歌單名稱" } },
                "required": ["name"],
            },
        },
        {
            "name": "harioto_playlist_create",
            "description": "建立一份空的本機歌單。名稱已存在時回報錯誤，不覆蓋既有內容。",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string", "description": "歌單名稱" } },
                "required": ["name"],
            },
        },
        {
            "name": "harioto_playlist_add",
            "description": "把一批影片加進歌單，歌單不存在就先建立，重複的曲目會略過。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "歌單名稱" },
                    "videos": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "影片 ID 或 YouTube 網址",
                    },
                },
                "required": ["name", "videos"],
            },
        },
        {
            "name": "harioto_history",
            "description": "讀取播放紀錄，最新的排在最前面。只有連結與標題，沒有音訊。",
            "inputSchema": {
                "type": "object",
                "properties": { "limit": { "type": "integer", "description": "筆數上限，預設 50" } },
            },
        },
        {
            "name": "harioto_seeds",
            "description": "讀取電台種子紀錄，也就是過去用哪些歌開過電台，最新的排在最前面。",
            "inputSchema": {
                "type": "object",
                "properties": { "limit": { "type": "integer", "description": "筆數上限，預設 50" } },
            },
        },
    ])
}

async fn call(id: Value, params: Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let text = |key: &str| -> String {
        args.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let number = |key: &str, default: usize| -> usize {
        args.get(key)
            .and_then(Value::as_u64)
            .map(|v| v as usize)
            .unwrap_or(default)
    };

    let outcome: Result<Value> = match name {
        "harioto_search" => api::search(&text("query"), number("limit", 20))
            .await
            .map(|videos| json!({ "count": videos.len(), "tracks": api::tracks_out(&videos) })),
        "harioto_radio" => api::radio(&text("video"), number("limit", 50))
            .await
            .map(|videos| json!({ "count": videos.len(), "tracks": api::tracks_out(&videos) })),
        "harioto_playlist_list" => {
            let items = api::playlists();
            Ok(json!({ "count": items.len(), "playlists": items }))
        }
        "harioto_playlist_show" => api::playlist_show(&text("name"))
            .map(|tracks| json!({ "count": tracks.len(), "tracks": api::tracks_out(&tracks) })),
        "harioto_playlist_create" => {
            let name = text("name");
            api::playlist_create(&name).map(|()| json!({ "created": name }))
        }
        "harioto_playlist_add" => {
            let videos: Vec<String> = args
                .get("videos")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let name = text("name");
            api::playlist_add(&name, &videos)
                .await
                .map(|count| json!({ "playlist": name, "count": count }))
        }
        "harioto_history" => Ok(json!({ "entries": api::history(number("limit", 50)) })),
        "harioto_seeds" => Ok(json!({ "entries": api::seeds(number("limit", 50)) })),
        other => {
            return error(id, -32602, format!("不認得的工具：{other}"));
        }
    };

    match outcome {
        Ok(payload) => success(id, content(payload, false)),
        Err(e) => success(id, content(json!({ "error": e.to_string() }), true)),
    }
}
