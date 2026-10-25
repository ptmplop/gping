//! A compact record of the whole session, for the readout sparklines.
//!
//! Samples are folded into fixed-width time buckets. When there are too many buckets,
//! neighbours are merged pairwise and the bucket width doubles, so memory stays bounded
//! however long gping runs, while the record always spans the entire session.

const MAX_BUCKETS: usize = 512;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Bucket {
    /// Sum of the reply latencies, in microseconds.
    pub sum: f64,
    pub max: f64,
    pub replies: u32,
    pub lost: u32,
}

impl Bucket {
    pub fn mean(&self) -> Option<f64> {
        (self.replies > 0).then(|| self.sum / f64::from(self.replies))
    }

    pub fn is_empty(&self) -> bool {
        self.replies == 0 && self.lost == 0
    }

    fn add(&mut self, other: &Bucket) {
        self.sum += other.sum;
        self.max = self.max.max(other.max);
        self.replies += other.replies;
        self.lost += other.lost;
    }
}

pub struct History {
    start: f64,
    /// Seconds per bucket.
    width: f64,
    buckets: Vec<Bucket>,
}

impl History {
    pub fn new(start: f64) -> History {
        History {
            start,
            width: 1.0,
            buckets: Vec::new(),
        }
    }

    /// Records a sample taken at `at` (seconds, same clock as `start`): a latency in
    /// microseconds, or NaN for a lost ping.
    pub fn record(&mut self, at: f64, value: f64) {
        let offset = (at - self.start).max(0.0);
        let mut index = (offset / self.width) as usize;
        while index >= MAX_BUCKETS {
            self.merge();
            index = (offset / self.width) as usize;
        }
        if index >= self.buckets.len() {
            self.buckets.resize(index + 1, Bucket::default());
        }
        let bucket = &mut self.buckets[index];
        if value.is_nan() {
            bucket.lost += 1;
        } else {
            bucket.sum += value;
            bucket.max = bucket.max.max(value);
            bucket.replies += 1;
        }
    }

    fn merge(&mut self) {
        self.buckets = self
            .buckets
            .chunks(2)
            .map(|pair| {
                let mut merged = pair[0];
                if let Some(second) = pair.get(1) {
                    merged.add(second);
                }
                merged
            })
            .collect();
        self.width *= 2.0;
    }

    /// The session squeezed (or stretched) into `columns` buckets, oldest first. Each column
    /// combines the buckets that fall into its share of the session.
    pub fn columns(&self, columns: usize) -> Vec<Bucket> {
        let mut out = vec![Bucket::default(); columns];
        if columns == 0 || self.buckets.is_empty() {
            return out;
        }
        let n = self.buckets.len();
        for (i, bucket) in self.buckets.iter().enumerate() {
            let column = (i * columns / n).min(columns - 1);
            out[column].add(bucket);
        }
        // With fewer buckets than columns some columns get nothing; spread the buckets out
        // instead of leaving gaps between them.
        if n < columns {
            out = (0..columns).map(|c| self.buckets[c * n / columns]).collect();
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.iter().all(Bucket::is_empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_land_in_their_second() {
        let mut h = History::new(100.0);
        h.record(100.2, 10.0);
        h.record(100.7, 30.0);
        h.record(101.1, f64::NAN);
        assert_eq!(h.buckets.len(), 2);
        assert_eq!(h.buckets[0].mean(), Some(20.0));
        assert_eq!(h.buckets[0].max, 30.0);
        assert_eq!(h.buckets[1].lost, 1);
        assert_eq!(h.buckets[1].mean(), None);
    }

    #[test]
    fn memory_stays_bounded_for_long_sessions() {
        let mut h = History::new(0.0);
        for second in 0..100_000 {
            h.record(second as f64, 1.0);
        }
        assert!(h.buckets.len() <= MAX_BUCKETS);
        let total: u32 = h.buckets.iter().map(|b| b.replies).sum();
        assert_eq!(total, 100_000);
    }

    #[test]
    fn columns_cover_the_whole_session() {
        let mut h = History::new(0.0);
        for second in 0..100 {
            h.record(second as f64, f64::from(second));
        }
        let columns = h.columns(10);
        assert_eq!(columns.len(), 10);
        assert!(columns.iter().all(|c| c.replies == 10));
        assert_eq!(columns[9].max, 99.0);
    }

    #[test]
    fn a_short_session_is_stretched_not_gappy() {
        let mut h = History::new(0.0);
        h.record(0.0, 5.0);
        h.record(1.0, 7.0);
        let columns = h.columns(6);
        assert!(columns.iter().all(|c| !c.is_empty()));
    }

    #[test]
    fn edge_cases() {
        let h = History::new(0.0);
        assert!(h.is_empty());
        assert_eq!(h.columns(0).len(), 0);
        assert!(h.columns(4).iter().all(Bucket::is_empty));
        let mut h = History::new(10.0);
        // A clock that steps backwards lands in the first bucket instead of panicking.
        h.record(5.0, 1.0);
        assert_eq!(h.buckets[0].replies, 1);
    }
}
