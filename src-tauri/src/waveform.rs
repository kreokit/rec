//! Sound levels for Finish's timeline (the scale of KreoKit Clips' waveform): one byte per
//! 10 ms over the whole recording, the louder of the microphone and the computer sound.
//! Worked out once and kept in the recording's folder (the tracks never change after the
//! recording is finished).

use std::fs;
use std::path::Path;

use crate::decode::Decoder;

/// Levels per second (one per 10 ms).
pub const PEAKS_PER_SECOND: u32 = 100;
/// Decoding rate: plenty for a picture of the sound.
const RATE: u32 = 16_000;
/// Levels are drawn on a decibel scale from this floor up to 0 dBFS.
const FLOOR_DB: f32 = -48.0;
const FILE: &str = "waveform.bin";
/// The sound tracks it's made of.
pub const TRACKS: [&str; 2] = ["mic.mp4", "system.mp4"];

/// One byte per 10 ms: the loudest sample in that slice on a 0–255 dB scale (FLOOR_DB
/// and quieter is 0, full scale is 255). A dB scale keeps quiet speech visible next to
/// loud parts.
pub fn peaks(samples: &[f32]) -> Vec<u8> {
    let bucket = (RATE / PEAKS_PER_SECOND) as usize;
    samples
        .chunks(bucket)
        .map(|chunk| {
            let peak = chunk.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let db = 20.0 * peak.max(1e-6).log10();
            ((db - FLOOR_DB) / -FLOOR_DB)
                .clamp(0.0, 1.0)
                .mul_add(255.0, 0.5) as u8
        })
        .collect()
}

fn track_peaks(path: &Path) -> Result<Vec<u8>, String> {
    let mut decoder = Decoder::open(path, 1, RATE)?;
    let mut levels = Vec::new();
    let mut samples = Vec::new();
    let block = (RATE / PEAKS_PER_SECOND) as usize * 1000;
    loop {
        samples.clear();
        if decoder.read(&mut samples, block)? == 0 {
            break;
        }
        levels.extend(peaks(&samples));
    }
    Ok(levels)
}

/// The recording's levels (empty without sound), from the kept file when there is one.
pub fn levels(dir: &Path) -> Result<Vec<u8>, String> {
    if let Ok(kept) = fs::read(dir.join(FILE)) {
        return Ok(kept);
    }
    let mut levels: Vec<u8> = Vec::new();
    for track in TRACKS.map(|t| dir.join(t)).iter().filter(|p| p.is_file()) {
        let more = track_peaks(track)?;
        if more.len() > levels.len() {
            levels.resize(more.len(), 0);
        }
        for (l, m) in levels.iter_mut().zip(more) {
            *l = (*l).max(m);
        }
    }
    // Not kept is fine: it's worked out again next time.
    let _ = fs::write(dir.join(FILE), &levels);
    Ok(levels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_level_per_10_ms_on_a_db_scale() {
        let mut samples = vec![0.0f32; 160]; // silence
        samples.extend(vec![1.0f32; 160]); // full scale
        samples.extend(vec![0.0039f32; 160]); // about -48 dB
        samples.extend(vec![0.0631f32; 160]); // about -24 dB: halfway
        samples.extend(vec![-0.5f32; 80]); // a short last bucket, negative
        let levels = peaks(&samples);
        assert_eq!(levels.len(), 5);
        assert_eq!(levels[0], 0);
        assert_eq!(levels[1], 255);
        assert!(levels[2] <= 1, "{}", levels[2]);
        assert!((126..=130).contains(&levels[3]), "{}", levels[3]);
        assert!((205..=225).contains(&levels[4]), "{}", levels[4]);
    }

    #[test]
    fn levels_of_a_recording_are_kept() {
        let dir = crate::sessions::tests::temp_root("waveform");
        crate::sessions::tests::make_tracks(&dir, 3.0, true, true);
        let levels = levels(&dir).unwrap();
        assert!((295..=305).contains(&levels.len()), "{}", levels.len());
        // The synthetic mic is a sine at −18 dBFS (FFmpeg's sine default): well above the
        // floor.
        assert!(levels[150] > 100, "{}", levels[150]);
        assert_eq!(fs::read(dir.join(FILE)).unwrap(), levels);
        // Without sound: no levels.
        let silent = crate::sessions::tests::temp_root("waveform-silent");
        crate::sessions::tests::make_tracks(&silent, 1.0, false, true);
        assert!(super::levels(&silent).unwrap().is_empty());
    }
}
