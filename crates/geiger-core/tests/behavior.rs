//! Checks on the detector model and the instrument model.
//!
//! These assert behaviour a listener can hear: a constant rate produces memoryless
//! intervals, dead time saturates, the same seed reproduces the same samples, and the
//! mix never leaves full scale.

use geiger_core::{
    dead_time_efficiency, density_gain, homogeneous_point_stats, DeadTime, Ramp, Track,
};

#[test]
fn poisson_intervals_are_exponential() {
    // 5 s at 200/s: the CV of an exponential is 1. A fixed-pitch train is ~0, and
    // "random clicks" with a clamped distribution drift off 1.
    let st = homogeneous_point_stats(1, 200.0, 40_000);
    assert!((st.cv - 1.0).abs() < 0.03, "cv = {}", st.cv);
    assert!(
        (st.mean_interval - 0.005).abs() < 1e-4,
        "mean = {}",
        st.mean_interval
    );
    assert!(
        (st.window - 200.0).abs() / 200.0 < 0.05,
        "window = {}",
        st.window
    );
}

#[test]
fn dead_time_saturates_at_one_over_tau() {
    // 20 kcps incident through a 150 us tube: 1/(1 + 3) of the events get through.
    let measured = dead_time_efficiency(2, 20_000.0, 150e-6, 5.0);
    let predicted = DeadTime::TYPICAL.efficiency(20_000.0);
    assert!(
        (measured - predicted).abs() < 0.02,
        "measured {measured}, predicted {predicted}"
    );
    assert!(measured < 0.30, "measured {measured}");
}

#[test]
fn dead_time_never_exceeds_the_ceiling() {
    let dead = DeadTime::TYPICAL;
    let mut prev = 0.0;
    for r in [1.0, 10.0, 1e2, 1e3, 1e4, 1e5, 1e6, 1e9] {
        let obs = dead.observed_rate(r);
        assert!(obs > prev, "observed rate must grow with the incident rate");
        assert!(obs < dead.ceiling(), "observed {obs} >= ceiling");
        prev = obs;
    }
}

#[test]
fn true_rate_inverts_observed_rate() {
    let dead = DeadTime::new(20e-6);
    for r in [1.0, 500.0, 12_345.0, 45_000.0] {
        let back = dead
            .true_rate(dead.observed_rate(r))
            .expect("below ceiling");
        assert!((back - r).abs() / r < 1e-9, "{back} vs {r}");
    }
    assert!(dead.true_rate(dead.ceiling()).is_none());
    assert!(dead.true_rate(dead.ceiling() * 2.0).is_none());
}

#[test]
fn ramp_is_continuous_and_log_linear() {
    let ramp = Ramp::demo();
    // Halfway between two anchors in time is the geometric mean in rate.
    let a = ramp.incident_cpm(10.0);
    let b = ramp.incident_cpm(56.0);
    let mid = ramp.incident_cpm(33.0);
    assert!((mid - (a * b).sqrt()).abs() / mid < 1e-9);

    let mut prev = ramp.incident_cpm(0.0);
    let mut t = 1e-6;
    while t < ramp.end_time() {
        let cur = ramp.incident_cpm(t);
        assert!(cur >= prev * 0.999, "rate dipped at {t}: {prev} -> {cur}");
        prev = cur;
        t += 1e-4;
    }
}

#[test]
fn density_tilt_never_boosts_and_stays_smooth() {
    let mut prev = density_gain(1.0);
    for i in 1..200_000 {
        let l = i as f64 / 200_000.0 * 6.0;
        let g = density_gain(10f64.powf(l));
        assert!(g.is_finite() && g > 0.0);
        assert!(g <= 1.000_1, "tilt boosts above 0 dB at {l}: {g}");
        assert!(
            (g / prev - 1.0).abs() < 0.01,
            "tilt jumped at {l}: {prev} -> {g}"
        );
        prev = g;
    }
}

#[test]
fn same_seed_renders_identical_samples() {
    let ramp = Ramp::ramp_only(20.0, 200_000.0, 60.0);
    let a = Track::synthesize(&ramp, 7, DeadTime::TYPICAL);
    let b = Track::synthesize(&ramp, 7, DeadTime::TYPICAL);
    assert_eq!(a.samples.len(), b.samples.len());
    for (i, (x, y)) in a.samples.iter().zip(b.samples.iter()).enumerate() {
        assert_eq!(x.to_bits(), y.to_bits(), "sample {i}");
    }
    assert_eq!(a.accepted.len(), b.accepted.len());
}

#[test]
fn rendered_samples_stay_inside_full_scale() {
    let ramp = Ramp::demo();
    let t = Track::synthesize(&ramp, 5, DeadTime::TYPICAL);
    for (i, &s) in t.samples.iter().enumerate() {
        assert!(s.abs() < 0.96, "sample {i} = {s} exceeds the limiter knee");
    }
    assert!(t.accepted.len() > 10_000);
    assert_eq!(t.truncated, 0);
}

#[test]
fn background_section_is_sparse_not_clicked_through() {
    let ramp = Ramp::demo();
    let t = Track::synthesize(&ramp, 3, DeadTime::TYPICAL);
    let first = t.epochs(2.0).remove(0);
    assert!(
        first.accepted <= 2,
        "{} clicks in the first 2 s",
        first.accepted
    );
}

#[test]
fn dead_time_rejects_a_real_fraction_of_the_stream() {
    let ramp = Ramp::demo();
    let t = Track::synthesize(&ramp, 5, DeadTime::TYPICAL);
    let s = t.stats();
    assert!(s.rejected_events() > 10_000, "{:?}", s);
    assert!(s.reporting_ratio() < 0.6);
}

// --- the general patch engine -----------------------------------------------------------------

use geiger_core::{patch_samples, render_stream, Patch, Source, PATCHES};

#[test]
fn every_patch_renders_one_event_at_full_scale() {
    // A single event is peak-normalised to 1.0 by the engine, so every patch must produce
    // exactly that. A stream is a different question: overlapping events sum, and that is the
    // mixer's job to stage, not the voice's.
    for p in PATCHES {
        let n = patch_samples(p, 48_000.0);
        let mut buf = vec![0.0f32; n];
        let mut rng = geiger_core::Rng::new(1);
        geiger_core::render_event(p, &mut rng, 48_000.0, &mut buf);
        let peak = buf.iter().fold(0.0f32, |a, &x| a.max(x.abs()));
        assert!(
            (peak - 1.0).abs() < 1e-3,
            "{} peaked at {peak}, not 1.0",
            p.name
        );
        let energy: f32 = buf.iter().map(|&x| x * x).sum();
        assert!(energy > 1e-4, "{} rendered silence", p.name);
    }
}

#[test]
fn every_patch_renders_a_non_silent_stream_of_the_asked_length() {
    for p in PATCHES {
        let src = Source::Regular { interval: 0.05 };
        let s = render_stream(p, &src, 2.0, 48_000.0, 1);
        assert_eq!(s.len(), 96_000, "{}", p.name);
        let energy: f32 = s.iter().map(|&x| x * x).sum();
        assert!(energy > 0.01, "{} rendered silence", p.name);
    }
}

#[test]
fn patch_rendering_is_deterministic() {
    for p in PATCHES {
        let src = Source::Poisson {
            seed: 42,
            rate: 7.0,
        };
        let a = render_stream(p, &src, 1.0, 48_000.0, 99);
        let b = render_stream(p, &src, 1.0, 48_000.0, 99);
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            assert_eq!(x.to_bits(), y.to_bits(), "{} differs at {i}", p.name);
        }
    }
}

#[test]
fn voice_buffer_covers_the_patch_length() {
    for p in PATCHES {
        let want = (p.length as f64 * 48_000.0).ceil() as usize;
        assert_eq!(patch_samples(p, 48_000.0), want, "{}", p.name);
        assert!(patch_samples(p, 48_000.0) > 0);
    }
}

#[test]
fn poisson_source_rate_matches_the_request() {
    let src = Source::Poisson {
        seed: 7,
        rate: 50.0,
    };
    let t = src.times(10.0);
    let measured = t.len() as f64 / 10.0;
    assert!((measured - 50.0).abs() < 3.0, "measured {measured}/s");
}

#[test]
fn regular_source_is_on_the_beat() {
    let src = Source::Regular { interval: 0.25 };
    let t = src.times(1.0);
    assert_eq!(t.len(), 4);
    for (i, x) in t.iter().enumerate() {
        assert!((x - i as f64 * 0.25).abs() < 1e-12);
    }
}

#[test]
fn explicit_times_are_filtered_to_the_window() {
    let src = Source::Times(vec![-1.0, 0.0, 0.5, 1.0, 2.0]);
    assert_eq!(src.times(1.0), vec![0.0, 0.5]);
}

#[test]
fn a_patch_with_no_partials_still_renders() {
    // `rumble` is noise-only; the engine must not assume at least one mode.
    let rumble = PATCHES.iter().find(|p| p.name == "rumble").unwrap();
    assert!(rumble.partials.is_empty());
    let s = render_stream(rumble, &Source::Regular { interval: 0.5 }, 2.0, 48_000.0, 3);
    let peak = s.iter().fold(0.0f32, |a, &x| a.max(x.abs()));
    assert!(peak > 0.01, "noise-only patch rendered silence");
}

#[test]
fn geiger_and_its_patch_agree() {
    // The click adapter and the general engine are two spellings of one model. Their output
    // will not be bit-identical (the adapter keeps a per-style band-pass, the engine a
    // state-variable one), but they must agree on the thing a listener hears: an event that
    // decays to nothing in a few milliseconds, with no DC and bounded amplitude.
    let g = PATCHES
        .iter()
        .find(|p: &&Patch| p.name == "geiger")
        .unwrap();
    let s = render_stream(g, &Source::Regular { interval: 0.5 }, 2.0, 48_000.0, 4);
    let peak = s.iter().fold(0.0f32, |a, &x| a.max(x.abs()));
    assert!(peak > 0.5 && peak <= 1.0, "peak {peak}");
    // Energy in the 4 ms after an event must dwarf the energy in the 4 ms before the next.
    let e_early: f32 = s[..192].iter().map(|&x| x * x).sum();
    let e_late: f32 = s[192..384].iter().map(|&x| x * x).sum();
    assert!(
        e_early > e_late * 4.0,
        "event did not decay: {e_early} vs {e_late}"
    );
    let dc: f32 = s.iter().sum::<f32>() / s.len() as f32;
    assert!(dc.abs() < 1e-3, "DC offset {dc}");
}
