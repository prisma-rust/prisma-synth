//! CSV export of the rate curves and a small analytic spectrogram.
//!
//! The spectrogram is a short-time Fourier transform with a Hann window on a log
//! magnitude scale, encoded straight to PNG with only `std`. The image is portrait so
//! the whole 60 s fits a phone screen: frequency runs left to right on a log axis
//! (20 Hz to 24 kHz), time runs top to bottom, and a strip on the left carries a tick
//! every 10 s::: the point is to see the click density climb and the top of the ramp
//! smear into a continuous band, not to make a publication figure.

use std::f64::consts::TAU;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use geiger_core::Track;

use crate::png::write_png;

/// Width of the axis strip, pixels.
pub const AXIS: usize = 34;
/// Image width, pixels.
pub const WIDTH: usize = 720;
/// Image height, pixels. One row per ~14 ms of audio.
pub const HEIGHT: usize = 1256;

const NFFT: usize = 1024;
/// Colour range below the image peak, dB.
pub const FLOOR_DB: f64 = 80.0;
/// Lift applied to the log magnitude before the palette.
pub const LIFT_DB: f64 = 44.0;

/// Columns: t_seconds,incident_cpm,displayed_cpm,clicks_in_window.
pub fn write_csv(path: &Path, track: &Track) -> io::Result<()> {
    let f = fs::File::create(path)?;
    let mut w = BufWriter::new(f);
    writeln!(w, "t_seconds,incident_cpm,displayed_cpm,clicks_in_window")?;
    let step = 0.25;
    let total = track.samples.len() as f64 / track.sr as f64;
    let mut t = 0.0;
    while t < total - 1e-9 {
        let t1 = (t + step).min(total);
        let clicks = track.accepted.iter().filter(|&&x| x >= t && x < t1).count();
        writeln!(
            w,
            "{t:.2},{:.2},{:.2},{}",
            track.incident_at(t),
            track.displayed_at(t),
            clicks
        )?;
        t += step;
    }
    w.flush()
}

/// Spectrogram to PNG.
pub fn write_spectrogram(path: &Path, track: &Track) -> io::Result<()> {
    let rows = spectrogram_rows(track);
    let mut bytes = Vec::with_capacity(WIDTH * HEIGHT * 3);
    for (y, row) in rows.iter().enumerate() {
        let axis = axis_pixel(track, y);
        for _ in 0..AXIS {
            bytes.extend_from_slice(&axis);
        }
        for &(r, g, b) in row {
            bytes.extend_from_slice(&[r, g, b]);
        }
    }
    write_png(path, WIDTH, HEIGHT, &bytes)
}

/// One axis pixel: a bright tick every 10 s of audio.
fn axis_pixel(track: &Track, row: usize) -> [u8; 3] {
    let duration = track.samples.len() as f64 / track.sr as f64;
    let step = duration / HEIGHT as f64;
    let t = (row as f64 + 0.5) * step;
    if ((t / 10.0) - (t / 10.0).round()).abs() < 0.5 * step / 10.0 {
        [150, 146, 130]
    } else {
        [26, 32, 34]
    }
}

/// Log-frequency spectrogram, one RGB triple per column.
fn spectrogram_rows(track: &Track) -> Vec<Vec<(u8, u8, u8)>> {
    let sr = track.sr as f64;
    let samples = &track.samples;
    let duration = (samples.len() as f64 / sr).max(1e-9);
    let plots_w = WIDTH - AXIS;

    let window: Vec<f64> = (0..NFFT)
        .map(|i| 0.5 - 0.5 * (TAU * i as f64 / NFFT as f64).cos())
        .collect();

    let fmin: f64 = 20.0;
    let fmax: f64 = 24_000.0;
    let ratio = fmax / fmin;

    let mut re = vec![0.0f64; NFFT];
    let mut im = vec![0.0f64; NFFT];
    let mut db = vec![vec![-140.0f64; plots_w]; HEIGHT];

    for (y, row) in db.iter_mut().enumerate() {
        let centre = (y as f64 + 0.5) / HEIGHT as f64 * duration;
        let start = ((centre * sr) as usize).saturating_sub(NFFT / 2);
        for k in 0..NFFT {
            re[k] = samples.get(start + k).copied().unwrap_or(0.0) as f64 * window[k];
            im[k] = 0.0;
        }
        fft(&mut re, &mut im);
        for (x, slot) in row.iter_mut().enumerate() {
            let f = fmin * ratio.powf((x as f64 + 0.5) / plots_w as f64);
            let bin = (((f / sr) * NFFT as f64).round() as usize).min(NFFT / 2 - 1);
            let mag = (re[bin] * re[bin] + im[bin] * im[bin]).sqrt() / NFFT as f64;
            *slot = 20.0 * mag.max(1e-9).log10();
        }
    }

    // One normalisation for the whole image, anchored at the loudest bin in the
    // file, so time and frequency are on the same scale.
    let peak = db.iter().flatten().fold(-140.0f64, |a, &b| a.max(b));
    let mut rows = Vec::with_capacity(HEIGHT);
    for column in db.iter() {
        let row: Vec<(u8, u8, u8)> = column
            .iter()
            .map(|&v| {
                // The lift is what makes the modes visible: the click body sits
                // 25 dB under the peak and would otherwise be painted black.
                let d = ((v - peak + FLOOR_DB + LIFT_DB) / FLOOR_DB).clamp(0.0, 1.0);
                palette(d)
            })
            .collect();
        rows.push(row);
    }
    rows
}

/// Inky floor at silence, cream at the top, so the plot reads as a figure.
fn palette(d: f64) -> (u8, u8, u8) {
    let x = d.powf(0.75);
    let pick = |a: f64, b: f64| (a + (b - a) * x).round().clamp(0.0, 255.0) as u8;
    (pick(6.0, 250.0), pick(12.0, 236.0), pick(14.0, 210.0))
}

/// In-place iterative radix-2 FFT; `re.len()` must be a power of two.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    assert!(n.is_power_of_two(), "FFT size must be a power of two");
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -TAU / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for i in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in 0..len / 2 {
                let ur = re[i + k];
                let ui = im[i + k];
                let vr0 = re[i + k + len / 2];
                let vi0 = im[i + k + len / 2];
                let vr = vr0 * cr - vi0 * ci;
                let vi = vr0 * ci + vi0 * cr;
                re[i + k] = ur + vr;
                im[i + k] = ui + vi;
                re[i + k + len / 2] = ur - vr;
                im[i + k + len / 2] = ui - vi;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}
