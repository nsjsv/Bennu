use std::time::{Duration, Instant};

pub(crate) fn elapsed_fraction(started_at: Instant, duration: Duration) -> f32 {
    elapsed_fraction_at(started_at, Instant::now(), duration)
}

pub(crate) fn elapsed_fraction_at(started_at: Instant, now: Instant, duration: Duration) -> f32 {
    if duration.is_zero() {
        return 1.0;
    }

    (now.saturating_duration_since(started_at).as_secs_f32() / duration.as_secs_f32())
        .clamp(0.0, 1.0)
}

pub(crate) fn ease_out_cubic(progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    1.0 - (1.0 - progress).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_fraction_is_clamped_and_zero_duration_is_complete() {
        let started_at = Instant::now();

        assert_eq!(
            elapsed_fraction_at(
                started_at,
                started_at - Duration::from_millis(1),
                Duration::from_millis(100),
            ),
            0.0
        );
        assert_eq!(
            elapsed_fraction_at(
                started_at,
                started_at + Duration::from_millis(50),
                Duration::from_millis(100),
            ),
            0.5
        );
        assert_eq!(
            elapsed_fraction_at(
                started_at,
                started_at + Duration::from_millis(200),
                Duration::from_millis(100),
            ),
            1.0
        );
        assert_eq!(
            elapsed_fraction_at(started_at, started_at, Duration::ZERO),
            1.0
        );
    }

    #[test]
    fn cubic_ease_out_clamps_input_and_keeps_endpoints() {
        assert_eq!(ease_out_cubic(-1.0), 0.0);
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(0.5), 0.875);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert_eq!(ease_out_cubic(2.0), 1.0);
    }
}
