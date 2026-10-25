//! Summary statistics over a window of samples.
//!
//! A sample is a latency in microseconds, or `NaN` for a ping that never came back.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    /// The most recent sample: `NaN` if it was lost.
    pub last: f64,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub stddev: f64,
    /// Mean absolute difference between consecutive replies, in the order they arrived.
    pub jitter: f64,
    pub p50: f64,
    pub p95: f64,
    pub received: usize,
    pub lost: usize,
}

impl Summary {
    /// Summarises the samples, which must be in chronological order. Returns `None` when
    /// there are no samples at all; when every sample was lost the latency fields are `NaN`.
    pub fn of(samples: impl IntoIterator<Item = f64>) -> Option<Summary> {
        let mut replies = Vec::new();
        let mut lost = 0;
        let mut last = None;
        for value in samples {
            last = Some(value);
            if value.is_nan() {
                lost += 1;
            } else {
                replies.push(value);
            }
        }
        let last = last?;

        let jitter = jitter(&replies);
        let received = replies.len();
        if received == 0 {
            return Some(Summary {
                last,
                min: f64::NAN,
                max: f64::NAN,
                mean: f64::NAN,
                stddev: f64::NAN,
                jitter: f64::NAN,
                p50: f64::NAN,
                p95: f64::NAN,
                received,
                lost,
            });
        }

        let mean = replies.iter().sum::<f64>() / received as f64;
        let variance = replies.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / received as f64;
        let min = replies.iter().copied().fold(f64::INFINITY, f64::min);
        let max = replies.iter().copied().fold(f64::NEG_INFINITY, f64::max);

        Some(Summary {
            last,
            min,
            max,
            mean,
            stddev: variance.sqrt(),
            jitter,
            // Selection rather than a sort keeps this linear: a long --buffer can hold a
            // few hundred thousand samples and this runs every frame.
            p50: percentile(&mut replies, 0.50),
            p95: percentile(&mut replies, 0.95),
            received,
            lost,
        })
    }

    pub fn sent(&self) -> usize {
        self.received + self.lost
    }

    /// Share of the pings in the window that were lost, in `0..=1`.
    pub fn loss(&self) -> f64 {
        match self.sent() {
            0 => 0.0,
            sent => self.lost as f64 / sent as f64,
        }
    }
}

/// Nearest-rank percentile of a non-empty slice: the smallest value that at least `p` of
/// the samples are less than or equal to. Reorders the slice.
pub fn percentile(values: &mut [f64], p: f64) -> f64 {
    debug_assert!(!values.is_empty());
    let rank = (p * values.len() as f64).ceil() as usize;
    let index = rank.clamp(1, values.len()) - 1;
    *values.select_nth_unstable_by(index, f64::total_cmp).1
}

/// Average absolute difference between consecutive values, taken in the order they are
/// given (so the caller passes them chronologically, not sorted). With fewer than two values
/// there is no consecutive pair to compare and jitter is 0.
pub fn jitter(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    values.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (values.len() - 1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_window_has_no_summary() {
        assert_eq!(Summary::of([]), None);
    }

    #[test]
    fn basic_statistics() {
        let s = Summary::of([10.0, 20.0, 30.0, 40.0]).unwrap();
        assert_eq!(s.last, 40.0);
        assert_eq!(s.min, 10.0);
        assert_eq!(s.max, 40.0);
        assert_eq!(s.mean, 25.0);
        assert_eq!(s.jitter, 10.0);
        assert!((s.stddev - 11.180_339_887).abs() < 1e-6);
        assert_eq!((s.received, s.lost), (4, 0));
    }

    // The old p95 indexed `round(0.95 * n)` into the sorted samples, which is one past the
    // end for every n up to 10 and printed 0ns.
    #[test]
    fn p95_is_defined_for_small_windows() {
        for n in 1..=12 {
            let samples: Vec<f64> = (1..=n).map(|v| v as f64).collect();
            let s = Summary::of(samples).unwrap();
            assert_eq!(s.p95, n as f64, "n = {n}");
        }
    }

    #[test]
    fn nearest_rank_percentiles() {
        // Shuffled, to make sure nothing relies on the input being sorted.
        let mut values: Vec<f64> = (1..=100).map(|i| f64::from((i * 37) % 100 + 1)).collect();
        assert_eq!(percentile(&mut values, 0.95), 95.0);
        assert_eq!(percentile(&mut values, 0.50), 50.0);
        assert_eq!(percentile(&mut values, 1.0), 100.0);
        assert_eq!(percentile(&mut values, 0.0), 1.0);
        assert_eq!(percentile(&mut [7.0], 0.95), 7.0);
    }

    #[test]
    fn lost_pings_are_counted_but_not_averaged() {
        let s = Summary::of([10.0, f64::NAN, 30.0, f64::NAN]).unwrap();
        assert!(s.last.is_nan());
        assert_eq!(s.mean, 20.0);
        assert_eq!((s.received, s.lost, s.sent()), (2, 2, 4));
        assert_eq!(s.loss(), 0.5);
        // Jitter compares consecutive replies, skipping the gaps.
        assert_eq!(s.jitter, 20.0);
    }

    #[test]
    fn all_lost() {
        let s = Summary::of([f64::NAN, f64::NAN]).unwrap();
        assert!(s.min.is_nan() && s.mean.is_nan() && s.p95.is_nan());
        assert_eq!(s.loss(), 1.0);
    }

    #[test]
    fn jitter_uses_chronological_order() {
        // Sorted order would telescope to (90 - 10) / 5 = 16; every consecutive pair really
        // differs by 80.
        assert_eq!(jitter(&[10.0, 90.0, 10.0, 90.0, 10.0, 90.0]), 80.0);
        assert_eq!(jitter(&[42.0]), 0.0);
        assert_eq!(jitter(&[]), 0.0);
    }
}
