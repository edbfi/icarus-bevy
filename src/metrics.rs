//! Fixed-memory latency distributions for long-running measurements.

/// Latency counts at a declared resolution with a constant memory footprint.
///
/// Samples at or above `buckets * resolution_micros` count as overflow; the exact
/// maximum is kept separately. Quantiles use the sorted-sample rank
/// `floor((n - 1) * percentile / 100)`. They are exact for samples below the
/// overflow bound, rounded down to the resolution, and unknown (`None`) when the
/// rank falls in the overflow range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatencyHistogram {
    resolution_micros: u64,
    counts: Box<[u64]>,
    overflow: u64,
    total: u64,
    max_micros: u64,
}

impl LatencyHistogram {
    /// # Panics
    ///
    /// Panics when the resolution or bucket count is zero, or their product
    /// does not fit in `u64`.
    pub fn new(resolution_micros: u64, buckets: usize) -> Self {
        assert!(
            resolution_micros > 0 && buckets > 0,
            "histogram resolution and bucket count must be nonzero"
        );
        assert!(
            u64::try_from(buckets)
                .ok()
                .and_then(|buckets| buckets.checked_mul(resolution_micros))
                .is_some(),
            "histogram range must fit in u64"
        );
        Self {
            resolution_micros,
            counts: vec![0; buckets].into_boxed_slice(),
            overflow: 0,
            total: 0,
            max_micros: 0,
        }
    }

    pub fn record(&mut self, micros: u64) {
        let bucket = usize::try_from(micros / self.resolution_micros).unwrap_or(usize::MAX);
        match self.counts.get_mut(bucket) {
            Some(count) => *count = count.saturating_add(1),
            None => self.overflow = self.overflow.saturating_add(1),
        }
        self.total = self.total.saturating_add(1);
        self.max_micros = self.max_micros.max(micros);
    }

    pub fn count(&self) -> u64 {
        self.total
    }

    pub fn overflow(&self) -> u64 {
        self.overflow
    }

    pub fn max_micros(&self) -> Option<u64> {
        (self.total > 0).then_some(self.max_micros)
    }

    /// Exclusive upper bound of the exactly counted range.
    pub fn range_micros(&self) -> u64 {
        self.counts.len() as u64 * self.resolution_micros
    }

    pub fn quantile_micros(&self, percentile: u64) -> Option<u64> {
        if self.total == 0 || percentile > 100 {
            return None;
        }
        let rank = (u128::from(self.total - 1) * u128::from(percentile)) / 100;
        let mut seen = 0_u128;
        for (bucket, count) in self.counts.iter().enumerate() {
            seen += u128::from(*count);
            if rank < seen {
                return Some(bucket as u64 * self.resolution_micros);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::LatencyHistogram;

    fn sorted_rank(samples: &[u64], percentile: u64) -> u64 {
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        sorted[(sorted.len() - 1) * percentile as usize / 100]
    }

    #[test]
    fn empty_histogram_has_no_quantiles_or_maximum() {
        let histogram = LatencyHistogram::new(1, 8);
        assert_eq!(histogram.count(), 0);
        assert_eq!(histogram.max_micros(), None);
        assert_eq!(histogram.quantile_micros(50), None);
    }

    #[test]
    fn unit_resolution_matches_the_sorted_sample_rule() {
        // Deterministic pseudo-random samples spanning the counted range.
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let samples: Vec<u64> = (0..10_007)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state % 50_000
            })
            .collect();
        let mut histogram = LatencyHistogram::new(1, 50_000);
        for sample in &samples {
            histogram.record(*sample);
        }
        assert_eq!(histogram.count(), samples.len() as u64);
        assert_eq!(histogram.overflow(), 0);
        for percentile in [0, 1, 25, 50, 75, 90, 99, 100] {
            assert_eq!(
                histogram.quantile_micros(percentile),
                Some(sorted_rank(&samples, percentile)),
                "p{percentile}"
            );
        }
        assert_eq!(histogram.max_micros(), samples.iter().copied().max());
    }

    #[test]
    fn coarse_resolution_rounds_down_to_the_bucket_start() {
        let mut histogram = LatencyHistogram::new(100, 10);
        for sample in [99, 100, 199, 250, 999] {
            histogram.record(sample);
        }
        assert_eq!(histogram.quantile_micros(0), Some(0));
        assert_eq!(histogram.quantile_micros(50), Some(100));
        assert_eq!(histogram.quantile_micros(100), Some(900));
        assert_eq!(histogram.range_micros(), 1_000);
    }

    #[test]
    fn overflow_ranks_are_unknown_but_the_maximum_is_exact() {
        let mut histogram = LatencyHistogram::new(10, 10);
        for sample in [5, 15, 25, 1_000, 123_456] {
            histogram.record(sample);
        }
        assert_eq!(histogram.overflow(), 2);
        assert_eq!(histogram.quantile_micros(50), Some(20));
        assert_eq!(histogram.quantile_micros(99), None);
        assert_eq!(histogram.max_micros(), Some(123_456));
    }

    #[test]
    fn percentiles_above_one_hundred_are_rejected() {
        let mut histogram = LatencyHistogram::new(1, 4);
        histogram.record(1);
        assert_eq!(histogram.quantile_micros(101), None);
    }

    #[test]
    fn extreme_samples_do_not_overflow_counters_or_ranks() {
        let mut histogram = LatencyHistogram::new(1, 4);
        histogram.record(u64::MAX);
        histogram.record(3);
        assert_eq!(histogram.overflow(), 1);
        assert_eq!(histogram.quantile_micros(0), Some(3));
        assert_eq!(histogram.max_micros(), Some(u64::MAX));
    }

    #[test]
    #[should_panic(expected = "nonzero")]
    fn zero_resolution_is_rejected() {
        let _ = LatencyHistogram::new(0, 4);
    }
}
