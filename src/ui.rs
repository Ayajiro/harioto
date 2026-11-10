//! 終端機畫面繪製。

use crate::app::{App, Focus, Mode, format_duration};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Gauge, List, ListItem, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const ACCENT: Color = Color::Rgb(255, 72, 72);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(4),
        Constraint::Length(1),
    ])
    .split(frame.area());

    draw_input(frame, app, rows[0]);
    draw_lists(frame, app, rows[1]);
    draw_now_playing(frame, app, rows[2]);
    draw_status(frame, app, rows[3]);
}

fn panel(title: &str, focused: bool) -> Block<'_> {
    let style = if focused {
        Style::default().fg(ACCENT)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(style)
        .title(Span::styled(
            format!(" {title} "),
            if focused {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        ))
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let editing = app.mode != Mode::Browse;
    let title = match app.mode {
        Mode::NewPlaylist => "新增歌單",
        _ => "搜尋",
    };
    let text = if editing {
        format!("{}▏", app.input)
    } else {
        "按 / 搜尋 YouTube，按 N 新增歌單".to_string()
    };
    let style = if editing {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(text, style)).block(panel(title, editing)),
        area,
    );
}

fn draw_lists(frame: &mut Frame, app: &mut App, area: Rect) {
    let columns =
        Layout::horizontal([Constraint::Percentage(28), Constraint::Percentage(72)]).split(area);

    let playlist_width = columns[0].width.saturating_sub(3) as usize;
    let playlist_items: Vec<ListItem> = app
        .library
        .playlists
        .iter()
        .map(|p| {
            let count = format!("{}", p.tracks.len());
            let name = truncate(&p.name, playlist_width.saturating_sub(count.len() + 2));
            let pad = playlist_width.saturating_sub(name.width() + count.len());
            ListItem::new(Line::from(vec![
                Span::raw(name),
                Span::raw(" ".repeat(pad)),
                Span::styled(count, Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    let playlists = List::new(playlist_items)
        .block(panel("歌單", app.focus == Focus::Playlists))
        .highlight_style(Style::default().bg(ACCENT).fg(Color::Black))
        .highlight_symbol("▌");
    frame.render_stateful_widget(playlists, columns[0], &mut app.playlist_state);

    let width = columns[1].width.saturating_sub(3) as usize;
    let track_items: Vec<ListItem> = app
        .tracks
        .iter()
        .map(|t| {
            let duration = format_duration(t.duration_ms);
            let left = format!("{} — {}", t.channel, t.title);
            let left = truncate(&left, width.saturating_sub(duration.len() + 2));
            let pad = width.saturating_sub(left.width() + duration.len());
            ListItem::new(Line::from(vec![
                Span::raw(left),
                Span::raw(" ".repeat(pad)),
                Span::styled(duration, Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    let tracks = List::new(track_items)
        .block(panel(&app.track_title, app.focus == Focus::Tracks))
        .highlight_style(Style::default().bg(ACCENT).fg(Color::Black))
        .highlight_symbol("▌");
    frame.render_stateful_widget(tracks, columns[1], &mut app.track_state);
}

fn draw_now_playing(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel("播放中", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(inner);

    let Some(now) = app.now.as_ref() else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "尚未播放任何曲目",
                Style::default().fg(Color::DarkGray),
            )),
            rows[0],
        );
        return;
    };

    let state = if now.buffering {
        "緩衝"
    } else if now.paused {
        "暫停"
    } else {
        "播放"
    };

    let header = Line::from(vec![
        Span::styled(
            format!("{state} "),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            now.video.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", now.video.channel),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(
            format!("   音量 {}%", app.volume_percent()),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(header), rows[0]);

    let position = now.position_ms();
    let duration = now.video.duration_ms;
    let ratio = if duration == 0 {
        0.0
    } else {
        (position as f64 / duration as f64).clamp(0.0, 1.0)
    };
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(ACCENT).bg(Color::Rgb(40, 40, 40)))
        .ratio(ratio)
        .label(format!(
            "{} / {}",
            format_duration(position),
            format_duration(duration)
        ));
    frame.render_widget(gauge, rows[1]);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let hints = "  /搜尋  r電台  N新歌單  a加入  d移除  Tab切換  Enter播放  空白鍵暫停  n下一首  ←→快轉  +-音量  q離開";
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {} ", app.status),
                Style::default().fg(Color::Black).bg(ACCENT),
            ),
            Span::styled(hints, Style::default().fg(Color::DarkGray)),
        ])),
        area,
    );
}

/// 依終端機顯示寬度截斷，中日韓文字算兩格。
fn truncate(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text.width() <= max {
        return text.to_string();
    }

    let budget = max.saturating_sub(1);
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}
