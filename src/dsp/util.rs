pub const SAMPLE_RATE: f32 = 48_000.0;

#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[inline]
pub fn lin_to_db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

/// Coefficient de lissage exponentiel pour une constante de temps en ms.
#[inline]
pub fn time_coef(ms: f32) -> f32 {
    if ms <= 0.0 {
        0.0
    } else {
        (-1.0 / (ms * 0.001 * SAMPLE_RATE)).exp()
    }
}
