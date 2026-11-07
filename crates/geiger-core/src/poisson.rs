//! Inhomogeneous Poisson point process + Geiger-Muller dead time.
//!
//! Radioactive decay is memoryless, so ionising events arrive as a Poisson
//! process: intervals are exponential and the count in any window is Poisson.
//! When the incident rate `lambda(t)` is constant, a single draw
//! `dt = -ln(U)/lambda` advances the stream exactly.
//!
//! In an audio renderer the rate is never constant: it is a function of time and
//! can change inside a single block. [`PoissonSource`] therefore exposes
//! [`pop_event`](PoissonSource::pop_event), which hands back one incident time and
//! takes a closure for the rate at that instant. The next interval is drawn from
//! the rate in force where the previous event landed, and the rate changes between
//! events have zero probability. This thinning-by-construction is exact for any
//! piecewise rate curve the caller can evaluate, which is why the renderer works in
//! event time: it pulls the same number of samples out of the RNG no matter how
//! many audio blocks are processed between events, so the stream is
//! bit-reproducible.
//!
//! # Dead time
//!
//! A GM tube has a quench interval `tau`. Once an avalanche fires, the positive ion
//! sheath around the anode masks the electric field; an event arriving inside `tau`
//! cannot start a new avalanche and is simply not counted. For tau = 150 us the
//! tube recovers completely, so the non-paralyzable model
//!
//! ```text
//! r_obs = r_true / (1 + r_true * tau)
//! ```
//!
//! applies and the observed rate saturates at `1/tau`. A real tube on a plateau
//! usually has partial recovery inside `tau` instead, and "x-ray doubling" lets a
//! small secondary pulse leak through, which is why the ceiling of a cheap counter
//! is soft rather than abrupt. The demo uses the clean non-paralyzable veto: at
//! 20 kcps incident the tube withholds 95% of everything that arrives.
//!
//! # Saturating the front end
//!
//! A real instrument does not know the incident rate of the world; it knows its own
//! count rate. The display inverse-corrects `r_obs` to `r_true` while `r_obs` is
//! below the ceiling, and pins at the ceiling above it. Above saturation the click
//! rate stops being proportional to dose. The API mirrors that: [`DeadTime`] and
//! [`PoissonSource`].

use crate::rng::Rng;

/// Non-paralyzable dead-time model for a Geiger-Muller tube.
///
/// Each discharge blocks the tube for `tau` seconds regardless of what lands
/// inside the window (the "extending" or non-paralyzable model). This is the one
/// that matches real GM counters: the observed rate saturates smoothly at
/// `1/tau` instead of collapsing the way a paralyzable (reset-on-every-hit) tube
/// would.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeadTime {
    /// Blocked interval after each counted discharge, seconds.
    pub tau: f64,
}

impl DeadTime {
    /// Typical quench-tube recovery: 150 us. Ceiling 6667 cps = 400 kCPM.
    pub const TYPICAL: DeadTime = DeadTime { tau: 150e-6 };

    /// 50 us: fast quench, hot tube. Ceiling 20 kcps = 1.2 MCPM.
    pub const FAST: DeadTime = DeadTime { tau: 50e-6 };

    /// No dead time. Use only for comparing models.
    pub const NONE: DeadTime = DeadTime { tau: 0.0 };

    pub fn new(tau: f64) -> Self {
        Self { tau }
    }

    /// True when the model passes every incident event through.
    #[inline]
    pub fn is_ideal(&self) -> bool {
        self.tau <= 0.0
    }

    /// Saturation ceiling of the observed rate, counts/s. Infinite when there is
    /// no dead time.
    #[inline]
    pub fn ceiling(&self) -> f64 {
        if self.is_ideal() {
            f64::INFINITY
        } else {
            1.0 / self.tau
        }
    }

    /// Fraction of incident events the tube manages to register.
    #[inline]
    pub fn efficiency(&self, true_rate: f64) -> f64 {
        if self.is_ideal() {
            1.0
        } else {
            1.0 / (1.0 + true_rate * self.tau)
        }
    }

    /// Observed (registerable) rate from the incident rate.
    #[inline]
    pub fn observed_rate(&self, true_rate: f64) -> f64 {
        if self.is_ideal() {
            true_rate
        } else {
            true_rate / (1.0 + true_rate * self.tau)
        }
    }

    /// Incident rate from the observed rate.
    ///
    /// `None` at or above the ceiling: a saturated counter cannot invert its way
    /// out of its own limit, and neither can this.
    #[inline]
    pub fn true_rate(&self, observed_rate: f64) -> Option<f64> {
        if self.is_ideal() {
            if observed_rate.is_finite() {
                Some(observed_rate)
            } else {
                None
            }
        } else if observed_rate >= 1.0 / self.tau {
            None
        } else {
            Some(observed_rate / (1.0 - observed_rate * self.tau))
        }
    }
}

/// Sampled Poisson process whose rate may change between events.
///
/// `time` is the absolute time (seconds since start) of the next incident event.
/// Pull events one at a time and evaluate the rate at each returned instant.
#[derive(Clone, Debug)]
pub struct PoissonSource {
    rng: Rng,
    time: f64,
    dead: DeadTime,
    incident: u64,
    accepted: u64,
    last_accepted: f64,
}

impl PoissonSource {
    /// `rate` is the incident rate (events/s) in force at t = 0.
    pub fn new(seed: u64, rate: f64, dead: DeadTime) -> Self {
        assert!(rate > 0.0, "rate must be positive");
        let mut rng = Rng::new(seed);
        let first = rng.exponential_interval(rate);
        Self {
            rng,
            time: first,
            dead,
            incident: 0,
            accepted: 0,
            last_accepted: f64::NEG_INFINITY,
        }
    }

    /// Absolute time (seconds since start) of the next incident event.
    #[inline]
    pub fn next_event_time(&self) -> f64 {
        self.time
    }

    /// Seconds since the last accepted event, for diagnostics.
    #[inline]
    pub fn since_last_accepted(&self) -> f64 {
        self.time - self.last_accepted
    }

    #[inline]
    pub fn dead_time(&self) -> DeadTime {
        self.dead
    }

    /// Incident events generated so far, accepted or not.
    #[inline]
    pub fn incident(&self) -> u64 {
        self.incident
    }

    /// Incident events the tube actually registered.
    #[inline]
    pub fn accepted(&self) -> u64 {
        self.accepted
    }

    /// Incident events dropped by the dead-time veto.
    #[inline]
    pub fn rejected(&self) -> u64 {
        self.incident - self.accepted
    }

    /// Generate the next incident event and move the clock to it.
    ///
    /// `rate_at` is evaluated once, at the event instant, to draw the following
    /// wait. Returns whether the tube registered the pulse.
    pub fn pop_event(&mut self, mut rate_at: impl FnMut(f64) -> f64) -> (f64, bool) {
        let t = self.time;
        let lambda = rate_at(t);
        assert!(
            lambda.is_finite() && lambda > 0.0,
            "rate must be positive and finite at t = {t}, got {lambda}"
        );
        self.incident += 1;
        let ok = self.dead.is_ideal() || t - self.last_accepted >= self.dead.tau;
        if ok {
            self.last_accepted = t;
            self.accepted += 1;
        }
        self.time = t + self.rng.exponential_interval(lambda);
        (t, ok)
    }
}
