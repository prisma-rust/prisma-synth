//! Derived statistics of a rendered [`Track`].
//!
//! Everything here is *measured* from the rendered samples or from the onset list
//! the renderer produced; almost nothing is re-derived from the ramp formula. The
//! report therefore doubles as a check that the generator, the dead-time veto and
//! the mixer agree with each other.

use crate::Track;

/// Rollup over a fixed-width time slice.
#[derive(Clone, Copy, Debug)]
pub struct Epoch {
    pub start: f64,
    pub end: f64,
    /// Incident CPM at the start of the slice.
    pub incident_cpm: f64,
    /// Click rate actually mixed inside the slice, CPM.
    pub clicks_per_min: f64,
    /// Clicks actually mixed inside the slice.
    pub accepted: u64,
    /// Clicks per second inside the slice, measured.
    pub accepted_cps: f64,
    /// Registration fraction at the slice's incident rate, from the model.
    pub efficiency: f64,
    pub rms: f64,
    pub peak: f64,
}

impl Track {
    /// Split the track into `epoch`-second slices and measure each.
    pub fn epochs(&self, epoch: f64) -> Vec<Epoch> {
        assert!(epoch > 0.0, "epoch must be positive");
        let sr = self.sr as f64;
        let total = self.samples.len() as f64 / sr;
        let mut out = Vec::new();
        let mut t0 = 0.0;
        while t0 < total - 1e-9 {
            let t1 = (t0 + epoch).min(total);
            let i0 = (t0 * sr) as usize;
            let i1 = ((t1 * sr) as usize).min(self.samples.len());
            let n = i1.saturating_sub(i0).max(1);
            let block = if i1 > i0 { &self.samples[i0..i1] } else { &[] };
            let rms =
                (block.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / n as f64).sqrt();
            let peak = block.iter().fold(0.0f64, |a, &s| a.max(s.abs() as f64));
            let accepted = self.accepted.iter().filter(|&&t| t >= t0 && t < t1).count() as u64;
            let incident_cpm = self.incident_at(t0);
            let dur = (t1 - t0).max(1e-12);
            out.push(Epoch {
                start: t0,
                end: t1,
                incident_cpm,
                clicks_per_min: accepted as f64 / dur * 60.0,
                accepted,
                accepted_cps: accepted as f64 / dur,
                efficiency: self.dead.efficiency(incident_cpm / 60.0),
                rms,
                peak,
            });
            t0 += epoch;
        }
        out
    }

    /// Whole-track summary.
    pub fn stats(&self) -> TrackStats {
        let dead = self.dead;
        TrackStats {
            seconds: self.samples.len() as f64 / self.sr as f64,
            sr: self.sr,
            tau_us: dead.tau * 1e6,
            incident_events: self.incident,
            accepted_events: self.accepted.len() as u64,
            ceiling_cpm: dead.ceiling() * 60.0,
            incident_cpm_end: self.incident_cpm.last().copied().unwrap_or(0.0),
            displayed_cpm_end: self.displayed_cpm.last().copied().unwrap_or(0.0),
            efficiency_end: dead
                .efficiency(self.incident_cpm.last().copied().unwrap_or(0.0) / 60.0),
            truncated: self.truncated,
        }
    }

    /// Highest incident CPM anywhere in the clip.
    pub fn max_incident_cpm(&self) -> f64 {
        self.incident_cpm.iter().copied().fold(0.0f64, f64::max)
    }

    /// Highest incident CPM divided by the natural background floor (15 cpm).
    ///
    /// That floor is the one fixed point every gamma survey meter agrees on, so
    /// it works as the zero of the ramp axis no matter whose counter this is.
    pub fn decades(&self) -> f64 {
        (self.max_incident_cpm() / 15.0).log10()
    }

    /// Largest CPM the display can show: the tube ceiling, or infinity.
    pub fn display_ceiling_cpm(&self) -> f64 {
        self.dead.ceiling() * 60.0
    }

    /// Seconds of the clip during which the incident rate is at or above the
    /// tube ceiling, i.e. the meter is pinned and the source is hotter than the
    /// display can tell you.
    pub fn pinned_seconds(&self) -> f64 {
        if self.incident_cpm.is_empty() {
            return 0.0;
        }
        let ceiling = self.display_ceiling_cpm();
        let n = self.displayed_cpm.iter().filter(|&&d| d >= ceiling).count();
        n as f64 / self.sr as f64
    }

    /// Mean and standard deviation of the incident rate over the active part of
    /// the clip (from the first click to the end), counts/s, plus the CV.
    pub fn incident_rate_summary(&self) -> (f64, f64, f64) {
        let recent = self.rate_trace();
        let n = recent.len() as f64;
        if n == 0.0 {
            return (0.0, 0.0, 0.0);
        }
        let mean = recent.iter().sum::<f64>() / n;
        let var = recent.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n;
        let sd = var.sqrt();
        (mean, sd, if mean > 0.0 { sd / mean } else { 0.0 })
    }

    /// Incident counts/s sampled once per second from the first click onward.
    pub fn rate_trace(&self) -> Vec<f64> {
        let sr = self.sr as f64;
        let start = self.accepted.first().copied().unwrap_or(0.0);
        let end = self.samples.len() as f64 / sr;
        let mut out = Vec::new();
        let mut t = start;
        while t < end {
            out.push(self.incident_at(t) / 60.0);
            t += 1.0;
        }
        out
    }
}

/// Whole-track summary numbers.
#[derive(Clone, Debug)]
pub struct TrackStats {
    pub seconds: f64,
    pub sr: u32,
    pub tau_us: f64,
    pub incident_events: u64,
    pub accepted_events: u64,
    pub ceiling_cpm: f64,
    pub incident_cpm_end: f64,
    pub displayed_cpm_end: f64,
    pub efficiency_end: f64,
    pub truncated: u64,
}

impl TrackStats {
    /// Fraction of incident events the tube registered over the whole clip.
    pub fn reporting_ratio(&self) -> f64 {
        if self.incident_events == 0 {
            1.0
        } else {
            self.accepted_events as f64 / self.incident_events as f64
        }
    }

    /// Incident events the dead-time veto dropped.
    pub fn rejected_events(&self) -> u64 {
        self.incident_events.saturating_sub(self.accepted_events)
    }

    /// Clicks lost because they started too close to the end of the buffer to
    /// play their whole decay.
    pub fn cut_events(&self) -> u64 {
        self.accepted_events.saturating_sub(self.truncated)
    }
}

impl Track {
    /// Peak of the mix before the safety gain, dBFS.
    pub fn pre_norm_peak_db(&self) -> f64 {
        if self.pre_norm_peak > 0.0 {
            20.0 * (self.pre_norm_peak as f64).log10()
        } else {
            -120.0
        }
    }
}
