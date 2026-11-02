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
    if ms <= 0.0 { 0.0 } else { (-1.0 / (ms * 0.001 * SAMPLE_RATE)).exp() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_and_linear_are_inverse() {
        for db in [-60.0, -20.0, -6.0, 0.0, 6.0, 20.0] {
            assert!((lin_to_db(db_to_lin(db)) - db).abs() < 1e-3, "{db} dB");
        }
        assert!((db_to_lin(-6.0206) - 0.5).abs() < 1e-4);
        assert!((db_to_lin(0.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn lin_to_db_never_returns_minus_infinity() {
        assert!((lin_to_db(0.0) + 180.0).abs() < 1e-3);
        assert!(lin_to_db(-1.0).is_finite());
    }

    #[test]
    fn time_coef_matches_one_time_constant() {
        assert!(time_coef(0.0).abs() < f32::EPSILON);
        assert!(time_coef(-5.0).abs() < f32::EPSILON);
        // Après `ms` millisecondes, il reste e^-1 ≈ 36,8 % de l'écart.
        let c = time_coef(10.0);
        let remaining = c.powi((0.010 * SAMPLE_RATE) as i32);
        assert!((remaining - (-1f32).exp()).abs() < 1e-3, "{remaining}");
        assert!(time_coef(100.0) > c, "constante plus longue = lissage plus lent");
    }
}
