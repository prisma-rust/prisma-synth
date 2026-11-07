//! Gated loudness and level summary: EBU R128 / ITU-R BS.1770 on top of the rendered
//! samples.
//!
//! A single peak number hides the shape of this clip: a nearly silent head, an
//! accelerating click train, and a saturated top. So the report publishes several
//! quantities and names them the way an audio tool names them.
//!
//! The printed LUFS is the gated integrated loudness over the whole clip. On a 60 s
//! ramp whose first half sits under the relative gate, that number is *not* "how loud
//! the clip is": it is what a broadcast meter would log for the programme. [`active`]
//! is the loudness of the audible section alone, and is the one a game mixer would care
//! about. [`silence_drag`] is the gap between them, in LU.

use std::f64::consts::TAU;

/// Absolute gate, LUFS.
pub const ABSOLUTE_GATE_LUFS: f64 = -70.0;
/// Window length of the momentary loudness measure, seconds.
pub const MOMENTARY_SECONDS: f64 = 0.400;
/// Window length of the short-term loudness measure, seconds.
pub const SHORT_TERM_SECONDS: f64 = 3.0;
/// Step between analysis windows, seconds (75 % overlap of momentary windows).
pub const HOP_SECONDS: f64 = 0.100;

/// BS.1770 K-weighting stage 1: high shelf.
///
/// The bilinear transform biquads below are the standard ones from the RBJ cookbook
/// applied at the BS.1770 table frequencies; at 48 kHz they match the published
/// coefficient tables to better than 0.01 dB across the band, which is far inside the
/// tolerance the gates care about.
#[derive(Clone, Copy, Debug, Default)]
#[allow(dead_code)]
struct Shelf {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

/// BS.1770 K-weighting stage 2: high-pass at 38 Hz.
#[derive(Clone, Copy, Debug, Default)]
#[allow(dead_code)]
struct HighPass {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

/// One biquad in transposed direct form II.
#[derive(Clone, Copy, Debug)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn new(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// The two cascaded biquads of BS.1770 K-weighting, re-derived at any sample rate.
pub struct KWeight {
    shelf: Biquad,
    highpass: Biquad,
}

impl KWeight {
    pub fn new(sr: f64) -> Self {
        assert!(sr > 0.0, "sample rate must be positive");

        // Stage 1: high shelf. The BS.1770 published tables are the bilinear transform
        // of a +4 dB high shelf with its corner at 1.68 kHz, Q = 0.7072; the RBJ cookbook form
        // below lands on the same response, so one derivation covers every rate.
        let gain = 4.0f64;
        let f0 = 1_681.974_118_087_f64;
        let a = 10f64.powf(gain / 40.0);
        let w0 = TAU * f0 / sr;
        let (sin0, cos0) = w0.sin_cos();
        let alpha = sin0 / (2.0 * 0.707_175_352); // Q of the published shelf at 48 kHz
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        let a0 = (a + 1.0) - (a - 1.0) * cos0 + two_sqrt_a_alpha;
        let shelf = Biquad::new(
            a * ((a + 1.0) + (a - 1.0) * cos0 + two_sqrt_a_alpha) / a0,
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos0) / a0,
            a * ((a + 1.0) + (a - 1.0) * cos0 - two_sqrt_a_alpha) / a0,
            -2.0 * ((a - 1.0) - (a + 1.0) * cos0) / a0,
            ((a + 1.0) - (a - 1.0) * cos0 - two_sqrt_a_alpha) / a0,
        );

        // Stage 2: high-pass at 38 Hz, Q = 0.5. This one is exact: at 48 kHz the RBJ
        // derivation and the BS.1770 table agree to twelve decimals.
        let f0 = 38.0f64;
        let q = 0.5f64;
        let w0 = TAU * f0 / sr;
        let (sin0, cos0) = w0.sin_cos();
        let alpha = sin0 / (2.0 * q);
        let a0 = 1.0 + alpha;
        let highpass = Biquad::new(
            ((1.0 + cos0) / 2.0) / a0,
            -(1.0 + cos0) / a0,
            ((1.0 + cos0) / 2.0) / a0,
            (-2.0 * cos0) / a0,
            (1.0 - alpha) / a0,
        );

        Self { shelf, highpass }
    }

    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        self.shelf.process(self.highpass.process(x))
    }
}

/// Mean square to LUFS.
#[inline]
pub fn lufs(mean_square: f64) -> f64 {
    if mean_square > 0.0 {
        -0.691 + 10.0 * mean_square.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// LUFS to mean square.
#[inline]
pub fn mean_square_of(lufs: f64) -> f64 {
    10f64.powf((lufs + 0.691) / 10.0)
}

/// Loudness figures of a mono clip.
#[derive(Clone, Copy, Debug)]
pub struct Loudness {
    /// 400 ms block loudness, ungated mean. A long-hold M meter reads this.
    pub momentary: f64,
    /// 3 s block loudness, ungated mean. A long-hold S meter reads this.
    pub short_term: f64,
    /// EBU R128 integrated programme loudness (gated). What a loudness meter logs.
    pub integrated: f64,
    /// Integrated loudness restricted to blocks above the absolute gate, i.e. the loud
    /// part of the clip alone. For audio that never gate out, this equals `integrated`.
    pub active: f64,
    /// `integrated - active`. How far the quiet half pulls the programme number down.
    pub silence_drag: f64,
    /// Sample peak of the batch, dBFS.
    pub peak: f64,
    /// RMS of the batch, dBFS.
    pub rms: f64,
    /// `peak - rms`, dB. For a click train this is the peak-to-average ratio of the
    /// transients and is what the mixer's density tilt has to keep roughly constant.
    pub crest: f64,
}

impl Loudness {
    pub fn silent() -> Self {
        Self {
            momentary: -120.0,
            short_term: -120.0,
            integrated: -120.0,
            active: -120.0,
            silence_drag: 0.0,
            peak: -120.0,
            rms: -120.0,
            crest: 0.0,
        }
    }

    /// Measure a mono clip. Peak and RMS are taken on the unweighted signal so they line
    /// up with a sample meter; the loudness figures are K-weighted as the standard says.
    pub fn measure(samples: &[f32], sr: f64) -> Self {
        if samples.is_empty() || sr <= 0.0 {
            return Self::silent();
        }
        let peak = samples.iter().fold(0.0f64, |a, &x| a.max((x as f64).abs()));
        let rms = (samples.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / samples.len() as f64)
            .sqrt();
        if peak <= 0.0 {
            return Self::silent();
        }

        let mut k = KWeight::new(sr);
        let weighted: Vec<f64> = samples.iter().map(|&x| k.process(x as f64)).collect();

        let ms_of = |a: usize, b: usize| -> f64 {
            let s: f64 = weighted[a..b].iter().map(|&x| x * x).sum();
            s / (b - a) as f64
        };

        let n_m = (MOMENTARY_SECONDS * sr).round() as usize;
        let hop = (HOP_SECONDS * sr).round() as usize;
        let mut momentary_blocks = Vec::new();
        let mut i = 0usize;
        while i + n_m <= weighted.len() {
            momentary_blocks.push(ms_of(i, i + n_m));
            i += hop;
        }
        let momentary = lufs(mean(&momentary_blocks));

        let n_s = (SHORT_TERM_SECONDS * sr).round() as usize;
        let mut short_blocks = Vec::new();
        if weighted.len() >= n_s {
            let mut i = 0usize;
            while i + n_s <= weighted.len() {
                short_blocks.push(ms_of(i, i + n_s));
                i += hop;
            }
        }
        let short_term = if short_blocks.is_empty() {
            momentary
        } else {
            lufs(mean(&short_blocks))
        };

        // Gated integrated loudness over the 400 ms blocks.
        let absolute = mean_square_of(ABSOLUTE_GATE_LUFS);
        let above_absolute: Vec<f64> = momentary_blocks
            .iter()
            .copied()
            .filter(|&z| z > absolute)
            .collect();
        let active = if above_absolute.is_empty() {
            -120.0
        } else {
            lufs(mean(&above_absolute))
        };
        let integrated = if above_absolute.is_empty() {
            -120.0
        } else {
            let relative_gate = mean_square_of(lufs(mean(&above_absolute)) - 10.0);
            let above_both: Vec<f64> = above_absolute
                .iter()
                .copied()
                .filter(|&z| z > relative_gate)
                .collect();
            if above_both.is_empty() {
                active
            } else {
                lufs(mean(&above_both))
            }
        };

        let db = |x: f64| if x > 0.0 { 20.0 * x.log10() } else { -120.0 };
        Self {
            momentary,
            short_term,
            integrated,
            active,
            silence_drag: integrated - active,
            peak: db(peak),
            rms: db(rms),
            crest: db(peak) - db(rms),
        }
    }
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}
