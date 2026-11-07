//! Detector-model measurements: the properties the audio claims to have.
//!
//! The audio is the deliverable, but a Poisson source that has been mis-scaled, or a
//! dead-time veto with an off-by-one interval, still *sounds* like a Geiger counter.
//! These measurements describe the process rather than the timbre, so a report can
//! state what the incident stream and the tube actually did.

use crate::poisson::{DeadTime, PoissonSource};

/// Statistics of a point process over an observation window.
#[derive(Clone, Copy, Debug, Default)]
pub struct PointStats {
    pub count: usize,
    /// Observation window, seconds.
    pub window: f64,
    /// Measured mean inter-arrival time, seconds.
    pub mean_interval: f64,
    /// Measured standard deviation of inter-arrival time, seconds.
    pub std_interval: f64,
    /// `std_interval / mean_interval`.
    ///
    /// Zero for a perfectly regular train, one for a Poisson process. That ratio is
    /// what the ear hears as randomness rather than a buzz at a fixed pitch, so it is
    /// reported next to the rate instead of the rate alone.
    pub cv: f64,
}

impl PointStats {
    fn from_gaps(gaps: &[f64], window: f64) -> Self {
        if gaps.is_empty() {
            return Self::default();
        }
        let n = gaps.len() as f64;
        let mean = gaps.iter().sum::<f64>() / n;
        let var = gaps.iter().map(|&x| (x - mean) * (x - mean)).sum::<f64>() / n;
        let sd = var.sqrt();
        Self {
            count: gaps.len(),
            window,
            mean_interval: mean,
            std_interval: sd,
            cv: if mean > 0.0 { sd / mean } else { 0.0 },
        }
    }
}

/// Sample a homogeneous Poisson process at `rate`, collecting `count` intervals.
pub fn homogeneous_point_stats(seed: u64, rate: f64, count: usize) -> PointStats {
    assert!(rate > 0.0 && count >= 2);
    let mut src = PoissonSource::new(seed, rate, DeadTime::NONE);
    let mut ts = Vec::with_capacity(count + 1);
    while ts.len() <= count {
        let (t, ok) = src.pop_event(|_| rate);
        if ok {
            ts.push(t);
        }
    }
    let gaps: Vec<f64> = ts.windows(2).map(|w| w[1] - w[0]).collect();
    PointStats::from_gaps(&gaps, ts[count] - ts[0])
}

/// Fraction of incident events passed at a constant rate: measured against the
/// simulated veto over `seconds`, comparable to `DeadTime::efficiency`.
pub fn dead_time_efficiency(seed: u64, rate: f64, tau: f64, seconds: f64) -> f64 {
    let mut src = PoissonSource::new(seed, rate, DeadTime::new(tau));
    while src.next_event_time() <= seconds {
        src.pop_event(|_| rate);
    }
    src.accepted() as f64 / src.incident() as f64
}
