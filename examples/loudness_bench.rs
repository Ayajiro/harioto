//! 量測單一檔案的響度，並印出耗時，給效能評估用。
//! 用法：cargo run --release --example loudness_bench -- <檔案>

use ebur128::{EbuR128, Mode};
use rodio::Source;
use std::{env, fs::File, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args().nth(1).expect("給我一個音訊檔路徑");
    let started = Instant::now();

    let decoder = rodio::Decoder::try_from(File::open(&path)?)?;
    let channels = decoder.channels() as u32;
    let rate = decoder.sample_rate();

    let mut meter = EbuR128::new(channels, rate, Mode::I | Mode::TRUE_PEAK)?;

    // 一次餵一批，批次大小固定，記憶體用量不隨曲長成長。
    let mut buffer: Vec<f32> = Vec::with_capacity(8192);
    let mut frames = 0u64;
    for sample in decoder {
        buffer.push(sample);
        if buffer.len() == 8192 {
            meter.add_frames_f32(&buffer)?;
            frames += (buffer.len() as u64) / channels as u64;
            buffer.clear();
        }
    }
    if !buffer.is_empty() {
        frames += (buffer.len() as u64) / channels as u64;
        meter.add_frames_f32(&buffer)?;
    }

    let loudness = meter.loudness_global()?;
    let peak = (0..channels)
        .map(|c| meter.true_peak(c).unwrap_or(0.0))
        .fold(0.0f64, f64::max);

    println!(
        "{} | {:.1} LUFS | true peak {:.2} dBTP | {} 聲道 {} Hz | {:.1} 秒音訊 | 耗時 {:?}",
        path,
        loudness,
        20.0 * peak.max(1e-9).log10(),
        channels,
        rate,
        frames as f64 / rate as f64,
        started.elapsed()
    );
    Ok(())
}
