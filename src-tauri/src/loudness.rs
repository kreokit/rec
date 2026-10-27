//! Loudness targets, from KreoKit Voice (its `loudness.rs`, trimmed to what Rec uses): the
//! measurement that turns a sound and a target into the chain's two gains
//! (`polish::Params`).
//!
//! Passes over the whole sound:
//! 1. its loudness → `pre_gain_db`, bringing it to the chain's working level;
//! 2. the chain without the limiter → `gain_db` to the target;
//! 3. the full chain, measured (loudness and true peak); while it misses the target by
//!    more than [`TOLERANCE_LU`] (the limiter takes some loudness off dynamic sound), the
//!    gain is corrected (following the measured slope) and the pass repeated, at most three
//!    times more.

use std::sync::atomic::{AtomicBool, Ordering};

use ebur128::{EbuR128, Mode};

use crate::polish::{Chain, Params, Stages, WORK_LUFS};

/// How close to the target the result must measure (the presets promise ±0.5 LU).
pub const TOLERANCE_LU: f64 = 0.2;
/// Gains are kept within this many dB (a near-silent sound isn't boosted without end).
const MAX_GAIN_DB: f64 = 30.0;
/// The true peak reported for silence.
pub const SILENT_DB: f64 = -200.0;

/// What the measurement found: the chain's parameters and the result's readings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measured {
    pub params: Params,
    pub input_lufs: Option<f64>,
    pub output_lufs: Option<f64>,
    pub output_true_peak_db: f64,
}

/// A source of blocks of interleaved 48 kHz samples, from the start each time it is called.
pub trait Source {
    fn channels(&self) -> usize;
    /// Samples per channel in the whole sound (for progress).
    fn frames(&self) -> u64;
    /// Calls `each` with consecutive blocks of the whole sound.
    fn for_each_block(
        &self,
        each: &mut dyn FnMut(&[f32]) -> Result<(), String>,
    ) -> Result<(), String>;
}

pub struct Meter {
    ebu: EbuR128,
}

impl Meter {
    pub fn new(channels: usize, true_peak: bool) -> Result<Self, String> {
        let mode = if true_peak {
            Mode::I | Mode::TRUE_PEAK
        } else {
            Mode::I
        };
        Ok(Self {
            ebu: EbuR128::new(channels as u32, 48_000, mode).map_err(|e| e.to_string())?,
        })
    }

    pub fn add(&mut self, block: &[f32]) -> Result<(), String> {
        self.ebu.add_frames_f32(block).map_err(|e| e.to_string())
    }

    pub fn lufs(&self) -> Option<f64> {
        self.ebu.loudness_global().ok().filter(|l| l.is_finite())
    }

    pub fn true_peak_db(&self, channels: usize) -> f64 {
        let peak = (0..channels as u32)
            .filter_map(|c| self.ebu.true_peak(c).ok())
            .fold(0.0f64, f64::max);
        if peak > 0.0 {
            20.0 * peak.log10()
        } else {
            SILENT_DB
        }
    }
}

/// Finds the chain's gains for `source` to reach `target` LUFS (`None`: keep its loudness,
/// peaks still under −1 dBTP) with `stages`. `progress` gets 0–1 over all the passes;
/// `cancel` stops it.
pub fn measure(
    source: &dyn Source,
    target: Option<f64>,
    stages: Stages,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Measured, String> {
    let channels = source.channels();
    // Passes: input, chain, then 1–4 checks. Progress assumes 3 and finishes at 1.
    let total = source.frames().max(1) as f64 * 3.0;
    let mut done = 0.0f64;
    let mut step = |frames: usize, progress: &mut dyn FnMut(f64)| {
        done += frames as f64;
        progress((done / total).min(0.99));
    };

    let check = |cancel: &AtomicBool| {
        if cancel.load(Ordering::Relaxed) {
            Err("cancelled".to_string())
        } else {
            Ok(())
        }
    };

    // 1. The input's loudness.
    let mut meter = Meter::new(channels, false)?;
    source.for_each_block(&mut |block| {
        check(cancel)?;
        meter.add(block)?;
        step(block.len() / channels, progress);
        Ok(())
    })?;
    let input_lufs = meter.lufs();
    let clamp = |db: f64| db.clamp(-MAX_GAIN_DB, MAX_GAIN_DB);
    let pre_gain_db = input_lufs.map_or(0.0, |l| clamp(WORK_LUFS - l));

    // Runs the chain over the whole sound and measures the result.
    let mut run = |params: Params,
                   true_peak: bool,
                   progress: &mut dyn FnMut(f64)|
     -> Result<(Option<f64>, f64), String> {
        let mut chain = Chain::new(channels, params);
        let mut meter = Meter::new(channels, true_peak)?;
        let mut buffer = Vec::new();
        source.for_each_block(&mut |block| {
            check(cancel)?;
            buffer.clear();
            buffer.extend_from_slice(block);
            chain.process(&mut buffer);
            meter.add(&buffer)?;
            step(block.len() / channels, progress);
            Ok(())
        })?;
        let mut tail = Vec::new();
        chain.flush(&mut tail);
        meter.add(&tail)?;
        Ok((meter.lufs(), meter.true_peak_db(channels)))
    };

    // 2. The chain without the limiter → the gain to the target.
    let mut params = Params {
        stages,
        pre_gain_db: pre_gain_db as f32,
        gain_db: 0.0,
        limit: false,
    };
    let (chained, _) = run(params, false, progress)?;
    let target = target.or(input_lufs);
    params.gain_db = match (target, chained) {
        (Some(t), Some(l)) => clamp(t - l) as f32,
        _ => -pre_gain_db as f32,
    };
    params.limit = true;

    // 3. The full chain, checked and corrected.
    // The limiter takes part of each gain step back, so after the first correction the
    // step follows the measured slope (secant): loudness change per dB of gain.
    let mut result = run(params, true, progress)?;
    let mut previous: Option<(f64, f64)> = None;
    for _ in 0..3 {
        let (Some(t), Some(l)) = (target, result.0) else {
            break;
        };
        if (t - l).abs() <= TOLERANCE_LU {
            break;
        }
        let gain = params.gain_db as f64;
        let slope = match previous {
            Some((g0, l0)) if (gain - g0).abs() > 1e-3 => ((l - l0) / (gain - g0)).clamp(0.2, 1.0),
            _ => 1.0,
        };
        previous = Some((gain, l));
        params.gain_db = clamp(gain + (t - l) / slope) as f32;
        result = run(params, true, progress)?;
    }
    progress(1.0);
    Ok(Measured {
        params,
        input_lufs,
        output_lufs: result.0,
        output_true_peak_db: result.1,
    })
}

/// A whole sound in memory.
#[cfg(test)]
pub struct Buffer<'a> {
    pub samples: &'a [f32],
    pub channels: usize,
}

#[cfg(test)]
impl Source for Buffer<'_> {
    fn channels(&self) -> usize {
        self.channels
    }

    fn frames(&self) -> u64 {
        (self.samples.len() / self.channels) as u64
    }

    fn for_each_block(
        &self,
        each: &mut dyn FnMut(&[f32]) -> Result<(), String>,
    ) -> Result<(), String> {
        for block in self.samples.chunks(4800 * self.channels) {
            each(block)?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::polish::{CEILING_DBTP, run};

    pub const GAIN_ONLY: Stages = Stages {
        rumble: false,
        de_ess: false,
        compress: false,
    };

    /// Speech-like test sound: bursts of a few tones at changing levels with pauses, so
    /// the loudness gating and the limiter both have work to do.
    pub fn speechy(seconds: f64, level_db: f32) -> Vec<f32> {
        let n = (seconds * 48_000.0) as usize;
        let amp = 10f32.powf(level_db / 20.0);
        (0..n)
            .flat_map(|i| {
                let t = i as f32 / 48_000.0;
                let syllable = (t * 4.0) as u32;
                let on = syllable % 5 != 4;
                let loud = [1.0, 0.5, 0.8, 0.3][(syllable % 4) as usize];
                let v = if on {
                    amp * loud
                        * ((2.0 * std::f32::consts::PI * 180.0 * t).sin() * 0.6
                            + (2.0 * std::f32::consts::PI * 720.0 * t).sin() * 0.3
                            + (2.0 * std::f32::consts::PI * 2400.0 * t).sin() * 0.1)
                } else {
                    0.0
                };
                [v, v]
            })
            .collect()
    }

    /// The result measured independently of `measure`'s own readings.
    fn check(samples: &[f32], m: &Measured) -> (f64, f64) {
        let out = run(&mut Chain::new(2, m.params), samples);
        let mut meter = Meter::new(2, true).unwrap();
        meter.add(&out).unwrap();
        (meter.lufs().unwrap(), meter.true_peak_db(2))
    }

    #[test]
    fn reaches_the_presets_under_minus_1_dbtp() {
        for (level, target) in [
            (-30.0, -16.0),
            (-12.0, -16.0),
            (-35.0, -14.0),
            (-6.0, -14.0),
        ] {
            let sound = speechy(20.0, level);
            let m = measure(
                &Buffer {
                    samples: &sound,
                    channels: 2,
                },
                Some(target),
                GAIN_ONLY,
                &AtomicBool::new(false),
                &mut |_| {},
            )
            .unwrap();
            let (lufs, peak) = check(&sound, &m);
            assert!(
                (lufs - target).abs() <= 0.5,
                "{level} dB → {lufs:.2} LUFS, not {target}"
            );
            assert!(peak <= -1.0, "{level} dB → true peak {peak:.2} dBTP");
            assert!(peak <= CEILING_DBTP as f64 + 0.2);
        }
    }

    #[test]
    fn keep_keeps_the_loudness() {
        let sound = speechy(20.0, -20.0);
        let m = measure(
            &Buffer {
                samples: &sound,
                channels: 2,
            },
            None,
            GAIN_ONLY,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        let (lufs, _) = check(&sound, &m);
        assert!(
            (lufs - m.input_lufs.unwrap()).abs() <= 0.5,
            "{lufs} vs {:?}",
            m.input_lufs
        );
    }

    #[test]
    fn silence_and_cancel() {
        let silence = vec![0.0f32; 96_000];
        let m = measure(
            &Buffer {
                samples: &silence,
                channels: 2,
            },
            Some(-16.0),
            GAIN_ONLY,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(m.input_lufs, None);
        assert_eq!(m.output_true_peak_db, SILENT_DB);
        let cancelled = measure(
            &Buffer {
                samples: &silence,
                channels: 2,
            },
            Some(-16.0),
            GAIN_ONLY,
            &AtomicBool::new(true),
            &mut |_| {},
        );
        assert!(cancelled.is_err());
    }
}
