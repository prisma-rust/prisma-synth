//! `geiger --calibrate <dbfs> ...` renders the ramp at fixed absolute peak levels and
//! reports what `ffmpeg` measures for each, so the publish master can be picked and then
//! verified instead of re-rendered until it happens to look right.

use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

use geiger_core::{DeadTime, Ramp, Track};

use crate::crest::crest_db;
use crate::{write_wav, SEED};

pub fn run(args: &[String]) -> io::Result<()> {
    let ramp = Ramp::demo();
    let track = Track::synthesize(&ramp, SEED, DeadTime::TYPICAL);
    let dir = Path::new("out/calib");
    fs::create_dir_all(dir)?;

    println!("target  integrated  LRA  crest  file");
    for arg in args {
        let target: f64 = arg.parse().expect("dBFS value");
        let target = target.clamp(-60.0, 0.0);
        let lin = (10f64.powf(target / 20.0) / track.pre_norm_peak as f64) as f32;
        let path = dir.join(format!("calib_{target:+.1}.wav"));
        let mut raw = track.samples.clone();
        for s in raw.iter_mut() {
            *s *= lin;
        }
        write_wav(&path, &raw, track.sr)?;
        match ebur128(&path) {
            Ok(eb) => println!(
                "{:>+7.1}  {:>7} LUFS  {:>5} LU  {:>5.1} dB  {}",
                target,
                eb.integrated,
                eb.lra,
                crest_db(&raw),
                path.display()
            ),
            Err(_) => println!("{:>+7.1}  (ffmpeg unavailable)", target),
        }
    }
    Ok(())
}

pub struct Ebur {
    pub integrated: String,
    pub lra: String,
}

pub fn ebur128(path: &Path) -> io::Result<Ebur> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(path)
        .args(["-af", "ebur128=framelog=quiet", "-f", "null", "-"])
        .output()?;
    let text = String::from_utf8_lossy(&out.stderr);
    let mut integrated = "?".to_string();
    let mut lra = "?".to_string();
    for line in text.lines() {
        if integrated == "?" {
            if let Some(rest) = line.split(" I: ").nth(1) {
                integrated = rest.split_whitespace().next().unwrap_or("?").to_string();
            }
        }
        if lra == "?" {
            if let Some(rest) = line.split("LRA:").nth(1) {
                lra = rest.split_whitespace().next().unwrap_or("?").to_string();
            }
        }
    }
    Ok(Ebur { integrated, lra })
}
