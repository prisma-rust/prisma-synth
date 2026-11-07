//! Diagnostic binary for the renderer, the loudness meter and the click shape.
//!
//! * `cargo run --release --bin diag` prints the loudness of the demo render.
//! * `--tone` checks the meter against a 1 kHz reference: a sine at -20 dBFS sample peak must
//!   read -23.00 LUFS.
//! * `--kweight` prints the K-weighting response.
//! * `--click [patch]` prints one event's envelope in milliseconds, which is the fastest way to
//!   see whether a patch is a click, a beep or a hiss.

use geiger_core::loud::{KWeight, Loudness};
use geiger_core::{patch_by_name, patch_samples, render_event, DeadTime, Patch, Ramp, Rng, Track};

const SEED: u64 = 0x2706_2026;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--tone") => tone(),
        Some("--kweight") => kweight(),
        Some("--click") => click(args.get(1).map(String::as_str).unwrap_or("geiger")),
        _ => render(),
    }
}

/// Envelope of one event, in 1 ms bins: peak and RMS per bin. A click shows a tall first bin
/// and rapid decay; a beep shows a flat first few bins; a hiss shows a broad, slowly falling
/// profile.
fn click(name: &str) {
    let patch: Patch = patch_by_name(name);
    let sr = 48_000.0;
    let n = patch_samples(&patch, sr);
    let mut buf = vec![0.0f32; n];
    let mut rng = Rng::new(SEED);
    render_event(&patch, &mut rng, sr, &mut buf);

    println!("patch `{}`: {}", patch.name, patch.blurb);
    println!(
        "  voice {:.1} ms, {} partials, noise {}, envelope attack {:.2} ms / decay {:.1} ms",
        patch.length * 1e3,
        patch.partials.len(),
        if patch.noise.is_some() { "on" } else { "off" },
        patch.env.attack * 1e3,
        patch.env.decay * 1e3
    );
    let db = |x: f64| if x > 0.0 { 20.0 * x.log10() } else { -120.0 };
    let bin = (sr / 1000.0) as usize; // 1 ms
    print!("  ms:");
    for (i, chunk) in buf.chunks(bin).enumerate().take(12) {
        let peak = chunk.iter().fold(0.0f64, |a, &x| a.max(x.abs() as f64));
        let rms =
            (chunk.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / chunk.len() as f64).sqrt();
        println!();
        print!(
            "  {i:>2}  peak {:>6.1} dB  rms {:>6.1} dB",
            db(peak),
            db(rms)
        );
    }
    println!();

    // Where does the energy sit? A coarse split says click (mostly < 2 ms) vs tone (spread).
    let total: f64 = buf.iter().map(|&x| (x as f64).powi(2)).sum();
    let early: f64 = buf[..(2 * bin).min(buf.len())]
        .iter()
        .map(|&x| (x as f64).powi(2))
        .sum();
    println!(
        "  energy in the first 2 ms: {:.1}% (a click concentrates it; a tone does not)",
        100.0 * early / total.max(1e-12)
    );
}

fn render() {
    let track = Track::synthesize(&Ramp::demo(), SEED, DeadTime::TYPICAL);
    let l = Loudness::measure(&track.samples, track.sr as f64);
    println!("{l:#?}");
}

/// A 1 kHz sine at `freq_khz` at -20 dBFS, 5 s: the meter must show -23.00 LUFS.
fn tone() {
    let sr = 48_000.0;
    let sine: Vec<f32> = (0..48_000 * 5)
        .map(|i| (0.1 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / sr).sin()) as f32)
        .collect();
    let l = Loudness::measure(&sine, sr);
    println!(
        "1 kHz sine at -20 dBFS sample peak: integrated {:.2} LUFS (want -23.00)",
        l.integrated
    );
}

fn kweight() {
    let sr = 48_000.0;
    for f in [
        20.0, 50.0, 100.0, 500.0, 1000.0, 1500.0, 2000.0, 3000.0, 5000.0, 10000.0,
    ] {
        let n = 48_000usize;
        let w = 2.0 * std::f64::consts::PI * f / sr;
        let mut k = KWeight::new(sr);
        let mut acc = 0.0f64;
        let mut acc_in = 0.0f64;
        for i in n / 4..n {
            let x = (w * i as f64).sin();
            acc += k.process(x).powi(2);
            acc_in += x * x;
        }
        println!("{f:7.0} Hz  {:+.3} dB", 10.0 * (acc / acc_in).log10());
    }
}
