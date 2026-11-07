//! Deterministic RNG and exponential-distribution sampling.

/// SplitMix64. Small, fast, no dependencies, and trivially reproducible: the
/// same `seed` yields the same stream on every platform and every Rust version,
/// which an offline renderer needs to produce bit-identical output.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Rebuild from a raw state, for hosts that own the state themselves (a C ABI wrapper, a
    /// game saving RNG position across a scene load). [`Rng::state`] is the inverse.
    pub fn from_state(state: u64) -> Self {
        Self { state }
    }

    /// The raw state. Passing it to [`Rng::from_state`] resumes the exact same stream.
    pub fn state(&self) -> u64 {
        self.state
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in the open interval (0, 1).
    ///
    /// Open matters: `-ln(U)` diverges at a closed end, and 0.0 is exactly what
    /// `bits as f64 / 2^53` returns for some draws. Taking 53 bits above the
    /// bottom and centring the result in the cell keeps every value strictly
    /// positive and strictly below 1.
    #[inline]
    pub fn next_f64_open(&mut self) -> f64 {
        let bits = self.next_u64() >> 11;
        (bits as f64 + 0.5) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// Uniform in [0, 1).
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }

    /// Uniform in [0, 1).
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / 16_777_216.0)
    }

    /// Bipolar noise in [-1, 1).
    #[inline]
    pub fn bipolar(&mut self) -> f32 {
        self.next_f32() * 2.0 - 1.0
    }

    /// Standard normal deviate (Box-Muller). About 1.5x the cost of a uniform
    /// draw, called a handful of times per click.
    #[inline]
    pub fn gaussian(&mut self) -> f32 {
        let u = self.next_f64_open();
        let v = self.next_f64();
        ((-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()) as f32
    }

    /// Waiting time from an exponential distribution with rate `lambda`
    /// (events per second): `dt = -ln(U) / lambda`.
    #[inline]
    pub fn exponential_interval(&mut self, lambda: f64) -> f64 {
        -self.next_f64_open().ln() / lambda
    }
}
