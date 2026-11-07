//! The stats block printed to stdout and written to `out/stats.txt`.
//!
//! Every number is measured off the rendered track: the click counts come from the
//! onset list the mixer produced, the intervals from the Poisson source, and the
//! dead-time pass rate from a five-second rerun of the veto. If any of them disagrees
//! with what the audio sounds like, that is a bug in the renderer, not in the prose.

use geiger_core::{dead_time_efficiency, homogeneous_point_stats, DeadTime, Track};

use crate::crest::crest_db;
use crate::{db, SEED};

fn format_block(track: &Track) -> String {
    let mut o = String::new();
    o.push('\n');
    o.push_str(" epoch   inc_cpm   clk_cpm   clicks    cps   eff   rms_dB   pk_dB\n");
    o.push_str("----------------------------------------------------------------\n");
    for e in track.epochs(2.0) {
        o.push_str(&format!(
            "{:>5.0} {:>10.0} {:>9.0} {:>8} {:>6.1} {:>5.2} {:>8.1} {:>7.1}\n",
            e.start,
            e.incident_cpm,
            e.clicks_per_min,
            e.accepted,
            e.accepted_cps,
            e.efficiency,
            db(e.rms),
            db(e.peak)
        ));
    }
    o
}

pub fn report(track: &Track) -> String {
    let s = track.stats();
    let mut o = String::new();
    o.push_str("geiger-synth: 60 s counter ramp\n");
    o.push_str("================================\n");
    o.push_str(&format!(
        "format        {:.2} s rendered, mono {} Hz, 24-bit PCM\n",
        s.seconds, s.sr
    ));
    o.push_str(&format!("seed          0x{SEED:016X}\n"));
    o.push_str(&format!(
        "dead time     {:.0} us -> ceiling {:.0} cpm ({:.2} c/s)\n",
        s.tau_us,
        s.ceiling_cpm,
        s.ceiling_cpm / 60.0
    ));
    o.push_str(&format!(
        "span          {:.1} decades of incident rate (background .. peak)\n",
        track.decades()
    ));
    o.push_str(&format!(
        "peak          {:.0} cpm = {:.2} c/s incident\n",
        track.max_incident_cpm(),
        track.max_incident_cpm() / 60.0
    ));
    o.push_str(&format!(
        "pin           display pinned at the ceiling for {:.2} s\n",
        track.pinned_seconds()
    ));
    let stat = homogeneous_point_stats(SEED, 200.0, 40_000);
    o.push_str(&format!(
        "intervals     incident CV {:.3} (exponential = 1) at 200 c/s\n",
        stat.cv
    ));
    let measured_eff = dead_time_efficiency(SEED ^ 0x5555_5555, 20_000.0, 150e-6, 5.0);
    o.push_str(&format!(
        "veto check    {:.1}% pass at 20 kcps (model {:.1}%)\n",
        100.0 * measured_eff,
        100.0 * DeadTime::TYPICAL.efficiency(20_000.0)
    ));
    o.push_str(&format!(
        "tube          passed {}/{} incident events ({:.2}%)\n",
        s.accepted_events,
        s.incident_events,
        100.0 * s.reporting_ratio()
    ));
    o.push_str(&format!(
        "tail          {} clicks cut by the end of the buffer\n",
        s.truncated
    ));
    o.push_str(&format!(
        "peak mix      {:.3} pre-normalise ({:.1} dBFS), crest {:.1} dB",
        track.pre_norm_peak,
        db(track.pre_norm_peak as f64),
        crest_db(&track.samples)
    ));

    // Loudness of the clipped product, over the sections that matter.
    let mean_all = mean_db(track, 0.0, s.seconds);
    let head = mean_db(track, 0.0, 10.0);
    let climb_lo = mean_db(track, 10.0, 34.0);
    let climb_hi = mean_db(track, 34.0, 56.0);
    let top = mean_db(track, 56.0, 60.0);
    o.push_str(&format!(
        "\nlevel (mean of 20 ms RMS)\n  all {mean_all:.1} dBFS | {head:.1} head | \
         {climb_lo:.1} low climb | {climb_hi:.1} high climb | {top:.1} top\n",
        mean_all = mean_all,
        head = head,
        climb_lo = climb_lo,
        climb_hi = climb_hi,
        top = top
    ));
    o.push_str(&format_block(track));
    o
}

/// Mean of the 20 ms window RMS, dBFS, over `[a, b)`.
fn mean_db(track: &Track, a: f64, b: f64) -> f64 {
    let sr = track.sr as f64;
    let win = (0.020 * sr) as usize;
    let s = &track.samples;
    let i0 = (a * sr) as usize;
    let i1 = ((b * sr) as usize).min(s.len());
    let mut sum = 0.0;
    let mut n = 0usize;
    let mut acc = 0.0;
    for (i, &x) in s[i0..i1].iter().enumerate() {
        acc += (x as f64) * (x as f64);
        if (i + 1) % win == 0 {
            let rms = (acc / win as f64).sqrt();
            sum += if rms > 0.0 {
                20.0 * rms.log10()
            } else {
                -120.0
            };
            n += 1;
            acc = 0.0;
        }
    }
    if n == 0 {
        -120.0
    } else {
        sum / n as f64
    }
}
