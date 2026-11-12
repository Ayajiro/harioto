//! 響度平衡：量測每首曲子的整合響度與真實峰值，換算成播放時要套用的增益。
//!
//! 量測走純 Rust（`ebur128`），解碼沿用播放用的那一套，不需要外部程式。
//! 一首七分鐘的曲子約 0.7 秒單核 CPU，記憶體佔用固定、不隨曲長成長，
//! 所以放在下載完成之後、播放開始之前的背景工作裡做。

use anyhow::{Result, anyhow};
use ebur128::{EbuR128, Mode};
use rodio::Source;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
};

/// 目標響度。串流平台多半落在 -14 LUFS，跟著走換曲時才不會忽大忽小。
pub const TARGET_LUFS: f64 = -14.0;
/// 增益的上下限，避免極端的量測值把音量推到難以收拾。
pub const MAX_GAIN_DB: f64 = 12.0;
/// 真實峰值的上限，留 1 dB 餘裕給解碼與重取樣造成的過衝。
pub const PEAK_CEILING_DBTP: f64 = -1.0;

/// 一首曲子的量測結果。
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Measurement {
    /// 整合響度，單位 LUFS。
    pub lufs: f64,
    /// 所有聲道中最高的真實峰值，單位 dBTP。
    pub peak_dbtp: f64,
}

impl Measurement {
    /// 要套用的增益，單位 dB。先依目標響度算，再依峰值壓回安全範圍。
    pub fn gain_db(&self) -> f64 {
        if !self.lufs.is_finite() {
            return 0.0;
        }
        let wanted = (TARGET_LUFS - self.lufs).clamp(-MAX_GAIN_DB, MAX_GAIN_DB);
        let headroom = PEAK_CEILING_DBTP - self.peak_dbtp;
        // 只在會削到頂時往下壓；峰值已經過高的曲子因此會被調小。
        wanted.min(headroom).clamp(-MAX_GAIN_DB, MAX_GAIN_DB)
    }

    /// 增益換算成線性倍率，直接乘在音量上。
    pub fn gain_factor(&self) -> f32 {
        10f64.powf(self.gain_db() / 20.0) as f32
    }
}

/// 量測一個音訊檔。這是會卡住執行緒的工作，呼叫端請放進 `spawn_blocking`。
pub fn measure(path: &Path) -> Result<Measurement> {
    let decoder =
        rodio::Decoder::try_from(File::open(path)?).map_err(|e| anyhow!("解碼失敗：{e}"))?;
    let channels = decoder.channels() as u32;
    let rate = decoder.sample_rate();
    if channels == 0 || rate == 0 {
        return Err(anyhow!("音訊格式不完整，無法量測"));
    }

    let mut meter = EbuR128::new(channels, rate, Mode::I | Mode::TRUE_PEAK)
        .map_err(|e| anyhow!("建立量測器失敗：{e:?}"))?;

    // 固定大小的批次：記憶體用量跟曲長無關。
    const BATCH: usize = 8192;
    let mut batch: Vec<f32> = Vec::with_capacity(BATCH);
    for sample in decoder {
        batch.push(sample);
        if batch.len() == BATCH {
            meter
                .add_frames_f32(&batch)
                .map_err(|e| anyhow!("量測失敗：{e:?}"))?;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        meter
            .add_frames_f32(&batch)
            .map_err(|e| anyhow!("量測失敗：{e:?}"))?;
    }

    let lufs = meter
        .loudness_global()
        .map_err(|e| anyhow!("讀取響度失敗：{e:?}"))?;
    let peak = (0..channels)
        .map(|c| meter.true_peak(c).unwrap_or(0.0))
        .fold(0.0f64, f64::max);
    let peak_dbtp = 20.0 * peak.max(1e-9).log10();

    Ok(Measurement { lufs, peak_dbtp })
}

// ---------- 量測結果的保存 ----------

fn path() -> PathBuf {
    crate::store::config_dir().join("loudness.json")
}

fn read_file() -> HashMap<String, Measurement> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// 讀回已知的量測結果。快取被清掉也留著，重新下載同一首不必再量一次。
pub fn load() -> HashMap<String, Measurement> {
    read_file()
}

/// 記下一首的量測結果。寫入前先重讀，避免蓋掉別的行程剛寫的內容。
pub fn save(video_id: &str, measurement: Measurement) {
    let mut all = read_file();
    all.insert(video_id.to_string(), measurement);
    let Ok(text) = serde_json::to_string(&all) else {
        return;
    };
    let file = path();
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&file, text) {
        log::warn!("寫入響度資料失敗：{e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(lufs: f64, peak_dbtp: f64) -> Measurement {
        Measurement { lufs, peak_dbtp }
    }

    #[test]
    fn quiet_track_is_raised_to_target() {
        // 峰值離上限還有 11 dB，所以 6 dB 的提升可以照做。
        assert!((m(-20.0, -12.0).gain_db() - 6.0).abs() < 0.01);
    }

    #[test]
    fn loud_track_is_lowered_to_target() {
        assert!((m(-6.4, -8.0).gain_db() + 7.6).abs() < 0.01);
    }

    #[test]
    fn gain_stops_where_the_peak_would_clip() {
        // 已經安靜但峰值貼頂：只能推到離上限還剩的那一點點。
        let gain = m(-20.0, -1.5).gain_db();
        assert!((gain - 0.5).abs() < 0.01, "{gain}");
    }

    #[test]
    fn gain_never_exceeds_the_limit() {
        assert!((m(-40.0, -40.0).gain_db() - MAX_GAIN_DB).abs() < 0.01);
        assert!((m(10.0, -40.0).gain_db() + MAX_GAIN_DB).abs() < 0.01);
    }

    #[test]
    fn factor_matches_decibels() {
        let factor = m(-20.0, -20.0).gain_factor();
        assert!((factor - 1.995).abs() < 0.01, "{factor}");
    }

    #[test]
    fn silence_measures_as_no_change() {
        assert_eq!(m(f64::NEG_INFINITY, -60.0).gain_db(), 0.0);
    }

    /// 寫一段 1 kHz 正弦波的 16 位元 WAV，給量測用。
    fn sine_wav(path: &Path, seconds: u32, amplitude: f32) {
        const RATE: u32 = 44_100;
        let frames = RATE * seconds;
        let data_len = frames * 2 * 2; // 立體聲、每個取樣 2 位元組
        let mut out = Vec::with_capacity(data_len as usize + 44);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&2u16.to_le_bytes()); // 兩聲道
        out.extend_from_slice(&RATE.to_le_bytes());
        out.extend_from_slice(&(RATE * 4).to_le_bytes()); // 每秒位元組
        out.extend_from_slice(&4u16.to_le_bytes()); // 每格位元組
        out.extend_from_slice(&16u16.to_le_bytes()); // 位元深度
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for n in 0..frames {
            let t = n as f32 / RATE as f32;
            let value = (amplitude * (std::f32::consts::TAU * 1000.0 * t).sin() * 32767.0) as i16;
            out.extend_from_slice(&value.to_le_bytes());
            out.extend_from_slice(&value.to_le_bytes());
        }
        std::fs::write(path, out).expect("寫入測試音訊");
    }

    #[test]
    fn measures_a_real_file_and_remembers_it() {
        let _guard = crate::store::CONFIG_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("建立暫存目錄");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };

        // 半幅的正弦波：峰值約 -6 dBFS，量出來約 -6 LUFS，比目標吵，所以增益往下。
        let wav = dir.path().join("tone.wav");
        sine_wav(&wav, 4, 0.5);

        let measured = measure(&wav).expect("量測");
        assert!(
            (measured.lufs - -6.0).abs() < 1.0,
            "響度不對：{}",
            measured.lufs
        );
        assert!(
            (measured.peak_dbtp - -6.0).abs() < 1.0,
            "峰值不對：{}",
            measured.peak_dbtp
        );
        assert!(
            (measured.gain_db() - -8.0).abs() < 1.0,
            "增益不對：{}",
            measured.gain_db()
        );

        save("tone", measured);
        let all = load();
        let back = all.get("tone").expect("讀回量測結果");
        assert!((back.lufs - measured.lufs).abs() < 0.001);
        assert!((back.peak_dbtp - measured.peak_dbtp).abs() < 0.001);
    }
}
