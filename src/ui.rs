//! 終端機畫面繪製。

use crate::app::{App, Focus, Mode, format_duration};
use crate::theme::Theme;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Gauge, List, ListItem, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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

/// 面板外框。主題設成無框時只留標題，省下四行高度。
fn panel<'a>(theme: &Theme, title: &'a str, focused: bool) -> Block<'a> {
    let line = if focused { theme.accent } else { theme.dim };
    let title = Span::styled(
        format!(" {title} "),
        if focused {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        },
    );
    match theme.border.kind() {
        Some(kind) => Block::bordered()
            .border_type(kind)
            .border_style(Style::default().fg(line))
            .title(title),
        None => Block::new().title(title),
    }
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
    let theme = &app.theme;
    let style = if editing {
        Style::default().fg(theme.text)
    } else {
        Style::default().fg(theme.dim)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(text, style)).block(panel(theme, title, editing)),
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
                Span::styled(count, Style::default().fg(app.theme.muted)),
            ]))
        })
        .collect();

    let playlists = List::new(playlist_items)
        .block(panel(&app.theme, "歌單", app.focus == Focus::Playlists))
        .highlight_style(
            Style::default()
                .bg(app.theme.selection_bg)
                .fg(app.theme.selection_fg),
        )
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
                Span::styled(duration, Style::default().fg(app.theme.muted)),
            ]))
        })
        .collect();

    let tracks = List::new(track_items)
        .block(panel(
            &app.theme,
            &app.track_title,
            app.focus == Focus::Tracks,
        ))
        .highlight_style(
            Style::default()
                .bg(app.theme.selection_bg)
                .fg(app.theme.selection_fg),
        )
        .highlight_symbol("▌");
    frame.render_stateful_widget(tracks, columns[1], &mut app.track_state);
}

fn draw_now_playing(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let block = panel(theme, "播放中", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(inner);

    let Some(now) = app.now.as_ref() else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "尚未播放任何曲目",
                Style::default().fg(theme.dim),
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
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            now.video.title.clone(),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", now.video.channel),
            Style::default().fg(theme.muted),
        ),
        Span::styled(
            format!("   音量 {}%", app.volume_percent()),
            Style::default().fg(theme.muted),
        ),
        Span::styled(
            if !app.normalize {
                "  平衡關".to_string()
            } else if app.gain_ready() {
                format!("  平衡 {:+.1} dB", app.gain_db())
            } else {
                "  平衡 量測中".to_string()
            },
            Style::default().fg(theme.muted),
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
        .gauge_style(Style::default().fg(theme.accent).bg(theme.gauge_track))
        .ratio(ratio)
        .label(format!(
            "{} / {}",
            format_duration(position),
            format_duration(duration)
        ));
    frame.render_widget(gauge, rows[1]);
}

/// 狀態訊息至少保留的寬度，免得提示列把它整個擠掉。
const STATUS_MIN: usize = 24;

/// 按鍵提示。窄畫面時從列表尾端開始捨棄，排在後面的先丟。
const HINTS: [&str; 13] = [
    "/搜尋",
    "Enter播放",
    "空白鍵暫停",
    "n下一首",
    "q離開",
    "r電台",
    "Tab切換",
    "+-音量",
    "←→快轉",
    "g平衡",
    "a加入",
    "d移除",
    "N新歌單",
];

/// 提示列的顯示順序，和捨棄順序分開，讓畫面上的排列維持穩定。
const HINT_ORDER: [usize; 13] = [0, 5, 12, 10, 11, 6, 1, 2, 3, 8, 7, 9, 4];

/// 在給定寬度下挑出放得進去的提示，回傳排好的字串。
fn hint_line(width: usize) -> String {
    let mut keep = [true; HINTS.len()];
    loop {
        let text = compose_hints(&keep);
        if text.width() <= width {
            return text;
        }
        // 從捨棄順序的尾端往前拿掉一項。
        let Some(drop) = (0..HINTS.len()).rev().find(|&i| keep[i]) else {
            return String::new();
        };
        keep[drop] = false;
    }
}

fn compose_hints(keep: &[bool; HINTS.len()]) -> String {
    let mut out = String::new();
    for &index in HINT_ORDER.iter() {
        if !keep[index] {
            continue;
        }
        out.push_str("  ");
        out.push_str(HINTS[index]);
    }
    out
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let total = area.width as usize;
    // 提示先取得空間，但替狀態訊息留一塊，兩邊都不會被完全擠掉。
    let reserved = if app.context_line().is_empty() {
        0
    } else {
        STATUS_MIN.min(total / 2)
    };
    let hints = hint_line(total.saturating_sub(reserved));
    let left = total.saturating_sub(hints.width());
    let message = app.context_line();
    let status = if message.is_empty() || left < 4 {
        String::new()
    } else {
        format!(" {} ", truncate(&message, left.saturating_sub(2)))
    };
    let gap = total.saturating_sub(status.width() + hints.width());

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                status,
                Style::default()
                    .fg(app.theme.on_accent)
                    .bg(app.theme.accent),
            ),
            Span::raw(" ".repeat(gap)),
            Span::styled(hints, Style::default().fg(app.theme.dim)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_screens_show_every_hint() {
        let line = hint_line(400);
        for hint in HINTS {
            assert!(line.contains(hint), "少了 {hint}");
        }
    }

    #[test]
    fn narrow_screens_drop_the_least_used_first() {
        let line = hint_line(30);
        assert!(line.width() <= 30, "寬度 {} 超過 30", line.width());
        assert!(line.contains("/搜尋"));
        assert!(line.contains("Enter播放"));
        assert!(!line.contains("N新歌單"));
    }

    #[test]
    fn a_hintless_width_gives_an_empty_line() {
        assert_eq!(hint_line(2), "");
    }

    #[test]
    fn hints_keep_their_display_order_whatever_is_dropped() {
        let line = hint_line(60);
        let search = line.find("/搜尋").expect("搜尋在列");
        let quit = line.find("q離開");
        if let Some(quit) = quit {
            assert!(search < quit, "順序不對：{line}");
        }
    }
}
