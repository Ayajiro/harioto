//! 命令列子指令。給人看的輸出是表格式的文字，給程式看的是 `--json`。

use crate::api;
use crate::history::Entry;
use crate::youtube::Video;
use anyhow::Result;
use serde_json::json;

pub const HELP: &str = "\
harioto - a radio in your terminal

Usage:
  harioto [--theme <name>] [--lang <name>]   Start the player
  harioto search <words> [--limit N]         Search YouTube
  harioto radio <video id or url> [--limit N]  Build a radio, seed first
  harioto playlist list                      List playlists
  harioto playlist show <name>               List what is in one
  harioto playlist create <name>             Create one
  harioto playlist add <name> <video...>     Add to one, creating it if new
  harioto history [--limit N]                Play history, newest first
  harioto seeds [--limit N]                  Radio seeds, newest first
  harioto ctl <command> [target]             Drive a running player
  harioto mcp                                Run as an MCP server for AI clients
  harioto --selftest <words>                 Search, download, decode, play 5 s
  harioto --prune [MB]                       Trim the audio cache to a limit

Control commands:
  harioto ctl status                         What is playing now
  harioto ctl play <id or words>             Play this one now
  harioto ctl radio <id or words>            Open a radio from it and play
  harioto ctl queue <id or words>            Append to the current list
  harioto ctl pause | resume | toggle        Pause, resume, switch
  harioto ctl next | prev | stop             Next, previous, stop
  harioto ctl volume <0-100>                 Volume
  harioto ctl normalize on|off|toggle        Loudness balance
  harioto ctl remove [video id]              Drop one from the list, default the playing one

Options:
  --json     Print JSON, for programs
  --limit N  How many at most
  --theme    Built-in themes: ember, mono, light. Overrides ~/.config/harioto/theme.json
             That file's palette can point at a terminal theme (Ghostty or JSON) to borrow its colours
             palette set to auto asks the terminal itself at startup
  --lang     Interface language: zh, en, ja. Overrides the theme file's lang
             With neither, LC_ALL, LC_MESSAGES and LANG decide; English if they say nothing
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
    println!("{} in all", videos.len());
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
    println!("{} in all", entries.len());
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
        "ctl" => ctl(&ops, &rest, json).await,
        "mcp" => crate::mcp::serve().await,
        _ => return None,
    };

    Some(result)
}

async fn ctl(ops: &[String], rest: &[String], json: bool) -> Result<()> {
    let Some(command) = ops.first().cloned() else {
        return fail(
            "usage: harioto ctl <status|play|radio|queue|pause|resume|toggle|next|prev|stop|volume|normalize|remove>".into(),
            json,
        );
    };
    let target = ops[1..].join(" ");

    let mut payload = serde_json::json!({ "command": command });
    if command == "volume" {
        let level: u64 = target.trim().parse().unwrap_or(50);
        payload["level"] = serde_json::Value::from(level);
    } else if !target.is_empty() {
        payload["target"] = serde_json::Value::from(target);
    }
    let _ = rest;

    let answer = match crate::control::request(payload).await {
        Ok(answer) => answer,
        Err(e) => return fail(e.to_string(), json),
    };

    if let Some(message) = answer.get("error").and_then(|v| v.as_str()) {
        return fail(message.to_string(), json);
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&answer).unwrap_or_default()
        );
        return Ok(());
    }

    match answer.get("playing") {
        Some(playing) if !playing.is_null() => {
            let field = |key: &str| playing.get(key).and_then(|v| v.as_str()).unwrap_or("");
            let position = playing
                .get("position_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let duration = playing
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let state = if playing.get("paused").and_then(|v| v.as_bool()) == Some(true) {
                "paused"
            } else {
                "playing"
            };
            println!(
                "{state} {} / {} | {} | {}",
                crate::app::format_duration(position),
                crate::app::format_duration(duration),
                field("channel"),
                field("title")
            );
        }
        _ => println!("nothing playing"),
    }
    if let Some(note) = answer.get("note").and_then(|v| v.as_str()) {
        println!("{note}");
    }
    println!(
        "list: {} ({} tracks) | volume {}%",
        answer.get("list").and_then(|v| v.as_str()).unwrap_or(""),
        answer
            .get("queue_len")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        answer.get("volume").and_then(|v| v.as_u64()).unwrap_or(0)
    );
    Ok(())
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
                    println!("{:>4} tracks | {}", p.count, p.name);
                }
                println!("{} playlists in all", items.len());
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
                        println!("created the playlist {name}");
                    }
                    Ok(())
                }
                Err(e) => fail(e.to_string(), json),
            }
        }
        "add" => {
            let Some(name) = ops.get(1).cloned() else {
                return fail("usage: harioto playlist add <name> <video...>".into(), json);
            };
            let videos: Vec<String> = ops[2..].to_vec();
            match api::playlist_add(&name, &videos).await {
                Ok(count) => {
                    if json {
                        println!("{}", json!({ "playlist": name, "count": count }));
                    } else {
                        println!("{name} now holds {count} tracks");
                    }
                    Ok(())
                }
                Err(e) => fail(e.to_string(), json),
            }
        }
        other => fail(format!("unknown subcommand: playlist {other}"), json),
    }
}
