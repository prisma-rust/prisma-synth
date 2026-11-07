//! Rate schedule: how the incident count rate evolves over the clip.
//!
//! The rate is piecewise exponential in time, i.e. linear in `log(lambda)`. That
//! is the natural axis for a dose ramp: every decade of count rate takes the same
//! amount of time, so the click accelerando sounds even to the ear instead of
//! crawling through the low decades and exploding at the top. Anchors are
//! `(seconds, incident_cpm)`; between them `log(lambda)` is lerped.

/// Piecewise log-linear schedule of incident rates.
#[derive(Clone, Debug)]
pub struct Ramp {
    anchors: Vec<(f64, f64)>,
    end: f64,
}

impl Ramp {
    pub fn new(anchors: Vec<(f64, f64)>) -> Self {
        assert!(anchors.len() >= 2, "need at least two anchors");
        for w in anchors.windows(2) {
            assert!(w[0].0 < w[1].0, "anchors must be strictly increasing");
        }
        for a in &anchors {
            assert!(a.1 > 0.0, "incident cpm must be positive");
        }
        let end = anchors[anchors.len() - 1].0;
        Self { anchors, end }
    }

    /// Background, exponential climb, a hold at the top, then a fast decay.
    ///
    /// Ten seconds of background (20 cpm, a click every 3 s on average), then 46
    /// seconds of climb spanning 4.8 decades to 1.2e6 cpm (20 kcps incident, i.e.
    /// three times the saturation ceiling), then a 2.8 s plateau where the tube is
    /// completely saturated and the display stops moving, then back to background.
    ///
    /// The two things worth hearing are the last ten seconds of the ramp, where the
    /// tube starts rejecting more events than it passes and the click train turns
    /// from bells into a hiss, and the decay, where the recovery walks backwards
    /// through that transition one click at a time. The plateau is what makes the
    /// drop legible: without it the whole clip sounds like one continuous crescendo.
    pub fn demo() -> Self {
        Self::new(vec![
            (0.0, 20.0),
            (10.0, 20.0),
            (56.0, 1_200_000.0),
            (58.4, 1_200_000.0),
            (60.0, 20.0),
        ])
    }

    /// Continuous climb, no hold and no decay: the top of the span is the end.
    pub fn ramp_only(start_cpm: f64, end_cpm: f64, seconds: f64) -> Self {
        Self::new(vec![(0.0, start_cpm), (seconds, end_cpm)])
    }

    #[inline]
    pub fn end_time(&self) -> f64 {
        self.end
    }

    /// Incident rate, events per minute, at `t` seconds.
    pub fn incident_cpm(&self, t: f64) -> f64 {
        let t = t.clamp(0.0, self.end);
        let n = self.anchors.len();
        if t <= self.anchors[0].0 {
            return self.anchors[0].1;
        }
        if t >= self.anchors[n - 1].0 {
            return self.anchors[n - 1].1;
        }
        let i = match self
            .anchors
            .binary_search_by(|a| a.0.partial_cmp(&t).unwrap())
        {
            Ok(k) => k,
            Err(k) => k - 1,
        };
        let (t0, c0) = self.anchors[i];
        let (t1, c1) = self.anchors[i + 1];
        let u = (t - t0) / (t1 - t0);
        c0 * (c1 / c0).powf(u)
    }

    /// Incident rate, events per second, at `t` seconds.
    #[inline]
    pub fn incident_rate(&self, t: f64) -> f64 {
        self.incident_cpm(t) / 60.0
    }
}
