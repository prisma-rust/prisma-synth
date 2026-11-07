//! Geiger-Muller counter synthesis.
//!
//! Two layers, independently testable:
//!
//! * [`poisson`] — the detector model. Draws incident events from an inhomogeneous
//!   Poisson process whose rate follows [`ramp`], then applies a non-paralyzable
//!   dead-time veto so that the count rate saturates at `1/tau` the way a real quench
//!   tube does.
//! * [`click`] + [`track`] — the instrument model. Turns each accepted event into a
//!   procedurally drawn speaker transient and mixes the whole track to a mono float
//!   buffer.
//!
//! Everything is deterministic from the `seed`: same seed, same samples, bit for bit,
//! on any platform.

pub mod click;
pub mod loud;
pub mod patch;
pub mod poisson;
pub mod props;
pub mod ramp;
pub mod rng;
pub mod stats;
pub mod track;

pub use click::{
    style_by_name, ClickStyle, ClickSynth, CLICK_PEAK, CLICK_SAMPLES, DEFAULT_STYLE, STYLES,
};
pub use patch::{
    patch_by_name, patch_samples, render_event, render_stream, Envelope, FilterMode, Jitter,
    NoiseSpec, Partial, Patch, Source, PATCHES,
};
pub use poisson::{DeadTime, PoissonSource};
pub use props::{dead_time_efficiency, homogeneous_point_stats, PointStats};
pub use ramp::Ramp;
pub use rng::Rng;
pub use stats::{Epoch, TrackStats};
pub use track::{density_gain, Track, CLICK_GAIN, DEMO_SECONDS, SAMPLE_RATE};
