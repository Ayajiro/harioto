//! 命令列子指令。給人看的輸出是表格式的文字，給程式看的是 `--json`。

use crate::api;
use crate::history::Entry;
use crate::youtube::Video;
use anyhow::Result;
use serde_json::json;

pub const HELP: &str = "\
harioto — 終端機裡的電台

用法：
  harioto                                   啟動播放器
  harioto search <關鍵字> [--limit N]        搜尋 YouTube
  harioto radio <影片ID或網址> [--limit N]    建立電台，種子排第一筆
  harioto playlist list                     列出歌單
  harioto playlist show <名稱>               列出歌單內容
  harioto playlist create <名稱>             建立歌單
  harioto playlist add <名稱> <影片...>      加入歌單，歌單不存在就先建立
  harioto history [--limit N]               播放紀錄，最新在前
  harioto seeds [--limit N]                 電台種子紀錄，最新在前
  harioto mcp                               以 MCP 伺服器執行，供 AI 客戶端呼叫
  harioto --selftest <關鍵字>                搜尋、下載、解碼、試播五秒
  harioto --prune [MB]                      把音訊快取修剪到指定上限

選項：
  --json     以 JSON 輸出，給程式讀
  --limit N  筆數上限
";

/// 找出 `--limit N`，沒給就用預設值。
fn limit_of(args: &[String], default: usize) -> usize {
    args.iter()
        .position(|a| a == "--limit")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn wants_json(args: &[String]) -> bool {
    args.iter().any(|a| a == "--json")
}

/// 取出不是選項的參數。
fn operands(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--limit" {
            skip_next = true;
            continue;
        }
        if arg.starts_with("--") {
            continue;
        }
        out.push(arg.clone());
    }
    out
}

fn print_tracks(videos: &[Video], json: bool) {
    if json {
        let out = json!({ "count": videos.len(), "tracks": api::tracks_out(videos) });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return;
    }
    for (i, v) in videos.iter().enumerate() {
        println!(
            "{:>3}. {} | {} | {} | {}",
            i + 1,
            v.id,
            crate::app::format_duration(v.duration_ms),
            v.channel,
            v.title
        );
    }
    println!("共 {} 筆", videos.len());
}

fn print_entries(entries: &[Entry], json: bool) {
    if json {
        let out = json!({ "count": entries.len(), "entries": entries });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return;
    }
    for e in entries {
        println!("{} | {} | {} | {}", e.at, e.id, e.channel, e.title);
    }
    println!("共 {} 筆", entries.len());
}

fn fail(message: String, json: bool) -> Result<()> {
    if json {
        let out = json!({ "error": message });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    } else {
        eprintln!("{message}");
    }
    std::process::exit(1);
}

/// 處理子指令。回傳 None 代表這次呼叫要進入播放器畫面。
pub async fn dispatch(args: &[String]) -> Option<Result<()>> {
    let command = args.first()?.clone();
    let rest: Vec<String> = args[1..].to_vec();
    let json = wants_json(&rest);
    let ops = operands(&rest);

    let result = match command.as_str() {
        "help" | "--help" | "-h" => {
            print!("{HELP}");
            Ok(())
        }
        "search" => {
            let query = ops.join(" ");
            match api::search(&query, limit_of(&rest, 20)).await {
                Ok(videos) => {
                    print_tracks(&videos, json);
                    Ok(())
                }
                Err(e) => fail(e.to_string(), json),
            }
        }
        "radio" => {
            let seed = ops.first().cloned().unwrap_or_default();
            match api::radio(&seed, limit_of(&rest, 50)).await {
                Ok(videos) => {
                    print_tracks(&videos, json);
                    Ok(())
                }
                Err(e) => fail(e.to_string(), json),
            }
        }
        "playlist" => playlist(&ops, json).await,
        "history" => {
            print_entries(&api::history(limit_of(&rest, 50)), json);
            Ok(())
        }
        "seeds" => {
            print_entries(&api::seeds(limit_of(&rest, 50)), json);
            Ok(())
        }
        "mcp" => crate::mcp::serve().await,
        _ => return None,
    };

    Some(result)
}

async fn playlist(ops: &[String], json: bool) -> Result<()> {
    let action = ops.first().map(String::as_str).unwrap_or("list");
    match action {
        "list" => {
            let items = api::playlists();
            if json {
                let out = json!({ "count": items.len(), "playlists": items });
                println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
            } else {
                for p in &items {
                    println!("{:>4} 首 | {}", p.count, p.name);
                }
                println!("共 {} 份歌單", items.len());
            }
            Ok(())
        }
        "show" => {
            let name = ops[1..].join(" ");
            match api::playlist_show(&name) {
                Ok(tracks) => {
                    print_tracks(&tracks, json);
                    Ok(())
                }
                Err(e) => fail(e.to_string(), json),
            }
        }
        "create" => {
            let name = ops[1..].join(" ");
            match api::playlist_create(&name) {
                Ok(()) => {
                    if json {
                        println!("{}", json!({ "created": name }));
                    } else {
                        println!("已建立歌單：{name}");
                    }
                    Ok(())
                }
                Err(e) => fail(e.to_string(), json),
            }
        }
        "add" => {
            let Some(name) = ops.get(1).cloned() else {
                return fail("用法：harioto playlist add <名稱> <影片...>".into(), json);
            };
            let videos: Vec<String> = ops[2..].to_vec();
            match api::playlist_add(&name, &videos).await {
                Ok(count) => {
                    if json {
                        println!("{}", json!({ "playlist": name, "count": count }));
                    } else {
                        println!("{name} 現在有 {count} 首");
                    }
                    Ok(())
                }
                Err(e) => fail(e.to_string(), json),
            }
        }
        other => fail(format!("不認得的子指令：playlist {other}"), json),
    }
}
