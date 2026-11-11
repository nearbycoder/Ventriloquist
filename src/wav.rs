//! Reads just enough of a WAV file to draw it: its length and a row of peaks.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub struct Shape {
    pub seconds: f64,
    /// Peak level per bucket, 0..1.
    pub peaks: Vec<f32>,
}

struct Format {
    tag: u16,
    channels: u16,
    rate: u32,
    bits: u16,
}

/// The length and `buckets` peaks of a PCM or float WAV. `None` for anything else (an mp3, a
/// compressed WAV, a file that isn't there yet), which draws as a plain bar.
pub fn shape(path: &Path, buckets: usize) -> Option<Shape> {
    let mut f = BufReader::new(File::open(path).ok()?);
    let mut head = [0u8; 12];
    f.read_exact(&mut head).ok()?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return None;
    }
    let mut format: Option<Format> = None;
    loop {
        let mut chunk = [0u8; 8];
        f.read_exact(&mut chunk).ok()?;
        let size = u32::from_le_bytes(chunk[4..8].try_into().ok()?) as u64;
        match &chunk[0..4] {
            b"fmt " => {
                let mut b = vec![0u8; size as usize];
                f.read_exact(&mut b).ok()?;
                if b.len() < 16 {
                    return None;
                }
                let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
                let mut tag = u16_at(0);
                // WAVE_FORMAT_EXTENSIBLE keeps the real format in the sub-format GUID.
                if tag == 0xFFFE && b.len() >= 26 {
                    tag = u16_at(24);
                }
                format = Some(Format { tag, channels: u16_at(2), rate: u32::from_le_bytes([b[4], b[5], b[6], b[7]]), bits: u16_at(14) });
                if size % 2 == 1 {
                    f.seek(SeekFrom::Current(1)).ok()?;
                }
            }
            b"data" => return read_peaks(&mut f, format?, size, buckets),
            _ => {
                f.seek(SeekFrom::Current((size + size % 2) as i64)).ok()?;
            }
        }
    }
}

fn read_peaks(f: &mut impl Read, fmt: Format, size: u64, buckets: usize) -> Option<Shape> {
    let bytes = (fmt.bits / 8) as usize;
    let frame = bytes * fmt.channels as usize;
    if frame == 0 || fmt.rate == 0 || buckets == 0 || !matches!((fmt.tag, fmt.bits), (1, 8 | 16 | 24 | 32) | (3, 32 | 64)) {
        return None;
    }
    let mut data = Vec::new();
    // A streamed WAV can carry a placeholder size; read what is really there, up to it.
    f.take(size.min(1 << 31)).read_to_end(&mut data).ok()?;
    let frames = data.len() / frame;
    if frames == 0 {
        return Some(Shape { seconds: 0.0, peaks: vec![0.0; buckets] });
    }
    let sample = |b: &[u8]| -> f32 {
        match (fmt.tag, fmt.bits) {
            (1, 8) => (b[0] as f32 - 128.0) / 128.0,
            (1, 16) => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
            (1, 24) => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
            (1, 32) => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
            (3, 32) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            _ => f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32,
        }
    };
    let mut peaks = vec![0f32; buckets];
    // Looking at every sample of a long file is wasted work for a few dozen bars.
    let step = (frames / (buckets * 2000)).max(1);
    for i in (0..frames).step_by(step) {
        let v = sample(&data[i * frame..i * frame + bytes]).abs();
        let slot = (i as u64 * buckets as u64 / frames as u64) as usize;
        if v > peaks[slot] {
            peaks[slot] = v;
        }
    }
    let top = peaks.iter().cloned().fold(0f32, f32::max);
    if top > 0.0 {
        // Quiet takes still draw a readable shape; the square root lifts the low end.
        peaks.iter_mut().for_each(|p| *p = (*p / top).sqrt().clamp(0.0, 1.0));
    }
    Some(Shape { seconds: frames as f64 / fmt.rate as f64, peaks })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(samples: &[i16], rate: u32) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut out = Vec::new();
        out.extend(b"RIFF");
        out.extend((36 + data.len() as u32).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(rate.to_le_bytes());
        out.extend((rate * 2).to_le_bytes());
        out.extend(2u16.to_le_bytes());
        out.extend(16u16.to_le_bytes());
        out.extend(b"data");
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        out
    }

    #[test]
    fn a_wav_gives_its_length_and_where_it_is_loud() {
        let mut samples = vec![0i16; 8000];
        samples[6000..].iter_mut().for_each(|s| *s = 16000);
        let path = std::env::temp_dir().join(format!("ventriloquist-wav-test-{}.wav", std::process::id()));
        std::fs::write(&path, wav(&samples, 8000)).unwrap();
        let shape = shape(&path, 4).unwrap();
        std::fs::remove_file(&path).ok();
        assert!((shape.seconds - 1.0).abs() < 1e-9);
        assert_eq!(shape.peaks, vec![0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn a_file_that_is_not_a_wav_has_no_shape() {
        let path = std::env::temp_dir().join(format!("ventriloquist-not-wav-{}.bin", std::process::id()));
        std::fs::write(&path, b"ID3 this is not audio we can read").unwrap();
        assert!(shape(&path, 8).is_none());
        std::fs::remove_file(&path).ok();
    }
}
