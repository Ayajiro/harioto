//! 問終端機兩件事：現在用什麼顏色，以及畫不畫得出圖。
//!
//! 送出 OSC 查詢（`OSC 10` 前景、`OSC 11` 背景、`OSC 4;n` 第 n 號色）和一個
//! kitty 圖形協定的查詢，終端機會把答案回在標準輸入。最後補一個 DA1 查詢當
//! 結尾記號：不支援這些查詢的終端機仍然會回 DA1，所以不必等到逾時。
//!
//! 圖形的答案不一定排在 DA1 前面，實測會晚一步到，所以看到 DA1 之後還會
//! 多等一小段時間。

use crate::theme::Palette;
use ratatui::style::Color;
use std::{
    io::{IsTerminal, Read, Write},
    os::fd::AsRawFd,
    time::{Duration, Instant},
};

/// 整個查詢最多花這麼久。終端機通常在幾毫秒內回覆。
const BUDGET: Duration = Duration::from_millis(300);

/// 看到 DA1 之後再多等這麼久，給晚一步到的圖形答案。
const GRACE: Duration = Duration::from_millis(120);

/// 探測結果。兩項各自獨立，問不到就是沒有。
pub struct Probe {
    pub palette: Option<Palette>,
    pub graphics: bool,
}

/// 問終端機配色與圖形支援。不是終端機時兩項都當成沒有。
pub fn query() -> Probe {
    let mut stdout = std::io::stdout();
    if !stdout.is_terminal() || !std::io::stdin().is_terminal() {
        return Probe {
            palette: None,
            graphics: false,
        };
    }

    let raw = ratatui::crossterm::terminal::enable_raw_mode().is_ok();
    // 先問圖形：傳一張 1x1 的圖進去問結果，支援的終端機會回 OK。
    let mut request = String::from("\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\");
    request.push_str("\x1b]10;?\x07\x1b]11;?\x07");
    for index in 0..16 {
        request.push_str(&format!("\x1b]4;{index};?\x07"));
    }
    request.push_str("\x1b[c"); // DA1：結尾記號
    let sent = stdout
        .write_all(request.as_bytes())
        .and_then(|_| stdout.flush())
        .is_ok();

    let answer = if sent { read_answer() } else { Vec::new() };

    if raw {
        let _ = ratatui::crossterm::terminal::disable_raw_mode();
    }

    let answer = String::from_utf8_lossy(&answer).into_owned();
    Probe {
        palette: parse(&answer),
        graphics: has_graphics(&answer),
    }
}

/// kitty 的答覆長成 `ESC _ G i=31 ; OK ESC \`。
fn has_graphics(answer: &str) -> bool {
    let mut rest = answer;
    while let Some(at) = rest.find("\x1b_G") {
        let after = &rest[at + 3..];
        let end = after.find('\x1b').unwrap_or(after.len());
        if after[..end].contains(";OK") {
            return true;
        }
        rest = after;
    }
    false
}

/// 讀到看見 DA1 回覆或時間用完為止；看到 DA1 之後再多讀一小段。
fn read_answer() -> Vec<u8> {
    let stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    let started = Instant::now();
    let mut collected = Vec::new();
    let mut chunk = [0u8; 1024];
    let mut deadline = BUDGET;

    while started.elapsed() < deadline {
        let remaining = deadline.saturating_sub(started.elapsed()).as_millis() as libc::c_int;
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // 只在確定有東西可讀時才去讀，否則沒人回答就會卡住整個啟動。
        let ready = unsafe { libc::poll(&mut poll, 1, remaining.max(1)) };
        if ready <= 0 {
            break;
        }
        let Ok(read) = stdin.lock().read(&mut chunk) else {
            break;
        };
        if read == 0 {
            break;
        }
        collected.extend_from_slice(&chunk[..read]);
        if ends_with_da1(&collected) {
            // 圖形的答案可能還在路上，把期限縮成一小段寬限時間。
            deadline = (started.elapsed() + GRACE).min(BUDGET + GRACE);
        }
    }
    collected
}

/// DA1 的回覆長成 `ESC [ ? ... c`，看到它就知道前面的答案都到齊了。
fn ends_with_da1(bytes: &[u8]) -> bool {
    let Some(start) = bytes.windows(3).position(|w| w == b"\x1b[?") else {
        return false;
    };
    bytes[start..].contains(&b'c')
}

/// 從回覆裡挑出前景、背景與 16 色。
pub fn parse(answer: &str) -> Option<Palette> {
    let foreground = find_color(answer, "]10;")?;
    let background = find_color(answer, "]11;")?;
    let mut ansi = [Color::Reset; 16];
    for (index, slot) in ansi.iter_mut().enumerate() {
        *slot = find_color(answer, &format!("]4;{index};"))?;
    }
    Some(Palette {
        background,
        foreground,
        selection: None,
        ansi,
    })
}

/// 找出某個標記後面的 `rgb:RRRR/GGGG/BBBB`，每個分量可以是 1 到 4 位十六進位。
fn find_color(answer: &str, marker: &str) -> Option<Color> {
    let mut rest = answer;
    while let Some(at) = rest.find(marker) {
        let after = &rest[at + marker.len()..];
        if let Some(body) = after.strip_prefix("rgb:")
            && let Some(color) = parse_rgb(body)
        {
            return Some(color);
        }
        rest = after;
    }
    None
}

fn parse_rgb(body: &str) -> Option<Color> {
    let end = body.find(['\x07', '\x1b']).unwrap_or(body.len());
    let mut parts = body[..end].split('/');
    let r = scale(parts.next()?)?;
    let g = scale(parts.next()?)?;
    let b = scale(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some(Color::Rgb(r, g, b))
}

/// `ffff`、`ff`、`f` 都代表滿值，所以依位數換算成 8 位元。
fn scale(part: &str) -> Option<u8> {
    let part = part.trim();
    if part.is_empty() || part.len() > 4 {
        return None;
    }
    let value = u32::from_str_radix(part, 16).ok()?;
    let max = 16u32.pow(part.len() as u32) - 1;
    Some(((value * 255 + max / 2) / max) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply() -> String {
        let mut out = String::from("\x1b]10;rgb:abab/b2b2/bfbf\x07\x1b]11;rgb:2121/2525/2b2b\x07");
        for index in 0..16 {
            out.push_str(&format!("\x1b]4;{index};rgb:4545/4c4c/5959\x07"));
        }
        out.push_str("\x1b[?62;c");
        out
    }

    #[test]
    fn reads_a_full_reply() {
        let palette = parse(&reply()).expect("解析回覆");
        assert_eq!(palette.foreground, Color::Rgb(0xab, 0xb2, 0xbf));
        assert_eq!(palette.background, Color::Rgb(0x21, 0x25, 0x2b));
        assert_eq!(palette.ansi[7], Color::Rgb(0x45, 0x4c, 0x59));
    }

    #[test]
    fn a_short_reply_gives_nothing() {
        assert!(parse("\x1b]10;rgb:abab/b2b2/bfbf\x07\x1b[?62;c").is_none());
    }

    #[test]
    fn accepts_both_two_and_four_digit_components() {
        assert_eq!(parse_rgb("ff/00/80\x07"), Some(Color::Rgb(255, 0, 128)));
        assert_eq!(
            parse_rgb("ffff/0000/8080\x07"),
            Some(Color::Rgb(255, 0, 128))
        );
        assert_eq!(parse_rgb("f/0/8\x07"), Some(Color::Rgb(255, 0, 136)));
    }

    #[test]
    fn spots_a_graphics_answer() {
        assert!(has_graphics("\x1b[?62;22c\x1b_Gi=31;OK\x1b\\"));
        assert!(!has_graphics("\x1b[?62;22c"));
        assert!(
            !has_graphics("\x1b_Gi=31;ENOTSUPPORTED:no\x1b\\"),
            "失敗的答覆不算支援"
        );
    }

    #[test]
    fn stops_at_the_da1_reply() {
        assert!(ends_with_da1(b"\x1b]10;rgb:1/1/1\x07\x1b[?62;c"));
        assert!(!ends_with_da1(b"\x1b]10;rgb:1/1/1\x07"));
    }
}
