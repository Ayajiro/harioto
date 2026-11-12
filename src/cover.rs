//! 專輯縮圖：抓 YouTube 的縮圖，用 kitty 圖形協定畫在歌單底下。
//!
//! 終端機不支援圖形協定時整個模組靜默不動作，版面也不會替它留位置。
//! 圖是「畫在文字上面」的一層，所以只要把它放進沒有字的格子裡，
//! ratatui 每一幀重畫文字都不會把它擦掉，換曲或改變位置時才重送。

use anyhow::{Result, anyhow};
use ratatui::layout::Rect;
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
};

/// 畫面上同時只有一張圖，固定用這個編號，換曲時直接覆蓋。
const IMAGE_ID: u32 = 7314;

/// 一次送出的 base64 長度上限，kitty 協定規定不超過 4096。
const CHUNK: usize = 4096;

/// 問不到字格尺寸時的猜測值，比例接近常見的等寬字型。
const FALLBACK_CELL: (u32, u32) = (8, 17);

/// 終端機支不支援 kitty 圖形協定。啟動時問一次。
static SUPPORTED: OnceLock<bool> = OnceLock::new();

/// 在進入畫面之前呼叫一次，把探測結果記下來。
pub fn detect(supported: bool) {
    let _ = SUPPORTED.set(supported);
}

pub fn supported() -> bool {
    SUPPORTED.get().copied().unwrap_or(false)
}

/// 解碼之後的縮圖，一個像素三個位元組。
#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// 縮圖快取，和音訊快取並排。
fn dir() -> PathBuf {
    match crate::cache::dir().parent() {
        Some(parent) => parent.join("thumbs"),
        None => PathBuf::from("thumbs"),
    }
}

/// 確保縮圖在本機，回傳檔案位置。已經有的就直接用。
pub async fn ensure(video_id: &str) -> Result<PathBuf> {
    let dir = dir();
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!("{video_id}.jpg"));
    if path.is_file() {
        return Ok(path);
    }

    // mqdefault 是 320x180，沒有上下黑邊，省得自己裁掉。
    let url = format!("https://i.ytimg.com/vi/{video_id}/mqdefault.jpg");
    let bytes = reqwest::get(&url)
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let part = dir.join(format!("{video_id}.part"));
    tokio::fs::write(&part, &bytes).await?;
    tokio::fs::rename(&part, &path).await?;
    Ok(path)
}

/// 把 JPEG 解成 RGB。kitty 協定只收 PNG 或原始像素，所以得先自己解。
pub fn decode(path: &Path) -> Result<Image> {
    let bytes = std::fs::read(path)?;
    let mut decoder =
        zune_jpeg::JpegDecoder::new(zune_jpeg::zune_core::bytestream::ZCursor::new(&bytes));
    let rgb = decoder
        .decode()
        .map_err(|e| anyhow!("could not decode the thumbnail: {e:?}"))?;
    let info = decoder
        .info()
        .ok_or_else(|| anyhow!("the thumbnail carries no size"))?;
    let (width, height) = (info.width as u32, info.height as u32);
    let expected = width as usize * height as usize * 3;
    if rgb.len() < expected {
        return Err(anyhow!(
            "the thumbnail is short of pixels: {} < {expected}",
            rgb.len()
        ));
    }
    Ok(Image { width, height, rgb })
}

/// 一個字格有多少像素。終端機不回報就用猜的，比例仍然接近。
fn cell_pixels() -> (u32, u32) {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } == 0;
    if ok && size.ws_xpixel > 0 && size.ws_ypixel > 0 && size.ws_col > 0 && size.ws_row > 0 {
        return (
            (size.ws_xpixel / size.ws_col) as u32,
            (size.ws_ypixel / size.ws_row) as u32,
        );
    }
    FALLBACK_CELL
}

/// 在給定的格子範圍裡放一張圖，維持比例，回傳佔幾欄幾列。
fn fit(area: (u16, u16), image: (u32, u32), cell: (u32, u32)) -> (u16, u16) {
    if area.0 == 0 || area.1 == 0 || image.0 == 0 || image.1 == 0 || cell.0 == 0 || cell.1 == 0 {
        return (0, 0);
    }
    let box_w = f64::from(area.0) * f64::from(cell.0);
    let box_h = f64::from(area.1) * f64::from(cell.1);
    let scale = (box_w / f64::from(image.0)).min(box_h / f64::from(image.1));
    let cols = (f64::from(image.0) * scale / f64::from(cell.0))
        .floor()
        .max(1.0) as u16;
    let rows = (f64::from(image.1) * scale / f64::from(cell.1))
        .floor()
        .max(1.0) as u16;
    (cols.min(area.0), rows.min(area.1))
}

/// 把圖畫進這塊範圍，置中。呼叫前要確定那些格子沒有文字。
pub fn place(image: &Image, area: Rect) -> Result<()> {
    let cell = cell_pixels();
    let (cols, rows) = fit((area.width, area.height), (image.width, image.height), cell);
    if cols == 0 || rows == 0 {
        return Ok(());
    }
    let x = area.x + (area.width - cols) / 2;
    let y = area.y + (area.height - rows) / 2;
    let payload = base64(&image.rgb);

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    // 圖從游標所在的格子開始畫，所以先把游標移過去。
    write!(out, "\x1b[{};{}H", y + 1, x + 1)?;

    let mut rest = payload.as_bytes();
    let mut first = true;
    while !rest.is_empty() {
        let (chunk, tail) = rest.split_at(rest.len().min(CHUNK));
        let more = u8::from(!tail.is_empty());
        if first {
            // q=2 要求終端機不要回覆，免得答案混進按鍵輸入。
            write!(
                out,
                "\x1b_Ga=T,f=24,s={},v={},i={},c={},r={},q=2,m={};",
                image.width, image.height, IMAGE_ID, cols, rows, more
            )?;
            first = false;
        } else {
            write!(out, "\x1b_Gm={more};")?;
        }
        out.write_all(chunk)?;
        out.write_all(b"\x1b\\")?;
        rest = tail;
    }
    out.flush()?;
    Ok(())
}

/// 把畫上去的圖拿掉，連同終端機裡的那份資料。
pub fn clear() {
    if !supported() {
        return;
    }
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = write!(out, "\x1b_Ga=d,d=I,i={IMAGE_ID},q=2\x1b\\");
    let _ = out.flush();
}

/// kitty 協定的酬載是 base64。只用這一處，所以自己算，不多一個相依。
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for group in data.chunks(3) {
        let a = u32::from(group[0]);
        let b = u32::from(group.get(1).copied().unwrap_or(0));
        let c = u32::from(group.get(2).copied().unwrap_or(0));
        let packed = (a << 16) | (b << 8) | c;
        out.push(ALPHABET[(packed >> 18) as usize & 63] as char);
        out.push(ALPHABET[(packed >> 12) as usize & 63] as char);
        out.push(if group.len() > 1 {
            ALPHABET[(packed >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if group.len() > 2 {
            ALPHABET[packed as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_known_answers() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0u8, 255, 128]), "AP+A");
    }

    #[test]
    fn a_wide_image_fills_the_width() {
        // 16:9 的圖放進 38 欄的框，字格 8x17：高度由比例決定。
        let (cols, rows) = fit((38, 12), (320, 180), (8, 17));
        assert_eq!(cols, 38);
        assert_eq!(rows, 10);
    }

    #[test]
    fn a_short_box_limits_the_height() {
        let (cols, rows) = fit((38, 4), (320, 180), (8, 17));
        assert!(rows <= 4, "高度 {rows} 超出框");
        assert!(cols < 38, "寬度應該跟著縮：{cols}");
    }

    #[test]
    fn an_empty_box_draws_nothing() {
        assert_eq!(fit((0, 10), (320, 180), (8, 17)), (0, 0));
        assert_eq!(fit((38, 12), (0, 0), (8, 17)), (0, 0));
    }
}
