/// Crest factor of a clip in dB: linear peak over linear RMS, 20 log10.
pub fn crest_db(samples: &[f32]) -> f64 {
    let peak = samples.iter().fold(0.0f64, |a, &x| a.max((x as f64).abs()));
    if peak <= 0.0 || samples.is_empty() {
        return -120.0;
    }
    let rms =
        (samples.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / samples.len() as f64).sqrt();
    if rms <= 0.0 {
        return 120.0;
    }
    20.0 * (peak / rms).log10()
}
