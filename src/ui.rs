//! 終端機畫面繪製。

use crate::app::{App, Focus, Mode, format_duration};
use crate::lang::{t, tf};
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
    let area = frame.area();
    let compact = area.height < COMPACT_BELOW;
    let narrow = area.width < NARROW_BELOW;
    let editing = app.mode != Mode::Browse;
    let rows = split_rows(area, compact, narrow, editing);

    if rows[0].height > 0 {
        draw_input(frame, app, rows[0], compact);
    }
    draw_lists(frame, app, rows[1], narrow);
    draw_now_playing(frame, app, rows[2], compact, narrow);
    draw_status(frame, app, rows[3]);
}

/// 低於這個高度就改用精簡版面：搜尋框只在輸入時出現，播放中不畫外框。
const COMPACT_BELOW: u16 = 18;

/// 低於這個寬度就改成單欄：只畫有焦點的那一份清單，另一份用 Tab 換過來。
const NARROW_BELOW: u16 = 70;

/// 四個區塊的位置。抽出來是為了能單獨驗算高度分配。
fn split_rows(area: Rect, compact: bool, narrow: bool, editing: bool) -> [Rect; 4] {
    let input = match (compact, editing) {
        (false, _) => 3,
        (true, true) => 1,
        (true, false) => 0,
    };
    // 播放中帶著進度條：單欄時曲名、頻道、進度各一行，其餘兩行。
    let now = match (compact, narrow) {
        (true, _) => 2,
        (false, true) => 5,
        (false, false) => 4,
    };
    let rows = Layout::vertical([
        Constraint::Length(input),
        Constraint::Min(2),
        Constraint::Length(now),
        Constraint::Length(1),
    ])
    .split(area);
    [rows[0], rows[1], rows[2], rows[3]]
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

fn draw_input(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let editing = app.mode != Mode::Browse;
    let title = match app.mode {
        Mode::NewPlaylist => t!("新增歌單", "New playlist", "新規プレイリスト"),
        _ => t!("搜尋", "Search", "検索"),
    };
    let text = if editing {
        format!("{}▏", app.input)
    } else {
        t!(
            "按 / 搜尋 YouTube，按 N 新增歌單",
            "Press / to search YouTube, N for a new playlist",
            "/ で YouTube を検索、N で新規プレイリスト"
        )
        .to_string()
    };
    let theme = &app.theme;
    let style = if editing {
        Style::default().fg(theme.text)
    } else {
        Style::default().fg(theme.dim)
    };
    if compact {
        // 精簡版面只有一行，用標題當前綴，省下外框的兩行。
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!(" {title} "),
                    Style::default().fg(theme.on_accent).bg(theme.accent),
                ),
                Span::styled(format!(" {text}"), style),
            ])),
            area,
        );
        return;
    }
    frame.render_widget(
        Paragraph::new(Span::styled(text, style)).block(panel(theme, title, editing)),
        area,
    );
}

/// 兩份清單的位置。單欄時只有拿到焦點的那一份有空間，另一份回傳 None。
fn list_columns(area: Rect, narrow: bool, focus: Focus) -> [Option<Rect>; 2] {
    if narrow {
        return match focus {
            Focus::Playlists => [Some(area), None],
            Focus::Tracks => [None, Some(area)],
        };
    }
    let columns =
        Layout::horizontal([Constraint::Percentage(28), Constraint::Percentage(72)]).split(area);
    [Some(columns[0]), Some(columns[1])]
}

/// 縮圖面板最多這麼高，含上下框線。再高就只是放大同一張圖。
const COVER_MAX: u16 = 12;

/// 低於這個高度圖會小到看不出是什麼，不如把列數還給歌單。
const COVER_MIN: u16 = 6;

/// 歌單至少要留這麼多列。
const PLAYLIST_MIN_ROWS: u16 = 4;

/// 左欄的分配。歌單先拿它的底線，剩下的給縮圖，不夠就整欄給歌單。
fn split_left(area: Rect, wanted: bool) -> (Rect, Option<Rect>) {
    if !wanted {
        return (area, None);
    }
    let spare = area.height.saturating_sub(PLAYLIST_MIN_ROWS);
    if spare < COVER_MIN {
        return (area, None);
    }
    let rows = Layout::vertical([
        Constraint::Min(PLAYLIST_MIN_ROWS),
        Constraint::Length(spare.min(COVER_MAX)),
    ])
    .split(area);
    (rows[0], Some(rows[1]))
}

/// 單欄時在標題前面標出 Tab 可以換到另一份清單。
fn list_title(title: &str, narrow: bool) -> String {
    if narrow {
        tf!(
            "Tab 切換 — {title}",
            "Tab switches — {title}",
            "Tab で切替 — {title}"
        )
    } else {
        title.to_string()
    }
}

fn draw_lists(frame: &mut Frame, app: &mut App, area: Rect, narrow: bool) {
    let columns = list_columns(area, narrow, app.focus);
    app.cover_rect = None;

    if let Some(column) = columns[0] {
        // 正在播、而且終端機畫得出圖，才替縮圖留位置。
        let wants_cover = !narrow && app.cover_on && app.now.is_some() && crate::cover::supported();
        let (column, cover) = split_left(column, wants_cover);
        if let Some(cover) = cover {
            let block = panel(&app.theme, t!("縮圖", "Cover", "サムネイル"), false);
            app.cover_rect = Some(block.inner(cover));
            frame.render_widget(block, cover);
        }

        let playlist_width = column.width.saturating_sub(3) as usize;
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

        let title = list_title(t!("歌單", "Playlists", "プレイリスト"), narrow);
        let playlists = List::new(playlist_items)
            .block(panel(&app.theme, &title, app.focus == Focus::Playlists))
            .highlight_style(
                Style::default()
                    .bg(app.theme.selection_bg)
                    .fg(app.theme.selection_fg),
            )
            .highlight_symbol("▌");
        frame.render_stateful_widget(playlists, column, &mut app.playlist_state);
    }

    if let Some(column) = columns[1] {
        let width = column.width.saturating_sub(3) as usize;
        let track_items: Vec<ListItem> = app
            .tracks
            .iter()
            .map(|t| {
                let duration = format_duration(t.duration_ms);
                // 單欄時把空間讓給曲名，頻道在播放中那一行仍然看得到。
                let left = if narrow {
                    t.title.clone()
                } else {
                    format!("{} — {}", t.channel, t.title)
                };
                let left = truncate(&left, width.saturating_sub(duration.len() + 2));
                let pad = width.saturating_sub(left.width() + duration.len());
                ListItem::new(Line::from(vec![
                    Span::raw(left),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(duration, Style::default().fg(app.theme.muted)),
                ]))
            })
            .collect();

        let title = list_title(&app.track_title, narrow);
        let tracks = List::new(track_items)
            .block(panel(&app.theme, &title, app.focus == Focus::Tracks))
            .highlight_style(
                Style::default()
                    .bg(app.theme.selection_bg)
                    .fg(app.theme.selection_fg),
            )
            .highlight_symbol("▌");
        frame.render_stateful_widget(tracks, column, &mut app.track_state);
    }
}

fn draw_now_playing(frame: &mut Frame, app: &App, area: Rect, compact: bool, narrow: bool) {
    let theme = &app.theme;
    let inner = if compact {
        area
    } else {
        let block = panel(theme, t!("播放中", "Now playing", "再生中"), false);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        inner
    };

    // 單欄時頻道另起一行，其餘版面把頻道接在曲名後面。
    let stacked = narrow && !compact;
    let heights = if stacked {
        vec![
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ]
    } else {
        vec![Constraint::Length(1), Constraint::Length(1)]
    };
    let rows = Layout::vertical(heights).split(inner);

    let Some(now) = app.now.as_ref() else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                t!(
                    "尚未播放任何曲目",
                    "Nothing playing yet",
                    "まだ何も再生していません"
                ),
                Style::default().fg(theme.dim),
            )),
            rows[0],
        );
        return;
    };

    let state = if now.buffering {
        t!("緩衝", "Buffering", "バッファ中")
    } else if now.paused {
        t!("暫停", "Paused", "一時停止")
    } else {
        t!("播放", "Playing", "再生")
    };

    let balance = if !app.normalize {
        t!("平衡關", "balance off", "バランス オフ").to_string()
    } else if app.gain_ready() {
        tf!(
            "平衡 {:+.1} dB",
            "balance {:+.1} dB",
            "バランス {:+.1} dB",
            app.gain_db()
        )
    } else {
        t!("平衡 量測中", "balance measuring", "バランス 測定中").to_string()
    };
    let metrics = tf!(
        "音量 {}%  {}",
        "volume {}%  {}",
        "音量 {}%  {}",
        app.volume_percent(),
        balance
    );

    let head = format!("{state} ");
    let total = rows[0].width as usize;
    let accent = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let strong = Style::default().fg(theme.text).add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(theme.muted);

    if stacked {
        let title = truncate(&now.video.title, total.saturating_sub(head.width()));
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(head, accent),
                Span::styled(title, strong),
            ])),
            rows[0],
        );

        let channel = truncate(
            &now.video.channel,
            total.saturating_sub(metrics.width() + 2),
        );
        let gap = total.saturating_sub(channel.width() + metrics.width());
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(channel, muted),
                Span::raw(" ".repeat(gap)),
                Span::styled(metrics, muted),
            ])),
            rows[1],
        );
        draw_progress(frame, app, rows[2]);
        return;
    }

    // 曲名先拿它需要的，剩下的歸頻道，兩個加起來放不下時曲名讓出三分之一。
    let room = total.saturating_sub(head.width());
    let budget = room
        .saturating_sub(now.video.channel.width() + 2)
        .max(room.saturating_sub(room / 3));
    let title = truncate(&now.video.title, budget);
    let channel = truncate(&now.video.channel, room.saturating_sub(title.width() + 2));
    let channel = if channel.is_empty() {
        String::new()
    } else {
        format!("  {channel}")
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(head, accent),
            Span::styled(title, strong),
            Span::styled(channel, muted),
        ])),
        rows[0],
    );

    // 進度條和音量共用一行：進度佔左邊，數值靠右。
    let tail = metrics.width() as u16 + 2;
    let columns =
        Layout::horizontal([Constraint::Min(MIN_GAUGE), Constraint::Length(tail)]).split(rows[1]);
    draw_progress(frame, app, columns[0]);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled(metrics, muted),
        ])),
        columns[1],
    );
}

/// 進度條至少要這麼寬，不然時間標籤擠不下。
const MIN_GAUGE: u16 = 14;

fn draw_progress(frame: &mut Frame, app: &App, area: Rect) {
    let Some(now) = app.now.as_ref() else {
        return;
    };
    let position = now.position_ms();
    let duration = now.video.duration_ms;
    let ratio = if duration == 0 {
        0.0
    } else {
        (position as f64 / duration as f64).clamp(0.0, 1.0)
    };
    let gauge = Gauge::default()
        .gauge_style(
            Style::default()
                .fg(app.theme.accent)
                .bg(app.theme.gauge_track),
        )
        .ratio(ratio)
        .label(format!(
            "{} / {}",
            format_duration(position),
            format_duration(duration)
        ));
    frame.render_widget(gauge, area);
}

/// 狀態訊息至少保留的寬度，免得提示列把它整個擠掉。
const STATUS_MIN: usize = 24;

/// 提示的數量。三種語言共用同一份順序，所以長度固定。
const HINT_COUNT: usize = 14;

/// 按鍵本身不翻譯，而且和動作分開上色，英文介面才看得出哪一段是要按的。
const HINT_KEYS: [&str; HINT_COUNT] = [
    "/", "Enter", "Space", "n", "q", "r", "Tab", "+-", "←→", "g", "a", "d", "N", "i",
];

/// 每個按鍵做的事。順序和 HINT_KEYS 一致。
fn hint_actions() -> [&'static str; HINT_COUNT] {
    match crate::lang::current() {
        crate::lang::Lang::Zh => [
            "搜尋",
            "播放",
            "暫停",
            "下一首",
            "離開",
            "電台",
            "切換",
            "音量",
            "快轉",
            "平衡",
            "加入",
            "移除",
            "新歌單",
            "縮圖",
        ],
        crate::lang::Lang::En => [
            "search", "play", "pause", "next", "quit", "radio", "switch", "volume", "seek",
            "balance", "add", "drop", "playlist", "cover",
        ],
        crate::lang::Lang::Ja => [
            "検索",
            "再生",
            "一時停止",
            "次へ",
            "終了",
            "ラジオ",
            "切替",
            "音量",
            "シーク",
            "バランス",
            "追加",
            "削除",
            "新規",
            "サムネ",
        ],
    }
}

/// 提示列的顯示順序，和捨棄順序分開，讓畫面上的排列維持穩定。
const HINT_ORDER: [usize; HINT_COUNT] = [0, 5, 12, 10, 11, 6, 1, 2, 3, 8, 7, 9, 13, 4];

/// 一項提示佔的寬度：前面兩格間隔、按鍵、一格、動作。
fn hint_width(key: &str, action: &str) -> usize {
    2 + key.width() + 1 + action.width()
}

/// 在給定寬度下挑出放得進去的提示，依顯示順序排好。
fn kept_hints(width: usize) -> Vec<(&'static str, &'static str)> {
    let actions = hint_actions();
    let mut keep = [true; HINT_COUNT];
    loop {
        let used: usize = HINT_ORDER
            .iter()
            .filter(|&&index| keep[index])
            .map(|&index| hint_width(HINT_KEYS[index], actions[index]))
            .sum();
        if used <= width {
            break;
        }
        // 從捨棄順序的尾端往前拿掉一項。
        let Some(drop) = (0..HINT_COUNT).rev().find(|&index| keep[index]) else {
            return Vec::new();
        };
        keep[drop] = false;
    }
    HINT_ORDER
        .iter()
        .filter(|&&index| keep[index])
        .map(|&index| (HINT_KEYS[index], actions[index]))
        .collect()
}

/// 提示列的純文字版本，給測試驗內容用。畫面走 `hint_spans`。
#[cfg(test)]
fn hint_line(width: usize) -> String {
    let mut out = String::new();
    for (key, action) in kept_hints(width) {
        out.push_str("  ");
        out.push_str(key);
        out.push(' ');
        out.push_str(action);
    }
    out
}

/// 畫面上的提示列：按鍵亮、動作暗，兩段分得開。
fn hint_spans(width: usize, theme: &Theme) -> (Vec<Span<'static>>, usize) {
    let mut spans = Vec::new();
    let mut used = 0;
    for (key, action) in kept_hints(width) {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(key, Style::default().fg(theme.text)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(action, Style::default().fg(theme.dim)));
        used += hint_width(key, action);
    }
    (spans, used)
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let total = area.width as usize;
    let message = app.context_line();
    // 提示先取得空間，但替狀態訊息留一塊，兩邊都不會被完全擠掉。
    let reserved = if message.is_empty() {
        0
    } else {
        STATUS_MIN.min(message.width() + 2).min(total / 2)
    };
    let (hints, hints_width) = hint_spans(total.saturating_sub(reserved), &app.theme);
    let left = total.saturating_sub(hints_width);
    let status = if message.is_empty() || left < 4 {
        String::new()
    } else {
        format!(" {} ", truncate(&message, left.saturating_sub(2)))
    };
    let gap = total.saturating_sub(status.width() + hints_width);

    let mut line = vec![
        Span::styled(
            status,
            Style::default()
                .fg(app.theme.on_accent)
                .bg(app.theme.accent),
        ),
        Span::raw(" ".repeat(gap)),
    ];
    line.extend(hints);
    frame.render_widget(Paragraph::new(Line::from(line)), area);
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
    use ratatui::layout::Rect;

    /// 提示的字會跟著語言變，所以要驗內容的測試先把語言固定住。
    fn in_chinese() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::lang::LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::lang::set(crate::lang::Lang::Zh);
        guard
    }

    fn rows_at(height: u16, compact: bool, editing: bool) -> [u16; 4] {
        let area = Rect::new(0, 0, 144, height);
        split_rows(area, compact, false, editing).map(|rect| rect.height)
    }

    #[test]
    fn a_short_pane_gives_the_lists_what_the_frames_took() {
        let roomy = rows_at(14, false, false);
        let compact = rows_at(14, true, false);
        assert_eq!(roomy, [3, 6, 4, 1]);
        assert_eq!(compact, [0, 11, 2, 1]);
        assert!(compact[1] > roomy[1] + 4, "清單沒有拿到足夠的行數");
    }

    #[test]
    fn typing_brings_the_search_line_back() {
        assert_eq!(rows_at(14, true, true), [1, 10, 2, 1]);
    }

    #[test]
    fn a_tall_pane_keeps_the_full_layout() {
        assert_eq!(rows_at(30, false, false), [3, 22, 4, 1]);
    }

    #[test]
    fn a_narrow_pane_shows_one_list_at_a_time() {
        let area = Rect::new(0, 0, 50, 20);

        let both = list_columns(area, false, Focus::Tracks);
        assert!(both[0].is_some() && both[1].is_some(), "寬畫面要有兩欄");

        let on_tracks = list_columns(area, true, Focus::Tracks);
        assert!(on_tracks[0].is_none(), "焦點在清單時歌單讓位");
        assert_eq!(on_tracks[1].map(|r| r.width), Some(50));

        let on_playlists = list_columns(area, true, Focus::Playlists);
        assert_eq!(on_playlists[0].map(|r| r.width), Some(50));
        assert!(on_playlists[1].is_none(), "焦點在歌單時清單讓位");
    }

    #[test]
    fn the_cover_takes_the_bottom_of_the_left_column() {
        let column = Rect::new(0, 0, 40, 22);
        let (playlists, cover) = split_left(column, true);
        assert_eq!(cover.map(|r| r.height), Some(COVER_MAX));
        assert_eq!(playlists.height, 22 - COVER_MAX);
        assert_eq!(cover.map(|r| r.y), Some(22 - COVER_MAX));
    }

    #[test]
    fn a_tight_column_still_gets_a_smaller_cover() {
        // 歌單拿走 4 列之後剩 7 列，縮圖就用這 7 列。
        let (playlists, cover) = split_left(Rect::new(0, 0, 40, 11), true);
        assert_eq!(playlists.height, PLAYLIST_MIN_ROWS);
        assert_eq!(cover.map(|r| r.height), Some(7));
    }

    #[test]
    fn a_short_column_keeps_every_row_for_the_playlists() {
        let column = Rect::new(0, 0, 40, COVER_MIN + PLAYLIST_MIN_ROWS - 1);
        let (playlists, cover) = split_left(column, true);
        assert!(cover.is_none(), "放不下就不切");
        assert_eq!(playlists.height, column.height);

        let (playlists, cover) = split_left(Rect::new(0, 0, 40, 30), false);
        assert!(cover.is_none(), "沒有要畫就不切");
        assert_eq!(playlists.height, 30);
    }

    #[test]
    fn one_column_titles_say_how_to_get_back() {
        assert_eq!(list_title("歌單", false), "歌單");
        assert!(list_title("歌單", true).contains("Tab"));
    }

    #[test]
    fn a_narrow_pane_gives_playing_its_own_rows() {
        let area = Rect::new(0, 0, 50, 30);
        let narrow = split_rows(area, false, true, false).map(|rect| rect.height);
        let wide = split_rows(area, false, false, false).map(|rect| rect.height);
        assert_eq!(narrow, [3, 21, 5, 1]);
        assert_eq!(wide, [3, 22, 4, 1]);
    }

    #[test]
    fn wide_screens_show_every_hint() {
        let _guard = in_chinese();
        let line = hint_line(400);
        for (key, action) in HINT_KEYS.iter().zip(hint_actions()) {
            assert!(
                line.contains(&format!("{key} {action}")),
                "少了 {key} {action}"
            );
        }
    }

    #[test]
    fn narrow_screens_drop_the_least_used_first() {
        let _guard = in_chinese();
        let line = hint_line(30);
        assert!(line.width() <= 30, "寬度 {} 超過 30", line.width());
        assert!(line.contains("/ 搜尋"));
        assert!(line.contains("Enter 播放"));
        assert!(!line.contains("N 新歌單"));
    }

    #[test]
    fn switching_language_switches_the_hints() {
        let _guard = in_chinese();
        let zh = hint_line(400);
        crate::lang::set(crate::lang::Lang::En);
        let en = hint_line(400);
        crate::lang::set(crate::lang::Lang::Ja);
        let ja = hint_line(400);
        crate::lang::set(crate::lang::Lang::Zh);

        assert!(zh.contains("/ 搜尋"));
        assert!(en.contains("/ search"), "英文提示：{en}");
        assert!(ja.contains("/ 検索"), "日文提示：{ja}");
    }

    #[test]
    fn a_hintless_width_gives_an_empty_line() {
        assert_eq!(hint_line(2), "");
    }

    #[test]
    fn hints_keep_their_display_order_whatever_is_dropped() {
        let _guard = in_chinese();
        let line = hint_line(60);
        let search = line.find("/ 搜尋").expect("搜尋在列");
        let quit = line.find("q 離開");
        if let Some(quit) = quit {
            assert!(search < quit, "順序不對：{line}");
        }
    }
}
