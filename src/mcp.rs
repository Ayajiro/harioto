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
            _ => id.map(|id| error(id, -32601, format!("unsupported method: {method}"))),
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
            "description": "Search YouTube for music and return a list of tracks with video id, title, channel and length.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What to search for" },
                    "limit": { "type": "integer", "description": "How many at most; 20 by default, 100 at the top" },
                },
                "required": ["query"],
            },
        },
        {
            "name": "harioto_radio",
            "description": "Build a radio from one track and return the mix YouTube generates for it, seed first. The call is written to the seed log.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video": { "type": "string", "description": "A video id or a YouTube url" },
                    "limit": { "type": "integer", "description": "How many at most; 50 by default, 100 at the top" },
                },
                "required": ["video"],
            },
        },
        {
            "name": "harioto_playlist_list",
            "description": "List the local playlists and how many tracks each holds.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "harioto_playlist_show",
            "description": "List every track in one playlist.",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string", "description": "The playlist name" } },
                "required": ["name"],
            },
        },
        {
            "name": "harioto_playlist_create",
            "description": "Create an empty local playlist. A name already in use is an error; nothing is overwritten.",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string", "description": "The playlist name" } },
                "required": ["name"],
            },
        },
        {
            "name": "harioto_playlist_add",
            "description": "Add videos to a playlist, creating it if it is new. Tracks already in it are skipped.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "The playlist name" },
                    "videos": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "A video id or a YouTube url",
                    },
                },
                "required": ["name", "videos"],
            },
        },
        {
            "name": "harioto_history",
            "description": "Read the play history, newest first. Links and titles only, no audio.",
            "inputSchema": {
                "type": "object",
                "properties": { "limit": { "type": "integer", "description": "How many at most; 50 by default" } },
            },
        },
        {
            "name": "harioto_seeds",
            "description": "Read the radio seed log: which tracks have been used as seeds, newest first.",
            "inputSchema": {
                "type": "object",
                "properties": { "limit": { "type": "integer", "description": "How many at most; 50 by default" } },
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
            return error(id, -32602, format!("unknown tool: {other}"));
        }
    };

    match outcome {
        Ok(payload) => success(id, content(payload, false)),
        Err(e) => success(id, content(json!({ "error": e.to_string() }), true)),
    }
}
