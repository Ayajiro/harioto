//! harioto：終端機裡的輕量 YouTube 音樂播放器。

mod app;
mod audio;
mod cache;
mod store;
mod ui;
mod youtube;

use anyhow::Result;
use app::{App, Msg};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::time::Duration;
use tokio::sync::mpsc;

/// yt-dlp 太舊時給出的提醒。超過這個天數就提示更新。
const STALE_DAYS: i64 = 60;

fn stale_warning(version: &str) -> Option<String> {
    let days = youtube::version_age_days(version)?;
    if days <= STALE_DAYS {
        return None;
    }
    Some(format!(
        "yt-dlp {version} 已經 {days} 天沒更新，YouTube 改版後可能靜默失敗，建議執行 brew upgrade yt-dlp"
    ))
}

/// 日誌寫到檔案，因為 TUI 佔用了終端機畫面。
fn init_logging() -> Option<std::path::PathBuf> {
    let dir = cache::dir().parent()?.to_path_buf();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("harioto.log");
    let file = std::fs::File::create(&path).ok()?;

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Pipe(Box::new(file)))
        .init();

    Some(path)
}

/// 自我檢測：搜尋、下載、確認解碼器能開啟，全程不進入終端機畫面。
async fn selftest(query: &str) -> Result<()> {
    let version = youtube::version().await?;
    match youtube::version_age_days(&version) {
        Some(days) => println!("yt-dlp {version}，距離發布 {days} 天"),
        None => println!("yt-dlp {version}，無法判斷發布日期"),
    }

    let results = youtube::search(query, 5).await?;
    println!("搜尋「{query}」得到 {} 筆：", results.len());
    for video in &results {
        println!(
            "  {} | {} | {} | {}",
            video.id,
            app::format_duration(video.duration_ms),
            video.channel,
            video.title
        );
    }

    let Some(first) = results.first() else {
        println!("沒有結果，無法繼續測試下載。");
        return Ok(());
    };

    println!("電台（以第一首為種子，取前 8 筆）：");
    match youtube::radio(&first.id, 8).await {
        Ok(tracks) => {
            for video in tracks.iter().take(8) {
                println!(
                    "  {} | {} | {} | {}",
                    video.id,
                    app::format_duration(video.duration_ms),
                    video.channel,
                    video.title
                );
            }
        }
        Err(e) => println!("  取得電台失敗：{e}"),
    }

    println!("正在下載第一首…");
    let started = std::time::Instant::now();
    let path = cache::ensure(&first.id).await?;
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!(
        "下載完成：{} （{:.1} MB，耗時 {:.1} 秒）",
        path.display(),
        size as f64 / 1_048_576.0,
        started.elapsed().as_secs_f64()
    );

    let file = std::fs::File::open(&path)?;
    match rodio::Decoder::try_from(file) {
        Ok(_) => println!("解碼器可以開啟這個檔案。"),
        Err(e) => {
            println!("解碼器無法開啟：{e}");
            return Ok(());
        }
    }

    println!("試播五秒，位置應該隨實際時間前進：");
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let mut player = audio::AudioPlayer::new(event_tx)?;
    player.set_volume(0.5);
    player.load(path);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            event = event_rx.recv() => match event {
                Some(audio::AudioEvent::Position(ms)) => {
                    println!("  位置 {}", app::format_duration(ms));
                }
                Some(audio::AudioEvent::Ended) => {
                    println!("  播放結束");
                    break;
                }
                Some(audio::AudioEvent::Error(e)) => {
                    println!("  播放錯誤：{e}");
                    break;
                }
                None => break,
            }
        }
    }
    player.stop();

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let log_path = init_logging();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(pos) = args.iter().position(|a| a == "--prune") {
        let limit_mb: u64 = args
            .get(pos + 1)
            .and_then(|v| v.parse().ok())
            .unwrap_or(cache::LIMIT_BYTES / 1_048_576);
        let before = cache::size_bytes();
        let freed = cache::prune(limit_mb * 1_048_576);
        println!(
            "快取 {:.1} MB，上限 {} MB，釋出 {:.1} MB，剩下 {:.1} MB",
            before as f64 / 1_048_576.0,
            limit_mb,
            freed as f64 / 1_048_576.0,
            cache::size_bytes() as f64 / 1_048_576.0
        );
        return Ok(());
    }

    if let Some(pos) = args.iter().position(|a| a == "--selftest") {
        let query = args.get(pos + 1).cloned().unwrap_or_else(|| "lofi".into());
        return selftest(&query).await;
    }

    let startup_note = match youtube::version().await {
        Ok(version) => {
            println!("yt-dlp {version} 就緒");
            stale_warning(&version)
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    if let Some(note) = startup_note.as_ref() {
        println!("{note}");
    }

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<Msg>();

    let audio_tx = event_tx.clone();
    let (audio_event_tx, mut audio_event_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(event) = audio_event_rx.recv().await {
            if audio_tx.send(Msg::Audio(event)).is_err() {
                break;
            }
        }
    });

    let audio = audio::AudioPlayer::new(audio_event_tx)?;
    println!("音訊輸出就緒");
    if let Some(path) = log_path.as_ref() {
        println!("日誌寫在 {}", path.display());
    }

    let terminal = ratatui::init();
    let result = run(terminal, audio, event_tx, &mut event_rx, startup_note).await;
    ratatui::restore();
    result
}

async fn run(
    mut terminal: ratatui::DefaultTerminal,
    audio: audio::AudioPlayer,
    tx: mpsc::UnboundedSender<Msg>,
    rx: &mut mpsc::UnboundedReceiver<Msg>,
    startup_note: Option<String>,
) -> Result<()> {
    // 鍵盤輸入在獨立執行緒讀取，避免阻塞 async 執行期。
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            loop {
                match event::poll(Duration::from_millis(250)) {
                    Ok(true) => match event::read() {
                        Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                            if tx.send(Msg::Key(key)).is_err() {
                                break;
                            }
                        }
                        Ok(_) => {
                            if tx.send(Msg::Tick).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    },
                    Ok(false) => {
                        if tx.send(Msg::Tick).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }

    let mut app = App::new(audio, tx, startup_note);

    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;

        let Some(msg) = rx.recv().await else { break };
        app.handle(msg);
        while let Ok(msg) = rx.try_recv() {
            app.handle(msg);
            if app.should_quit {
                break;
            }
        }
    }

    Ok(())
}
