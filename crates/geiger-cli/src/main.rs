//! Offline renderer for the Geiger-Muller counter demo.
//!
//! Produces, in `out/`:
//!
//! * `geiger-60s.wav` - 48 kHz, 24-bit mono master
//! * `geiger-60s.mp3` - 320 kbps listen copy, when `ffmpeg` is on PATH
//! * `analysis.csv` - per 0.25 s: incident CPM, displayed CPM, clicks
//! * `spectrogram.png` - analytic spectrogram, encoded here (no image crates)
//! * `stats.txt` - the summary also printed to stdout
//!
//! `geiger --calibrate <dBFS> ...` renders at fixed peak levels for level matching.
//! `geiger --style <name>` picks a click style; `geiger --styles` renders a short
//! comparison clip of every style, so the choice can be made by listening rather than by
//! guessing.

mod calib;
mod crest;
mod export;
mod png;
mod report;

use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use geiger_core::{
    patch_by_name, patch_samples, render_event, render_stream, style_by_name, ClickStyle, DeadTime,
    Patch, Ramp, Source, Track, DEFAULT_STYLE, PATCHES, STYLES,
};

/// Fixed seed so the track is reproducible; change it to get another take on the same
/// schedule.
const SEED: u64 = 0x2706_2026;

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = || args.get(1).map(String::as_str).unwrap_or("");
    match args.first().map(String::as_str) {
        Some("--calibrate") => calib::run(&args[1..]),
        Some("--styles") => showcase(),
        Some("--patches") => patch_showcase(),
        Some("--patch") => patch_demo(patch_by_name(name())),
        Some("--style") => {
            eprintln!("using click style `{}`", style_by_name(name()).name);
            render(style_by_name(name()))
        }
        _ => render(DEFAULT_STYLE),
    }
}

/// Render each style as a short comparison clip: the same rate schedule every time, so the
/// only thing that changes between files is the click itself.
fn showcase() -> io::Result<()> {
    let out = Path::new("out/styles");
    fs::create_dir_all(out)?;
    // Three rates, so a style is heard both as isolated events and as a dense train.
    let seg = 7.0;
    println!("style    constant rates 30 / 300 / 3000 cpm, {seg}s each");
    for st in STYLES {
        let mut samples: Vec<f32> = Vec::new();
        for (i, cpm) in [30.0f64, 300.0, 3000.0].iter().enumerate() {
            let ramp = Ramp::new(vec![(0.0, *cpm), (seg, *cpm)]);
            let t =
                Track::synthesize_style_with(&ramp, SEED, DeadTime::TYPICAL, *st, 48_000.0, seg);
            samples.extend_from_slice(&t.samples);
            if i + 1 < 3 {
                // 300 ms of silence between sections.
                samples.extend(std::iter::repeat_n(0.0, 14_400));
            }
        }
        let path = out.join(format!("style_{}.wav", st.name));
        crate::write_wav(&path, &samples, 48_000)?;
        println!("  {:<7} {}", st.name, st.blurb);
    }
    Ok(())
}

/// A demo rate that suits one-shot percussion: dense enough to hear the train, sparse enough
/// to hear one event. Long patches (alerts, rumbles) get a gentler rate so they do not weld.
fn demo_rate(patch: &Patch) -> f64 {
    if patch.length > 0.8 {
        0.8
    } else if patch.length > 0.2 {
        3.0
    } else {
        9.0
    }
}

/// Render every patch in the library with the same engine, so the engine's generality can be
/// heard rather than asserted.
fn patch_showcase() -> io::Result<()> {
    let out = Path::new("out/patches");
    fs::create_dir_all(out)?;
    println!("patch      rate   one-line description");
    for p in PATCHES {
        let rate = demo_rate(p);
        let seconds = 6.0;
        let src = Source::Poisson {
            seed: SEED ^ 0x9E37_79B9,
            rate,
        };
        let samples = render_stream(p, &src, seconds, 48_000.0, SEED);
        let path = out.join(format!("patch_{}.wav", p.name));
        crate::write_wav(&path, &samples, 48_000)?;
        println!("  {:<9} {:>4.1}/s {}", p.name, rate, p.blurb);
    }
    Ok(())
}

/// Render one patch, and print what it is made of.
fn patch_demo(patch: Patch) -> io::Result<()> {
    let out = Path::new("out/patches");
    fs::create_dir_all(out)?;
    let rate = demo_rate(&patch);
    println!("patch `{}`: {}", patch.name, patch.blurb);
    println!(
        "  {} partial(s), noise {}, envelope attack {:.2} ms / decay {:.1} ms, voice {:.1} ms",
        patch.partials.len(),
        if patch.noise.is_some() { "on" } else { "off" },
        patch.env.attack * 1e3,
        patch.env.decay * 1e3,
        patch.length * 1e3
    );
    for p in patch.partials {
        println!(
            "    {:.0} Hz, {:.1} ms decay, gain {:.2}",
            p.hz,
            p.tau * 1e3,
            p.gain
        );
    }
    let seconds = 8.0;
    let src = Source::Poisson {
        seed: SEED ^ 0x9E37_79B9,
        rate,
    };
    let samples = render_stream(&patch, &src, seconds, 48_000.0, SEED);
    let path = out.join(format!("patch_{}.wav", patch.name));
    crate::write_wav(&path, &samples, 48_000)?;
    println!("wrote {} at {rate:.1} events/s", path.display());
    eprintln!("voice buffer {} samples", patch_samples(&patch, 48_000.0));
    let _ = render_event;
    Ok(())
}

fn render(style: ClickStyle) -> io::Result<()> {
    let out = Path::new("out");
    fs::create_dir_all(out)?;

    let ramp = Ramp::demo();
    let dead = DeadTime::TYPICAL;

    let t0 = Instant::now();
    let track = Track::synthesize_style(&ramp, SEED, dead, style);
    eprintln!("rendered in {:.2?}", t0.elapsed());

    let wav = out.join("geiger-60s.wav");
    write_wav(&wav, &track.samples, track.sr)?;
    eprintln!(
        "wrote {} ({:.2} s, {} samples, {} clicks)",
        wav.display(),
        track.samples.len() as f64 / track.sr as f64,
        track.samples.len(),
        track.accepted.len()
    );

    if command_exists("ffmpeg") {
        let mp3 = out.join("geiger-60s.mp3");
        let status = Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-i"])
            .arg(&wav)
            .args(["-codec:a", "libmp3lame", "-b:a", "320k"])
            .arg(&mp3)
            .status();
        match status {
            Ok(s) if s.success() => eprintln!("wrote {}", mp3.display()),
            Ok(s) => eprintln!("ffmpeg exited with {s}"),
            Err(e) => eprintln!("could not run ffmpeg: {e}"),
        }
    }

    export::write_csv(&out.join("analysis.csv"), &track)?;
    export::write_spectrogram(&out.join("spectrogram.png"), &track)?;
    let block = report::report(&track);
    fs::write(out.join("stats.txt"), &block)?;
    println!("{block}");

    Ok(())
}

fn write_wav(path: &Path, samples: &[f32], sr: u32) -> io::Result<()> {
    let f = fs::File::create(path)?;
    let mut w = BufWriter::new(f);
    let bits: u16 = 24;
    let bytes_per_sample = (bits / 8) as u32;
    let data_len = samples.len() as u32 * bytes_per_sample;

    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data_len).to_le_bytes())?;
    w.write_all(b"WAVE")?;
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?; // PCM
    w.write_all(&1u16.to_le_bytes())?; // mono
    w.write_all(&sr.to_le_bytes())?;
    w.write_all(&(sr * bytes_per_sample).to_le_bytes())?;
    w.write_all(&(bytes_per_sample as u16).to_le_bytes())?;
    w.write_all(&bits.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_len.to_le_bytes())?;

    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 8_388_607.0) as i32;
        let b = v.to_le_bytes();
        w.write_all(&b[..3])?;
    }
    w.flush()
}

fn command_exists(prog: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {prog} >/dev/null 2>&1"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Linear amplitude to dB, floored at -120 so the floor prints.
fn db(x: f64) -> f64 {
    if x > 0.0 {
        20.0 * x.log10()
    } else {
        -120.0
    }
}
