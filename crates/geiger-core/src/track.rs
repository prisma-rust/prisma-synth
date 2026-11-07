//! Renders one track: Poisson source -> click synth -> mix -> master.
//!
//! # Rate ramp
//!
//! [`Ramp::demo`] starts at natural background (20 cpm) and climbs exponentially
//! to an incident rate of 1.2e6 cpm (20 kcps) by t = 56 s, past this tube's
//! saturation ceiling of `1/tau` = 6.67 kcps = 400 kcpm. The displayed figure
//! therefore never reaches the source level: it saturates and stops while the
//! source keeps getting hotter. The source then holds at the top for 1.6 s and
//! decays back to background by 60 s. The hold is there because the edge of the
//! ramp is where a dense pulse train stops reading as clicks and starts reading as
//! a tone; the decay lets the listener hear the tube recovering one click at a
//! time.
//!
//! # Loudness coupling
//!
//! A real counter's speaker is driven by the same comparator output at a fixed
//! pulse width, so perceived loudness is roughly constant and only the rate
//! changes. Mixing every event at unity gain is that model, and at hundreds of
//! clicks/s the density itself raises the RMS. Without coupling the demo would be
//! inaudible for the first 15 s and then clip. [`Track`] therefore applies a
//! mid-level linear tilt, [`density_gain`]: -9 dB at 1 c/s rising to 0 dB at
//! 10 c/s so isolated clicks are audible, flat up to 1000 cps so the ramp is heard as
//! a ramp and not as a volume automation, and -15 dB down to the tube ceiling
//! (6667 cps) so the pile-up stays out of the limiter. This is a deliberate
//! *musical* choice, not a physical one. The tanh knee is the only other nonlinear
//! stage; it engages only at the top, where dozens of clicks overlap within a few
//! milliseconds.

use crate::click::{ClickStyle, ClickSynth, CLICK_SAMPLES, DEFAULT_STYLE};
use crate::poisson::{DeadTime, PoissonSource};
use crate::ramp::Ramp;
use crate::rng::Rng;

/// Sample rate of the published track.
pub const SAMPLE_RATE: u32 = 48_000;
/// Length of the published track, seconds.
pub const DEMO_SECONDS: f64 = 60.0;

/// Gain applied to one click in the mix. An isolated click peaks around -12 dBFS
/// here, audible on a reference monitor without being startling.
pub const CLICK_GAIN: f32 = 0.5;

/// Knee of the output limiter, full scale.
const LIMIT_KNEE: f32 = 0.89;

/// Mixer tilt, linear, as a function of incident clicks per second.
///
/// Piecewise linear in `log10(lambda)` between anchor rates:
///
/// | click rate | tilt  |
/// |------------|-------|
/// | 1 c/s      | -9 dB |
/// | 10 c/s     |  0 dB |
/// | 1000 c/s    |  0 dB |
/// | 6667 c/s    | -15 dB |
///
/// Below 10 c/s the mix is cut so the sparse background does not sit at the same
/// loudness as the dense middle; past 1000 c/s it is cut so the top does not turn
/// into a brown-noise roar. The ceiling anchor sits where the click train is
/// already continuous in the source itself, so everything above it is texture, not
/// more events.
#[inline]
pub fn density_gain(lambda: f64) -> f32 {
    const LO: f64 = 1.0;
    // 0 dB is held to 1000 c/s: the tube ceiling (6667 c/s, i.e. 3.82) is where the
    // click train is already fully continuous below the source, and the tilt should
    // not start to eat the source-derived loudness before that.
    const MID: f64 = 3.079_18;
    const HI: f64 = 3.823_91;
    let l = lambda.max(1e-3).log10();
    let db = if l <= LO {
        6.0 * (l - LO)
    } else if l <= MID {
        0.0
    } else {
        // 0 dB to 1000 c/s, then -15 dB per tube ceiling (6667 c/s): the clicks
        // are already a continuous rattle well before the veto rate, so the extra
        // density cannot add loudness and only drives the limiter.
        -15.0 * ((l - MID) / (HI - MID)).min(1.0)
    };
    10f32.powf(db as f32 / 20.0)
}

/// One rendered track.
pub struct Track {
    /// Mono samples, already faded in and out.
    pub samples: Vec<f32>,
    /// Incident CPM per sample.
    pub incident_cpm: Vec<f64>,
    /// Displayed CPM per sample: dead-time corrected while the count rate is
    /// below the tube ceiling, pinned at the ceiling above it.
    pub displayed_cpm: Vec<f64>,
    /// Accepted click onsets, absolute seconds.
    pub accepted: Vec<f64>,
    pub sr: u32,
    pub dead: DeadTime,
    pub incident: u64,
    pub rejected: u64,
    /// Clicks that ran past the end of the buffer and were cut short.
    pub truncated: u64,
    /// Peak of the mix before the safety gain.
    pub pre_norm_peak: f32,
}

impl Track {
    /// Render the demo track driven by `ramp`.
    pub fn synthesize(ramp: &Ramp, seed: u64, dead: DeadTime) -> Self {
        Self::synthesize_with(ramp, seed, dead, SAMPLE_RATE as f64, DEMO_SECONDS)
    }

    /// Render the demo track with an explicit click style.
    pub fn synthesize_style(ramp: &Ramp, seed: u64, dead: DeadTime, style: ClickStyle) -> Self {
        Self::synthesize_style_with(ramp, seed, dead, style, SAMPLE_RATE as f64, DEMO_SECONDS)
    }

    pub fn synthesize_with(ramp: &Ramp, seed: u64, dead: DeadTime, sr: f64, seconds: f64) -> Self {
        Self::synthesize_style_with(ramp, seed, dead, DEFAULT_STYLE, sr, seconds)
    }

    pub fn synthesize_style_with(
        ramp: &Ramp,
        seed: u64,
        dead: DeadTime,
        style: ClickStyle,
        sr: f64,
        seconds: f64,
    ) -> Self {
        assert!(sr > 0.0 && seconds > 0.0);
        let n = (seconds * sr).round() as usize;

        // 1. Incident stream -> accepted events -> one click waveform per event.
        let mut source = PoissonSource::new(seed, ramp.incident_rate(0.0), dead);
        let mut rng = Rng::new(seed ^ 0xDEAD_BEEF_0000_0001);
        let synth = ClickSynth::with_style(sr, style);
        let mut clicks: Vec<[f32; CLICK_SAMPLES]> = Vec::new();
        let mut accepted: Vec<f64> = Vec::new();
        while source.next_event_time() <= seconds {
            let (t, ok) = source.pop_event(|t| ramp.incident_rate(t));
            if ok {
                let mut w = [0.0f32; CLICK_SAMPLES];
                synth.render(&mut rng, &mut w);
                clicks.push(w);
                accepted.push(t);
            }
        }

        // 2. Per-sample rates and density tilt.
        let mut incident_cpm = Vec::with_capacity(n);
        let mut displayed_cpm = Vec::with_capacity(n);
        let mut density = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f64 / sr;
            let lambda = ramp.incident_rate(t);
            incident_cpm.push(lambda * 60.0);
            let observed = dead.observed_rate(lambda);
            displayed_cpm.push(match dead.true_rate(observed) {
                Some(u) => u * 60.0,
                None => observed * 60.0,
            });
            density.push(density_gain(lambda));
        }

        // 3. Mix: sum every click at its sample-accurate onset.
        let mut buf = vec![0.0f32; n];
        let mut truncated = 0u64;
        for (t, click) in accepted.iter().zip(clicks.iter()) {
            let i0 = (t * sr).round() as isize;
            let mut written = 0usize;
            for (k, &c) in click.iter().enumerate() {
                let i = i0 + k as isize;
                if i < 0 {
                    continue;
                }
                if i as usize >= n {
                    break;
                }
                buf[i as usize] += c * CLICK_GAIN * density[i as usize];
                written += 1;
            }
            if written < CLICK_SAMPLES {
                truncated += 1;
            }
        }

        // 4. Master.
        //
        // Every clip is peak-normalised to the same ceiling, whatever rate it runs
        // at. That is what a real counter does, by construction: the comparator's
        // output is a fixed-width pulse at rail, and the speaker reproduces *that*
        // pulse with the same amplitude whether one click per second or one thousand
        // arrives. The density tilt truncated loudness coupling on purpose because a
        // game needs the quiet head audible, but the masters must not drift in level
        // as the tilt saturates, and without re-normalising each clip the "0 on the
        // knob" would become +15 dBFS at the top.
        //
        // Peak-normalise per clip, then apply a static static soft knee at 0.89. The
        // knee costs almost nothing at this crest factor: clicks are 25 dB peaky, so
        // only the single loudest rarefaction of the whole clip is anywhere near it.
        let peak = buf.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
        if peak > 0.0 {
            const RAIL: f64 = 0.95;
            let scale = 0.95 / peak as f64;
            for s in buf.iter_mut() {
                let z = *s as f64 * scale;
                let ax = z.abs();
                let mag = if ax <= LIMIT_KNEE as f64 {
                    ax
                } else {
                    let knee = LIMIT_KNEE as f64 + (ax - LIMIT_KNEE as f64) / 3.0;
                    knee.min(RAIL)
                };
                *s = mag.copysign(z) as f32;
            }
        }

        Self {
            samples: buf,
            incident_cpm,
            displayed_cpm,
            accepted,
            sr: sr as u32,
            dead,
            incident: source.incident(),
            rejected: source.rejected(),
            truncated,
            pre_norm_peak: peak,
        }
    }

    /// Displayed CPM at time `t` seconds.
    pub fn displayed_at(&self, t: f64) -> f64 {
        let i = ((t * self.sr as f64) as usize).min(self.displayed_cpm.len().saturating_sub(1));
        self.displayed_cpm[i]
    }

    /// Incident CPM at time `t` seconds.
    pub fn incident_at(&self, t: f64) -> f64 {
        let i = ((t * self.sr as f64) as usize).min(self.incident_cpm.len().saturating_sub(1));
        self.incident_cpm[i]
    }
}
