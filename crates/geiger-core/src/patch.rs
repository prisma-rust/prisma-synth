//! A general event-driven synthesiser.
//!
//! Everything this project makes is the same three stages, and only the middle one changes
//! from sound to sound:
//!
//! ```text
//!   event source            voice (a patch)                 mixer
//!   -------------           ----------------                -----
//!   when does it fire  ->   what does one event sound  ->   overlap, gain, output
//! ```
//!
//! The Geiger counter is one patch and one source, but neither is special. A rain bed is the
//! same engine with a different patch and a Poisson source; footsteps are the engine with a
//! patch and a source driven by animation events; a UI tick is the engine with a patch and a
//! source driven by button presses. One synthesiser, many sounds.
//!
//! # The voice
//!
//! A patch is a small physical model of a struck object:
//!
//! ```text
//! event(t) = A(t) * [ sum_m  G_m * e^(-t/tau_m) * sin(2*pi*f_m(t)*t + phi_m)
//!                     + N(t) ]
//! ```
//!
//! * `A(t)` — the amplitude envelope, an instantaneous attack into an exponential decay. A
//!   struck object starts moving at full speed.
//! * the sum — the object's *modes*: the partials that ring. Frequency, decay and gain per
//!   mode. A metal bar has few, long, inharmonic modes; a plastic box has few, short ones; a
//!   drum has many; a beep has exactly one.
//! * `f_m(t)` — an optional pitch sweep, which is what turns a partial into a drip, a laser
//!   or a kick.
//! * `N(t)` — band-limited noise, the strike itself. The "contact" of the hammer.
//!
//! Every mode's frequency, decay and gain can be jittered per event, and the whole set can be
//! jittered as a group, which is the difference between a machine and a body.
//!
//! # The source
//!
//! A [`Source`] is anything that yields event times. [`Source::Poisson`] is the radioactive
//! one, [`Source::Regular`] is a metronome, [`Source::Times`] is whatever the game hands over
//! (foot plants, bullet impacts, button presses). The renderer never cares which.
//!
//! # What is deliberately not here
//!
//! A full synth would also have filters per voice, LFOs, modulation matrices, note tracking,
//! and continuous control. This is the smallest thing that covers percussive and one-shot
//! sounds — the ones a game needs by the thousand — and it keeps the same property as the
//! Geiger model: one event in, a bounded amount of work, no hidden state between events, so a
//! pool of voices can render it on an audio callback.

use crate::rng::Rng;

/// One resonant mode of the object: a partial that rings with its own decay.
#[derive(Clone, Copy, Debug)]
pub struct Partial {
    /// Base frequency, Hz.
    pub hz: f32,
    /// Decay time constant, seconds.
    pub tau: f32,
    /// Gain in the summed voice.
    pub gain: f32,
    /// End frequency as a multiple of `hz`, reached over the decay. `1.0` is a steady tone.
    ///
    /// Sweeps are what make a drop a drop and a kick a kick; a struck block has `1.0`.
    pub sweep: f32,
}

/// The strike itself: noise under a filter, with its own decay.
#[derive(Clone, Copy, Debug)]
pub struct NoiseSpec {
    /// Filter centre or corner, Hz.
    pub hz: f32,
    /// Filter Q. `~0.7` is a broad low pass, `>1` is a resonant band.
    pub q: f32,
    /// Decay time constant, seconds.
    pub tau: f32,
    /// Gain in the summed voice.
    pub gain: f32,
    /// Which side of `hz` the noise sits on.
    pub mode: FilterMode,
}

/// Filter response of the noise layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterMode {
    /// Low pass: rumble, footfalls, thunder.
    Low,
    /// Band pass: the "clack" of a hammer on plastic, the hiss of rain.
    Band,
    /// High pass: sparks, splashes, ticks.
    High,
}

/// Amplitude envelope: instantaneous attack into an exponential decay.
#[derive(Clone, Copy, Debug)]
pub struct Envelope {
    /// Attack time constant, seconds. Near zero is a click; a few tens of ms is a swell.
    pub attack: f32,
    /// Decay time constant, seconds.
    pub decay: f32,
}

/// Per-event variation. Constant, identical events are what make synthesis sound synthetic;
/// a real body never hits the same way twice.
#[derive(Clone, Copy, Debug)]
pub struct Jitter {
    /// Fractional jitter of every partial's frequency, per event.
    pub hz: f32,
    /// Fractional jitter of every partial's decay, per event.
    pub tau: f32,
    /// Fractional jitter of the envelope decay, per event.
    pub env: f32,
    /// dB of jitter applied to the partial gains, per event.
    pub gain_db: f32,
    /// Fractional jitter of the partial group as one body, per event.
    pub body: f32,
}

impl Jitter {
    /// No variation at all. Only right for machines and test tones.
    pub const NONE: Jitter = Jitter {
        hz: 0.0,
        tau: 0.0,
        env: 0.0,
        gain_db: 0.0,
        body: 0.0,
    };
}

/// A complete recipe for one event of one instrument.
#[derive(Clone, Copy, Debug)]
pub struct Patch {
    pub name: &'static str,
    pub blurb: &'static str,
    /// The ringing modes. Empty is legal: a pure noise sound.
    pub partials: &'static [Partial],
    /// The strike, if the sound has one.
    pub noise: Option<NoiseSpec>,
    pub env: Envelope,
    pub jitter: Jitter,
    /// Length of the voice buffer, seconds. Must cover the longest decay that matters.
    pub length: f32,
    /// Whether each event is peak-normalised. Off preserves dynamics between events.
    pub normalize: bool,
}

/// Look a patch up by name from [`PATCHES`], falling back to the Geiger click.
pub fn patch_by_name(name: &str) -> Patch {
    PATCHES
        .iter()
        .find(|p| p.name == name)
        .copied()
        .unwrap_or(PATCHES[0])
}

// --------------------------------------------------------------------------------------------
// The patch library.
//
// These are starting points, not measurements. Each one is a handful of numbers that a game
// can override per emitter; the point is that they all run on the same engine.
// --------------------------------------------------------------------------------------------

/// The Geiger counter's click, as approved: a broadband edge plus two short modes at 2.6 and
/// 1.25 kHz, a band of noise for the strike, everything gone in a few milliseconds.
pub const GEIGER: Patch = Patch {
    name: "geiger",
    blurb: "short broadband click with a 2.6 kHz plastic box ring",
    partials: &[
        Partial {
            hz: 2_600.0,
            tau: 0.0012,
            gain: 0.60,
            sweep: 1.0,
        },
        Partial {
            hz: 1_250.0,
            tau: 0.0016,
            gain: 0.27,
            sweep: 1.0,
        },
    ],
    noise: Some(NoiseSpec {
        hz: 3_000.0,
        q: 1.1,
        tau: 0.0012,
        gain: 0.55,
        mode: FilterMode::Band,
    }),
    env: Envelope {
        attack: 0.00006,
        decay: 0.0012,
    },
    jitter: Jitter {
        hz: 0.18,
        tau: 0.25,
        env: 0.25,
        gain_db: 1.6,
        body: 0.13,
    },
    length: 0.008,
    normalize: true,
};

/// A water drop: one high partial that falls in pitch over its life, with a bright splash of
/// noise at the contact. The sweep is the whole character; without it this is a beep.
pub const RAINDROP: Patch = Patch {
    name: "raindrop",
    blurb: "pitched drip sweeping down, wet noise contact",
    partials: &[
        Partial {
            hz: 1_900.0,
            tau: 0.035,
            gain: 1.00,
            sweep: 0.45,
        },
        Partial {
            hz: 2_700.0,
            tau: 0.018,
            gain: 0.35,
            sweep: 0.60,
        },
    ],
    noise: Some(NoiseSpec {
        hz: 4_500.0,
        q: 0.8,
        tau: 0.0018,
        gain: 0.40,
        mode: FilterMode::High,
    }),
    env: Envelope {
        attack: 0.0004,
        decay: 0.030,
    },
    jitter: Jitter {
        hz: 0.22,
        tau: 0.30,
        env: 0.20,
        gain_db: 2.5,
        body: 0.18,
    },
    length: 0.150,
    normalize: true,
};

/// A boot on gravel: a low thud for the mass and a band of noise for the grit. No stable
/// partials at all, which is what makes it read as a footfall rather than a note.
pub const FOOTSTEP: Patch = Patch {
    name: "footstep",
    blurb: "low thud plus gritty band of noise",
    partials: &[Partial {
        hz: 95.0,
        tau: 0.030,
        gain: 0.70,
        sweep: 0.80,
    }],
    noise: Some(NoiseSpec {
        hz: 1_400.0,
        q: 0.9,
        tau: 0.012,
        gain: 0.60,
        mode: FilterMode::Band,
    }),
    env: Envelope {
        attack: 0.0012,
        decay: 0.020,
    },
    jitter: Jitter {
        hz: 0.10,
        tau: 0.25,
        env: 0.25,
        gain_db: 3.0,
        body: 0.10,
    },
    length: 0.120,
    normalize: true,
};

/// A mallet on a wooden block: a few short inharmonic modes, no tilt to the noise. The
/// inharmonic ratios (1 : 2.31 : 3.93) are what separate wood from a bell, which would be
/// closer to 1 : 2.76 : 5.40.
pub const WOOD: Patch = Patch {
    name: "wood",
    blurb: "mallet on a woodblock, inharmonic short modes",
    partials: &[
        Partial {
            hz: 420.0,
            tau: 0.045,
            gain: 1.00,
            sweep: 1.0,
        },
        Partial {
            hz: 970.0,
            tau: 0.030,
            gain: 0.55,
            sweep: 1.0,
        },
        Partial {
            hz: 1_650.0,
            tau: 0.018,
            gain: 0.30,
            sweep: 1.0,
        },
    ],
    noise: Some(NoiseSpec {
        hz: 2_600.0,
        q: 0.7,
        tau: 0.004,
        gain: 0.35,
        mode: FilterMode::High,
    }),
    env: Envelope {
        attack: 0.0004,
        decay: 0.040,
    },
    jitter: Jitter {
        hz: 0.03,
        tau: 0.20,
        env: 0.20,
        gain_db: 1.5,
        body: 0.02,
    },
    length: 0.200,
    normalize: true,
};

/// A UI tick: one short mode, no noise, no jitter. The only patch here that is meant to sound
/// like a machine, which is why its jitter is zero.
pub const UI_TICK: Patch = Patch {
    name: "ui_tick",
    blurb: "single steady 1.6 kHz beep, no noise, no jitter",
    partials: &[Partial {
        hz: 1_600.0,
        tau: 0.020,
        gain: 1.00,
        sweep: 1.0,
    }],
    noise: None,
    env: Envelope {
        attack: 0.0008,
        decay: 0.018,
    },
    jitter: Jitter::NONE,
    length: 0.120,
    normalize: true,
};

/// An alert tone: one partial held long enough to be a pitch, with a slight fall. A pose
/// rather than a strike, and the only patch whose length is set by the note and not the body.
pub const ALERT: Patch = Patch {
    name: "alert",
    blurb: "held 880 Hz tone with a small downward lean",
    partials: &[Partial {
        hz: 880.0,
        tau: 0.200,
        gain: 1.00,
        sweep: 0.97,
    }],
    noise: None,
    env: Envelope {
        attack: 0.006,
        decay: 0.180,
    },
    jitter: Jitter::NONE,
    length: 1.200,
    normalize: true,
};

/// Distant thunder: a long low-passed noise bed with no partials. Shows that a patch does not
/// need a pitch at all.
pub const RUMBLE: Patch = Patch {
    name: "rumble",
    blurb: "long low-passed noise, no pitch",
    partials: &[],
    noise: Some(NoiseSpec {
        hz: 90.0,
        q: 0.6,
        tau: 0.900,
        gain: 1.00,
        mode: FilterMode::Low,
    }),
    env: Envelope {
        attack: 0.040,
        decay: 1.100,
    },
    jitter: Jitter {
        hz: 0.0,
        tau: 0.30,
        env: 0.30,
        gain_db: 3.0,
        body: 0.0,
    },
    length: 4.000,
    normalize: true,
};

/// Every patch, in chooser order. The Geiger click is first on purpose: it is the reference.
pub const PATCHES: &[Patch] = &[GEIGER, RAINDROP, FOOTSTEP, WOOD, UI_TICK, ALERT, RUMBLE];

// --------------------------------------------------------------------------------------------
// Event sources.
// --------------------------------------------------------------------------------------------

/// Where event times come from.
#[derive(Clone, Debug)]
pub enum Source {
    /// A Poisson process at a constant rate, events per second. Radioactive decay, rain on a
    /// roof, anything memoryless.
    Poisson { seed: u64, rate: f64 },
    /// A perfectly regular train every `interval` seconds. A metronome, a machine.
    Regular { interval: f64 },
    /// Explicit times in seconds. Whatever the game hands over: foot plants, impacts, clicks.
    Times(Vec<f64>),
}

impl Source {
    /// Fill `out` with the event times inside `[0, seconds)`.
    pub fn times(&self, seconds: f64) -> Vec<f64> {
        match self {
            Source::Poisson { seed, rate } => {
                assert!(*rate > 0.0, "rate must be positive");
                let mut rng = Rng::new(*seed);
                let mut t = rng.exponential_interval(*rate);
                let mut out = Vec::new();
                while t < seconds {
                    out.push(t);
                    t += rng.exponential_interval(*rate);
                }
                out
            }
            Source::Regular { interval } => {
                assert!(*interval > 0.0, "interval must be positive");
                let mut out = Vec::new();
                let mut t = 0.0;
                while t < seconds {
                    out.push(t);
                    t += interval;
                }
                out
            }
            Source::Times(v) => v
                .iter()
                .copied()
                .filter(|&t| (0.0..seconds).contains(&t))
                .collect(),
        }
    }
}

// --------------------------------------------------------------------------------------------
// Rendering.
// --------------------------------------------------------------------------------------------

/// Samples a patch needs for its own decay, rounded up.
pub fn patch_samples(patch: &Patch, sr: f64) -> usize {
    ((patch.length as f64 * sr).ceil() as usize).max(1)
}

/// Render one event into `buf`, overwriting it. `buf.len()` sets the length actually drawn,
/// so pass a buffer sized by [`patch_samples`].
///
/// Peak-normalises if the patch asks, so a game can keep one gain per emitter and change the
/// patch underneath without the level jumping.
pub fn render_event(patch: &Patch, rng: &mut Rng, sr: f64, buf: &mut [f32]) {
    assert!(sr > 0.0, "sample rate must be positive");
    let n = buf.len();
    if n == 0 {
        return;
    }
    buf.fill(0.0);

    // One body, hit slightly differently every time: a shared pitch offset, then per-mode
    // variation on top. The shared part is what keeps the modes in tune with each other.
    let body = 1.0 + rng.gaussian() as f64 * patch.jitter.body as f64;

    // Per-mode coefficients, on the stack. The fixed bound is what keeps this call legal on an
    // audio callback: no allocation, no length check that can fail, no branch on capacity.
    // Patches with more modes than [`MAX_MODES`] render their first `MAX_MODES` only.
    let mut modes = [Mode::IDLE; MAX_MODES];
    let count = patch.partials.len().min(MAX_MODES);
    for (m, p) in modes[..count].iter_mut().zip(patch.partials.iter()) {
        let f = p.hz as f64 * body * (1.0 + rng.gaussian() as f64 * patch.jitter.hz as f64);
        let tau = p.tau as f64 * (1.0 + rng.gaussian() as f64 * patch.jitter.tau as f64) * 0.7
            + p.tau as f64 * 0.3;
        let gain =
            p.gain as f64 * 10f64.powf(rng.gaussian() as f64 * patch.jitter.gain_db as f64 / 20.0);
        m.hz = f;
        m.tau = tau.max(1e-6);
        m.gain = gain;
        m.phase = rng.next_f64() * std::f64::consts::TAU;
        m.phase_acc = 0.0;
        m.sweep = p.sweep as f64;
    }

    let decay = patch.env.decay as f64 * (1.0 + rng.gaussian() as f64 * patch.jitter.env as f64);
    let decay = decay.max(1e-6);
    let attack = patch.env.attack as f64;

    // The noise layer's filter, per event.
    let mut filter = NoiseFilter::IDLE;
    if let Some(spec) = patch.noise {
        let (s1, s2) = svf_coeffs(
            spec.hz as f64 * (1.0 + rng.gaussian() as f64 * patch.jitter.hz as f64),
            spec.q as f64,
            sr,
        );
        filter.spec = Some(spec);
        filter.s1 = s1;
        filter.s2 = s2;
    }

    for (i, out) in buf.iter_mut().enumerate() {
        let t = i as f64 / sr;

        // A(t): instantaneous attack, exponential decay.
        let a = if attack > 0.0 {
            (1.0 - (-t / attack).exp()) * (-t / decay).exp()
        } else {
            (-t / decay).exp()
        };

        // Ringing modes.
        let mut osc = 0.0;
        for m in modes[..count].iter_mut() {
            // Instantaneous frequency, interpolated toward `hz*sweep` over the decay.
            let progress = 1.0 - (-t / m.tau).exp();
            let f = m.hz * (1.0 + (m.sweep - 1.0) * progress);
            m.phase_acc += std::f64::consts::TAU * f / sr;
            osc += m.gain * (-t / m.tau).exp() * (m.phase + m.phase_acc).sin();
        }

        // Strike: band-limited noise with its own decay, on top of the amp envelope.
        let nz = if filter.spec.is_some() {
            let tau = filter.spec.map(|s| s.tau as f64).unwrap_or(1.0);
            (-t / tau.max(1e-6)).exp() * filter.tick(rng.bipolar()) as f64
        } else {
            0.0
        };

        *out = (a * (osc + nz)) as f32;
    }

    dc_block(buf, 120.0, sr);

    if patch.normalize {
        let peak = buf.iter().fold(0.0f32, |a, &x| a.max(x.abs()));
        if peak > 0.0 {
            let g = 1.0 / peak;
            for x in buf.iter_mut() {
                *x *= g;
            }
        }
    }
}

/// Largest number of partials one voice renders. A patch with more is truncated, which keeps
/// the per-event coefficient block a fixed size and the render allocation-free.
pub const MAX_MODES: usize = 8;

/// Per-event state of one resonant mode.
#[derive(Clone, Copy, Debug)]
struct Mode {
    hz: f64,
    tau: f64,
    gain: f64,
    phase: f64,
    phase_acc: f64,
    sweep: f64,
}

impl Mode {
    const IDLE: Mode = Mode {
        hz: 0.0,
        tau: 1.0,
        gain: 0.0,
        phase: 0.0,
        phase_acc: 0.0,
        sweep: 1.0,
    };
}

/// Per-event state of the noise layer's state-variable filter.
#[derive(Clone, Copy, Debug)]
struct NoiseFilter {
    spec: Option<NoiseSpec>,
    s1: f32,
    s2: f32,
    low: f32,
    band: f32,
}

impl NoiseFilter {
    const IDLE: NoiseFilter = NoiseFilter {
        spec: None,
        s1: 0.0,
        s2: 0.0,
        low: 0.0,
        band: 0.0,
    };

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let spec = match &self.spec {
            Some(s) => s,
            None => return 0.0,
        };
        self.low += self.s1 * self.band;
        let high = x - self.low - self.s2 * self.band;
        self.band += self.s1 * high;
        let shaped = match spec.mode {
            FilterMode::Low => self.low,
            FilterMode::Band => self.band,
            FilterMode::High => high,
        };
        spec.gain * shaped
    }
}

/// State-variable filter coefficients: `(f, q)` such that
/// `low += f*band; high = x - low - q*band; band += f*high;` is stable.
fn svf_coeffs(hz: f64, q: f64, sr: f64) -> (f32, f32) {
    let f = 2.0 * (std::f64::consts::PI * hz / sr).sin();
    let damp = 1.0 / q.max(0.05);
    (f as f32, damp as f32)
}

/// One-pole DC blocker, in place.
pub fn dc_block(buf: &mut [f32], hz: f64, sr: f64) {
    let r = (-std::f64::consts::TAU * hz / sr).exp() as f32;
    let mut y1 = 0.0f32;
    let mut x1 = 0.0f32;
    for x in buf.iter_mut() {
        let v = *x;
        let y = v - x1 + r * y1;
        x1 = v;
        y1 = y;
        *x = y;
    }
}

/// Render a whole stream of events of one patch into a mono buffer.
///
/// This is the offline form of the same thing a game does at run time: schedule events, render
/// each into a voice buffer, sum them in. It allocates a buffer per event, which is correct
/// offline and wrong on an audio callback; a real-time host keeps a fixed pool of voices and
/// reuses them. See `docs` for that split.
pub fn render_stream(patch: &Patch, source: &Source, seconds: f64, sr: f64, seed: u64) -> Vec<f32> {
    let total = ((seconds * sr).ceil() as usize).max(1);
    let mut out = vec![0.0f32; total];
    let voice_len = patch_samples(patch, sr);
    let mut voice = vec![0.0f32; voice_len];
    let mut rng = Rng::new(seed);

    for t in source.times(seconds) {
        let start = (t * sr).round() as isize;
        render_event(patch, &mut rng, sr, &mut voice);
        for (k, &v) in voice.iter().enumerate() {
            let idx = start + k as isize;
            if idx < 0 || idx as usize >= total {
                continue;
            }
            out[idx as usize] += v;
        }
    }

    out
}
