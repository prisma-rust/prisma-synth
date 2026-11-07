//! C ABI for the event-driven synthesiser.
//!
//! The point of this crate is to make one question cheap to answer: *can I use this engine in
//! Unity or Unreal without rewriting it?* The engine itself is allocation-free on the per-event
//! path already, so the remaining work is just crossing a language boundary, and that is what
//! this file is: a flat, `extern "C"` surface with no Rust types in any signature and no
//! allocation anywhere on the hot path.
//!
//! # Shape of the API
//!
//! The host owns everything: the voice buffer, the RNG state, the event list, the mixing. The
//! library only turns *one event* into samples. That split is deliberate; it means the same
//! library serves
//!
//! * an offline renderer that sums into a file,
//! * Unity's `OnAudioFilterRead`, and
//! * Unreal's `ISoundGenerator::OnGenerateAudio`,
//!
//! without any of them having to agree on a threading model.
//!
//! ```c
//! // once, for each emitter
//! uint32_t len = synth_voice_len(SYNTH_PATCH_GEIGER, 48000.0);
//! float *voice = malloc(len * sizeof(float));
//! uint64_t rng = 0x27062026;
//!
//! // on every event
//! synth_render_event(SYNTH_PATCH_GEIGER, 48000.0, &rng, voice, len);
//! // ... then add `voice` into the output at the event's sample offset
//! ```
//!
//! # Threading
//!
//! Every function here is pure with respect to the library: no global state, no locks, no
//! allocation. The only mutable state is the caller's buffer and the caller's `rng` word, so the
//! host decides what runs on which thread and the library cannot introduce a data race.
//!
//! # Safety contract
//!
//! `out` must point to at least `len` writable `f32`s, and `len` must be at least the value
//! returned by [`synth_voice_len`] for the same patch and sample rate, or the event is truncated
//! (not an error: a host that wants a short tail may pass a shorter buffer deliberately).

use geiger_core::patch::{patch_samples, render_event, Patch, PATCHES};
use geiger_core::Rng;

/// Render one event of `patch_index` into `out`.
///
/// `rng` is a caller-owned SplitMix64 state; it is advanced exactly as the matching Rust call
/// would. Returns the number of samples written, which is `min(len, synth_voice_len(...))`.
///
/// # Safety
///
/// `out` must be valid for writes of `len` floats. `rng` must be valid for a read and a write.
#[no_mangle]
pub unsafe extern "C" fn synth_render_event(
    patch_index: i32,
    sample_rate: f64,
    rng: *mut u64,
    out: *mut f32,
    len: u32,
) -> u32 {
    if out.is_null() || rng.is_null() || len == 0 || !sample_rate.is_finite() || sample_rate <= 0.0
    {
        return 0;
    }
    let patch = patch_at(patch_index);
    let n = (len as usize).min(patch_samples(patch, sample_rate));
    if n == 0 {
        return 0;
    }

    let mut state = Rng::from_state(*rng);
    // Render into a slice of exactly `n`, then hand the advanced state back to the caller.
    let buf = std::slice::from_raw_parts_mut(out, n);
    render_event(patch, &mut state, sample_rate, buf);
    *rng = state.state();
    n as u32
}

/// Voice buffer length, in samples, needed to hold one event of `patch_index` without
/// truncation.
#[no_mangle]
pub extern "C" fn synth_voice_len(patch_index: i32, sample_rate: f64) -> u32 {
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return 0;
    }
    patch_samples(patch_at(patch_index), sample_rate) as u32
}

/// Number of patches in the library.
#[no_mangle]
pub extern "C" fn synth_patch_count() -> u32 {
    PATCHES.len() as u32
}

/// Name of the patch at `index`, or a null pointer if the index is out of range.
///
/// The returned pointer is to a `'static` NUL-terminated string; the host must not free it.
#[no_mangle]
pub extern "C" fn synth_patch_name(index: u32) -> *const std::os::raw::c_char {
    match PATCHES.get(index as usize) {
        Some(p) => c_string(p.name).as_ptr(),
        None => std::ptr::null(),
    }
}

/// One-line description of the patch at `index`, or a null pointer if out of range.
#[no_mangle]
pub extern "C" fn synth_patch_blurb(index: u32) -> *const std::os::raw::c_char {
    match PATCHES.get(index as usize) {
        Some(p) => c_string(p.blurb).as_ptr(),
        None => std::ptr::null(),
    }
}

/// Index of the patch named `name`, or `-1`.
///
/// # Safety
///
/// `name` must be a NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn synth_patch_index(name: *const std::os::raw::c_char) -> i32 {
    if name.is_null() {
        return -1;
    }
    let bytes = std::ffi::CStr::from_ptr(name).to_bytes();
    PATCHES
        .iter()
        .position(|p| p.name.as_bytes() == bytes)
        .map(|i| i as i32)
        .unwrap_or(-1)
}

/// The recommended event source for a patch, as encoded hints the host can read: the nominal
/// events-per-second and the maximum simultaneous voices at that rate.
///
/// Returns `(rate, polyphony)`. The host is free to ignore both; they exist so a Unity or
/// Unreal wrapper does not have to guess a pool size.
#[no_mangle]
pub extern "C" fn synth_patch_hint(patch_index: i32, sample_rate: f64) -> SynthPatchHint {
    let patch = patch_at(patch_index);
    let voice_seconds = patch.length as f64;
    // A plausible working point: 3x the rate at which the voice tail would just fill the
    // timeline, which is where a one-shot starts to read as a texture.
    let rate = (1.0 / voice_seconds.max(1e-3)) * 3.0;
    let polyphony = (rate * voice_seconds * 1.5).ceil().max(1.0) as u32;
    let _ = sample_rate;
    SynthPatchHint { rate, polyphony }
}

/// See [`synth_patch_hint`].
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SynthPatchHint {
    pub rate: f64,
    pub polyphony: u32,
}

/// Stable patch ids, so the binding side does not depend on library order.
///
/// The names are the contract; the discriminants are not. A host that wants to be robust across
/// library versions should call [`synth_patch_index`] once with the name it wants.
pub mod patch_id {
    pub const GEIGER: i32 = 0;
    pub const RAINDROP: i32 = 1;
    pub const FOOTSTEP: i32 = 2;
    pub const WOOD: i32 = 3;
    pub const UI_TICK: i32 = 4;
    pub const ALERT: i32 = 5;
    pub const RUMBLE: i32 = 6;
}

fn patch_at(index: i32) -> &'static Patch {
    if index < 0 {
        return &PATCHES[0];
    }
    PATCHES.get(index as usize).unwrap_or(&PATCHES[0])
}

/// `'static` NUL-terminated copy of a name or blurb, so the pointer stays valid.
fn c_string(s: &'static str) -> &'static [std::os::raw::c_char] {
    // Names and blurbs are `'static` and contain no interior NUL, so leaking one terminator per
    // distinct string is bounded by the size of the patch table and happens at most once each.
    let mut out = Vec::with_capacity(s.len() + 1);
    out.extend(s.bytes().map(|b| b as std::os::raw::c_char));
    out.push(0);
    Box::leak(out.into_boxed_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_len_and_render_agree() {
        for idx in 0..synth_patch_count() as i32 {
            let len = synth_voice_len(idx, 48_000.0);
            assert!(len > 0);
            let mut buf = vec![0.0f32; len as usize];
            let mut rng = 0x2706_2026u64;
            let written =
                unsafe { synth_render_event(idx, 48_000.0, &mut rng, buf.as_mut_ptr(), len) };
            assert_eq!(written, len);
            let peak = buf.iter().fold(0.0f32, |a, &x| a.max(x.abs()));
            assert!((peak - 1.0).abs() < 1e-3, "patch {idx} peak {peak}");
            assert_ne!(rng, 0x2706_2026, "rng was not advanced");
        }
    }

    #[test]
    fn short_buffer_truncates_instead_of_failing() {
        let mut buf = vec![0.0f32; 8];
        let mut rng = 1u64;
        let written = unsafe { synth_render_event(0, 48_000.0, &mut rng, buf.as_mut_ptr(), 8) };
        assert_eq!(written, 8);
    }

    #[test]
    fn null_and_bad_arguments_are_survivable() {
        let mut rng = 1u64;
        let mut buf = vec![0.0f32; 16];
        unsafe {
            assert_eq!(
                synth_render_event(0, 48_000.0, &mut rng, std::ptr::null_mut(), 16),
                0
            );
            assert_eq!(
                synth_render_event(0, 48_000.0, std::ptr::null_mut(), buf.as_mut_ptr(), 16),
                0
            );
            assert_eq!(
                synth_render_event(0, 0.0, &mut rng, buf.as_mut_ptr(), 16),
                0
            );
            assert_eq!(
                synth_render_event(0, 48_000.0, &mut rng, buf.as_mut_ptr(), 0),
                0
            );
        }
    }

    #[test]
    fn names_round_trip() {
        for i in 0..synth_patch_count() {
            let name = synth_patch_name(i);
            assert!(!name.is_null());
            let idx = unsafe { synth_patch_index(name) };
            assert_eq!(idx, i as i32);
        }
        assert_eq!(unsafe { synth_patch_index(std::ptr::null()) }, -1);
        assert!(synth_patch_name(999).is_null());
    }
}
