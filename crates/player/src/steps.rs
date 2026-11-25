//! Speed and subtitle timing, as the keys step them.

/// The speed after a step from `now`: on a grid of quarters, so that pressing the key back
/// returns to 1 exactly, and within what playback can do.
pub fn speed(now: f64, step: f64, min: f64, max: f64) -> f64 {
    (((now + step) * 4.0).round() / 4.0).clamp(min, max)
}

/// Subtitle timing after a step from `now`: in tenths of a second, so that no fraction left
/// over from earlier steps keeps it from coming back to 0.
pub fn subtitle_delay(now: f64, step: f64, most: f64) -> f64 {
    (((now + step) * 10.0).round() / 10.0).clamp(-most, most)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_steps_in_quarters_and_stays_in_range() {
        assert_eq!(speed(1.0, 0.25, 0.5, 2.0), 1.25);
        assert_eq!(speed(1.25, -0.25, 0.5, 2.0), 1.0);
        assert_eq!(speed(2.0, 0.25, 0.5, 2.0), 2.0);
        assert_eq!(speed(0.5, -0.25, 0.5, 2.0), 0.5);
        // A speed set off the grid comes back onto it.
        assert_eq!(speed(1.1, 0.25, 0.5, 2.0), 1.25);
    }

    #[test]
    fn subtitle_timing_steps_in_tenths_and_comes_back_to_zero() {
        let mut d = 0.0;
        for _ in 0..7 {
            d = subtitle_delay(d, 0.1, 30.0);
        }
        assert_eq!(d, 0.7);
        for _ in 0..7 {
            d = subtitle_delay(d, -0.1, 30.0);
        }
        assert_eq!(d, 0.0);
        assert_eq!(subtitle_delay(29.5, 1.0, 30.0), 30.0);
    }
}
