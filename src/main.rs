//! harioto：終端機裡的輕量 YouTube 音樂播放器。

mod api;
mod app;
mod audio;
mod cache;
mod cli;
mod control;
mod cover;
mod history;
mod lang;
mod loudness;
mod mcp;
mod probe;
mod store;
mod theme;
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
        "yt-dlp {version} is {days} days old; YouTube changes break stale copies silently, so try brew upgrade yt-dlp"
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
        Some(days) => println!("yt-dlp {version}, released {days} days ago"),
        None => println!("yt-dlp {version}, release date unknown"),
    }

    let results = youtube::search(query, 5).await?;
    println!("searching for \"{query}\" found {}:", results.len());
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
        println!("nothing found, so there is nothing to download.");
        return Ok(());
    };

    println!("radio seeded with the first result, first 8:");
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
        Err(e) => println!("  could not build the radio: {e}"),
    }

    println!("downloading the first one…");
    let started = std::time::Instant::now();
    let path = cache::ensure(&first.id).await?;
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!(
        "downloaded: {} ({:.1} MB in {:.1} s)",
        path.display(),
        size as f64 / 1_048_576.0,
        started.elapsed().as_secs_f64()
    );

    let file = std::fs::File::open(&path)?;
    match rodio::Decoder::try_from(file) {
        Ok(_) => println!("the decoder opens this file."),
        Err(e) => {
            println!("the decoder cannot open it: {e}");
            return Ok(());
        }
    }

    println!("playing five seconds; the position should track real time:");
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
                    println!("  at {}", app::format_duration(ms));
                }
                Some(audio::AudioEvent::Ended) => {
                    println!("  finished");
                    break;
                }
                Some(audio::AudioEvent::Error(e)) => {
                    println!("  playback error: {e}");
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

    // 子指令（search、radio、playlist、history、seeds、mcp）走命令列路徑，不進入畫面。
    if let Some(result) = cli::dispatch(&args).await {
        return result;
    }

    if let Some(pos) = args.iter().position(|a| a == "--prune") {
        let limit_mb: u64 = args
            .get(pos + 1)
            .and_then(|v| v.parse().ok())
            .unwrap_or(cache::LIMIT_BYTES / 1_048_576);
        let before = cache::size_bytes();
        let freed = cache::prune(limit_mb * 1_048_576);
        println!(
            "cache {:.1} MB, limit {} MB, freed {:.1} MB, now {:.1} MB",
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

    // `--theme <名稱>` 覆寫設定檔裡的 preset。
    let theme_preset = args
        .iter()
        .position(|a| a == "--theme")
        .and_then(|pos| args.get(pos + 1))
        .cloned();
    if let Some(name) = theme_preset.as_ref()
        && !theme::PRESETS.contains(&name.as_str())
    {
        eprintln!(
            "No built-in theme called \"{name}\". Available: {}",
            theme::PRESETS.join("、")
        );
        std::process::exit(1);
    }

    // `--lang <名稱>` 覆寫設定檔裡的語言。
    let lang_flag = args
        .iter()
        .position(|a| a == "--lang")
        .and_then(|pos| args.get(pos + 1))
        .cloned();
    if let Some(name) = lang_flag.as_ref()
        && lang::Lang::parse(name).is_none()
    {
        eprintln!(
            "No interface language called \"{name}\". Available: {}",
            lang::NAMES.join(", ")
        );
        std::process::exit(1);
    }

    let startup_note = match youtube::version().await {
        Ok(version) => {
            println!("yt-dlp {version} ready");
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

    // 控制通道：讓 harioto ctl 與其他程式操作這個播放器。
    {
        let control_tx = event_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = control::serve(control_tx).await {
                log::warn!("the control socket did not open: {e}");
            }
        });
        println!("control socket: {}", control::socket_path().display());
    }

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
    println!("audio output ready");
    if let Some(path) = log_path.as_ref() {
        println!("log at {}", path.display());
    }

    // 問終端機現在的配色和畫不畫得出圖。要在進入畫面之前做，否則回覆會和
    // 按鍵事件混在一起。
    let probed = probe::query();
    match probed.palette.as_ref() {
        Some(palette) => log::info!(
            "the terminal reports foreground {:?} and background {:?}",
            palette.foreground,
            palette.background
        ),
        None => log::info!(
            "the terminal reported no colours, so palette: \"auto\" falls back to the preset"
        ),
    }
    log::info!(
        "terminal graphics: {}",
        if probed.graphics {
            "supported, the cover will show under the playlists"
        } else {
            "no answer, so no cover"
        }
    );
    theme::detect(probed.palette);
    cover::detect(probed.graphics);

    let terminal = ratatui::init();
    let result = run(
        terminal,
        audio,
        event_tx,
        &mut event_rx,
        startup_note,
        theme_preset,
        lang_flag,
    )
    .await;
    cover::clear();
    ratatui::restore();
    control::cleanup();
    result
}

async fn run(
    mut terminal: ratatui::DefaultTerminal,
    audio: audio::AudioPlayer,
    tx: mpsc::UnboundedSender<Msg>,
    rx: &mut mpsc::UnboundedReceiver<Msg>,
    startup_note: Option<String>,
    theme_preset: Option<String>,
    lang_flag: Option<String>,
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

    let mut app = App::new(audio, tx, startup_note, theme_preset, lang_flag);

    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        // 圖要畫在文字之上，所以等這一幀的文字都寫完再送。
        app.sync_cover();

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
