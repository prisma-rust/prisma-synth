//! The Geiger counter click, expressed as a [`Patch`](crate::patch::Patch).
//!
//! This file used to be its own synthesiser. It is now a thin adapter: it turns a
//! [`ClickStyle`] into the same [`Patch`](crate::patch::Patch) that every other sound in the
//! crate uses, and delegates the rendering. The click is not a special case of the engine, it
//! is the engine's first preset.
//!
//! What remains here is the part that is specific to this instrument: the named styles and
//! the vocabulary for talking about them. The composite transient model they encode is
//! `A(t) * [ sum_m G_m e^(-t/tau_m) sin(2*pi*f_m t) + N(t) ]` — a strike plus the body it
//! drives, which is the same shape as a raindrop or a woodblock. See [`crate::patch`] for the
//! model itself.
//!
//! Two degenerate cases bracket the target, and getting this wrong in either direction is the
//! easiest mistake in procedural percussion:
//!
//! * oscillators only, no strike: a decaying sine, heard as a short *beep*. At any realistic
//!   rate the ear extracts the fixed pitch and reads a tone with clicks on top.
//! * strike only, no body: a burst of noise, heard as a *hiss*. It loses the instrument's
//!   identity, because there is nothing left to identify.
//!
//! A real counter is neither: a sharp broadband edge immediately followed by a very short,
//! very damped resonance from the box. The edge carries the attack; the ring carries the
//! instrument. The balance between them is a listening decision, so it lives in the named
//! styles rather than in magic constants.

use crate::patch::{Envelope, Jitter};
use crate::patch::{FilterMode, NoiseSpec, Partial, Patch, MAX_MODES};
use crate::rng::Rng;

/// Buffered length of one click, in samples at 48 kHz.
pub const CLICK_SAMPLES: usize = 384;

/// Nominal peak of one click before the mix applies `CLICK_GAIN`.
pub const CLICK_PEAK: f32 = 1.0;

/// A complete recipe for one click of a particular instrument.
///
/// The fields are the Geiger-specific spelling of the general [`Patch`]; [`ClickStyle::patch`]
/// makes the translation.
#[derive(Clone, Copy, Debug)]
pub struct ClickStyle {
    pub name: &'static str,
    pub blurb: &'static str,
    /// The body's resonant modes, as `(hz, tau, gain)`.
    pub modes: &'static [(f32, f32, f32)],
    /// Attack time constant of the envelope, seconds.
    pub attack_tau: f32,
    /// Decay time constant of the envelope, seconds.
    pub decay_tau: f32,
    /// Weight of the oscillating part relative to the noise, `B` in the model.
    pub b_weight: f32,
    /// Gain of the noise part.
    pub noise_gain: f32,
    /// Centre frequency of the noise band, Hz.
    pub noise_hz: f32,
    /// Q of the noise band.
    pub noise_q: f32,
    /// Fractional jitter of every resonance centre, per event.
    pub jitter_hz: f32,
    /// Fractional jitter of the envelope decay, per event.
    pub jitter_decay: f32,
    /// Fractional jitter of `B`, per event.
    pub jitter_b: f32,
}

impl ClickStyle {
    /// Translate to the general patch. `B` folds into the partial gains, which is exact: the
    /// oscillating part is scaled as one group, so scaling each member is the same thing.
    pub fn patch(&self) -> Patch {
        // `modes` is a slice at an unknown address, so the patch cannot borrow it statically.
        // The Geiger path therefore renders through `ClickSynth::render` below, which is the
        // single-mode form of the general engine; the named patches in `patch::PATCHES` are
        // the fully static ones. This method exists for callers that want the parameters.
        let partials: Vec<Partial> = self
            .modes
            .iter()
            .map(|&(hz, tau, gain)| Partial {
                hz,
                tau,
                gain: gain * self.b_weight,
                sweep: 1.0,
            })
            .collect();
        let noise = Some(NoiseSpec {
            hz: self.noise_hz,
            q: self.noise_q,
            tau: self.decay_tau,
            gain: self.noise_gain,
            mode: FilterMode::Band,
        });
        // Leak the mode table once per call. Only used by tools that want a `Patch` value;
        // the real-time path never calls this.
        let leaked: &'static [Partial] = Box::leak(partials.into_boxed_slice());
        Patch {
            name: self.name,
            blurb: self.blurb,
            partials: leaked,
            noise,
            env: Envelope {
                attack: self.attack_tau,
                decay: self.decay_tau,
            },
            jitter: Jitter {
                hz: self.jitter_hz,
                tau: self.jitter_decay * 0.6,
                env: self.jitter_decay,
                gain_db: 0.0,
                body: self.jitter_hz * 0.7,
            },
            length: CLICK_SAMPLES as f32 / 48_000.0,
            normalize: true,
        }
    }
}

/// The published track's style.
pub const DEFAULT_STYLE: ClickStyle = STYLE_CLACK;

/// Balanced: the model at `B = 0.6`, a mid resonance and a little over a millisecond of
/// decay. The middle of the reference ranges, and the closest to a small counter in a plastic
/// box.
pub const STYLE_CLACK: ClickStyle = ClickStyle {
    name: "clack",
    blurb: "balanced composite, 2.6 kHz, B 0.60, 1.2 ms decay",
    modes: &[(2_600.0, 0.0012, 1.00), (1_250.0, 0.0016, 0.45)],
    attack_tau: 0.00006,
    decay_tau: 0.0012,
    b_weight: 0.60,
    noise_gain: 0.55,
    noise_hz: 3_000.0,
    noise_q: 1.1,
    jitter_hz: 0.18,
    jitter_decay: 0.25,
    jitter_b: 0.20,
};

/// Harder and brighter: less resonance, more edge. A modern handheld.
pub const STYLE_HARD: ClickStyle = ClickStyle {
    name: "hard",
    blurb: "edgier composite, 3.4 kHz, B 0.42, 0.9 ms decay",
    modes: &[(3_400.0, 0.0009, 1.00), (1_600.0, 0.0012, 0.40)],
    attack_tau: 0.00005,
    decay_tau: 0.0009,
    b_weight: 0.42,
    noise_gain: 0.75,
    noise_hz: 3_800.0,
    noise_q: 1.0,
    jitter_hz: 0.20,
    jitter_decay: 0.25,
    jitter_b: 0.22,
};

/// More resonance: the crisp end, still short enough not to be a beep.
pub const STYLE_RING: ClickStyle = ClickStyle {
    name: "ring",
    blurb: "more resonant, 2.2 kHz, B 0.85, 1.8 ms decay",
    modes: &[(2_200.0, 0.0018, 1.00), (4_400.0, 0.0010, 0.35)],
    attack_tau: 0.00008,
    decay_tau: 0.0018,
    b_weight: 0.85,
    noise_gain: 0.30,
    noise_hz: 2_600.0,
    noise_q: 1.3,
    jitter_hz: 0.15,
    jitter_decay: 0.22,
    jitter_b: 0.16,
};

/// Driest: mostly edge, almost no tone. The bare-spark end of the range.
pub const STYLE_DRY: ClickStyle = ClickStyle {
    name: "dry",
    blurb: "mostly transient, 4.0 kHz, B 0.30, 0.7 ms decay",
    modes: &[(4_000.0, 0.0007, 1.00)],
    attack_tau: 0.00004,
    decay_tau: 0.0007,
    b_weight: 0.30,
    noise_gain: 0.85,
    noise_hz: 4_200.0,
    noise_q: 0.9,
    jitter_hz: 0.22,
    jitter_decay: 0.28,
    jitter_b: 0.25,
};

/// Every style, in chooser order.
pub const STYLES: &[ClickStyle] = &[STYLE_CLACK, STYLE_HARD, STYLE_RING, STYLE_DRY];

/// Look a style up by name, falling back to [`DEFAULT_STYLE`].
pub fn style_by_name(name: &str) -> ClickStyle {
    STYLES
        .iter()
        .find(|s| s.name == name)
        .copied()
        .unwrap_or(DEFAULT_STYLE)
}

/// A second-order band-pass (state variable) for the noise layer.
#[derive(Clone, Copy, Debug, Default)]
struct Bandpass {
    f: f32,
    q: f32,
    low: f32,
    band: f32,
}

impl Bandpass {
    fn new(hz: f64, q: f64, sr: f64) -> Self {
        Self {
            f: (2.0 * (std::f64::consts::PI * hz / sr).sin()) as f32,
            q: (1.0 / q) as f32,
            low: 0.0,
            band: 0.0,
        }
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        self.low += self.f * self.band;
        let high = x - self.low - self.q * self.band;
        self.band += self.f * high;
        self.band
    }
}

/// Draws clicks of one style. Every call to [`render`](Self::render) jitters afresh.
///
/// This is the single-mode form of [`crate::patch::render_event`]: the same envelope, the same
/// mode sum, the same noise band. It is kept separate only because the style's mode table is a
/// run-time slice rather than a `'static` one, and the audio path must not allocate.
#[derive(Clone, Copy, Debug)]
pub struct ClickSynth {
    sr: f64,
    style: ClickStyle,
}

impl ClickSynth {
    pub fn new(sr: f64) -> Self {
        Self::with_style(sr, DEFAULT_STYLE)
    }

    pub fn with_style(sr: f64, style: ClickStyle) -> Self {
        Self { sr, style }
    }

    pub fn style(&self) -> ClickStyle {
        self.style
    }

    /// Render one click into `buf[..CLICK_SAMPLES]`, peak-normalised to [`CLICK_PEAK`].
    pub fn render(&self, rng: &mut Rng, buf: &mut [f32]) {
        assert!(buf.len() >= CLICK_SAMPLES);
        let sr = self.sr;
        let n = CLICK_SAMPLES.min(buf.len());
        let st = &self.style;
        buf[..n].fill(0.0);

        // `f` per event. A rigid body moves as a whole, so the pitch offset is shared; the
        // independent part keeps the modes from ever locking together. The coefficient block
        // is a fixed-size stack array: this call is legal on an audio callback, which a
        // `Vec` here would not be.
        let body = 1.0 + rng.gaussian() as f64 * st.jitter_hz as f64 * 0.7;
        let mut modes = [(0.0f64, 1.0f64, 0.0f64, 0.0f64); MAX_MODES];
        let count = st.modes.len().min(MAX_MODES);
        for (m, &(hz, tau, gain)) in modes[..count].iter_mut().zip(st.modes.iter()) {
            m.0 = hz as f64 * body * (1.0 + rng.gaussian() as f64 * st.jitter_hz as f64 * 0.5);
            m.1 = (tau as f64 * (1.0 + rng.gaussian() as f64 * st.jitter_decay as f64 * 0.6))
                .max(1e-5);
            m.2 = gain as f64;
            m.3 = rng.next_f64() * std::f64::consts::TAU;
        }

        let decay = (st.decay_tau as f64 * (1.0 + rng.gaussian() as f64 * st.jitter_decay as f64))
            .max(1e-5);
        let attack = st.attack_tau as f64;
        let b =
            st.b_weight as f64 * (1.0 + rng.gaussian() as f64 * st.jitter_b as f64).clamp(0.2, 1.5);

        let mut bp = Bandpass::new(
            st.noise_hz as f64 * (0.9 + 0.2 * rng.next_f64()),
            st.noise_q as f64,
            sr,
        );
        let noise_gain = st.noise_gain as f64;

        for (i, out) in buf[..n].iter_mut().enumerate() {
            let t = i as f64 / sr;

            // A(t): near-instant attack, short decay.
            let a = if attack > 0.0 {
                (1.0 - (-t / attack).exp()) * (-t / decay).exp()
            } else {
                (-t / decay).exp()
            };

            // Oscillating part, B * sin.
            let mut osc = 0.0;
            for &(f, tau, gain, phase) in &modes[..count] {
                osc += gain * (-t / tau).exp() * (std::f64::consts::TAU * f * t + phase).sin();
            }

            // N(t): coloured noise.
            let noise = noise_gain * bp.tick(rng.bipolar()) as f64;

            *out = (a * (b * osc + noise)) as f32;
        }

        crate::patch::dc_block(buf, 120.0, sr);
        let peak = buf[..n].iter().fold(0.0f32, |a, &x| a.max(x.abs()));
        if peak > 0.0 {
            let g = CLICK_PEAK / peak;
            for x in buf[..n].iter_mut() {
                *x *= g;
            }
        }
    }
}
