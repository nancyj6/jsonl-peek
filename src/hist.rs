use std::collections::BTreeMap;

/// Values below this are counted exactly, one bucket per integer. Below 16
/// bytes a log bucket would be wider than the values it covers, so exact
/// counting is both cheaper and more accurate.
const EXACT_LIMIT: u64 = 16;

/// Growth factor between adjacent log buckets above `EXACT_LIMIT`. A bucket's
/// representative value is its geometric mean, so the worst-case relative
/// error is `sqrt(LOG_BASE) - 1`; at 1.06 that is just under 3%.
const LOG_BASE: f64 = 1.06;

/// A log-bucketed histogram for percentile estimation over a stream too
/// large to sort. `min`, `max`, `sum` (and so `mean`) are exact; `percentile`
/// is exact for values under 16 and approximate above it, per `LOG_BASE`.
pub struct Histogram {
    count: u64,
    min: Option<u64>,
    max: Option<u64>,
    sum: u128,
    exact: [u64; EXACT_LIMIT as usize],
    buckets: BTreeMap<u32, u64>,
}

impl Histogram {
    pub fn new() -> Self {
        Histogram {
            count: 0,
            min: None,
            max: None,
            sum: 0,
            exact: [0; EXACT_LIMIT as usize],
            buckets: BTreeMap::new(),
        }
    }

    pub fn record(&mut self, value: u64) {
        self.count += 1;
        self.sum += value as u128;
        self.min = Some(self.min.map_or(value, |m| m.min(value)));
        self.max = Some(self.max.map_or(value, |m| m.max(value)));
        if value < EXACT_LIMIT {
            self.exact[value as usize] += 1;
        } else {
            *self.buckets.entry(bucket_index(value)).or_insert(0) += 1;
        }
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn min(&self) -> Option<u64> {
        self.min
    }

    pub fn max(&self) -> Option<u64> {
        self.max
    }

    pub fn mean(&self) -> Option<f64> {
        if self.count == 0 {
            None
        } else {
            Some(self.sum as f64 / self.count as f64)
        }
    }

    /// The value at quantile `p` (`0.5` for the median, `0.99` for p99),
    /// using nearest-rank selection over the exact and bucketed counts.
    /// `None` on an empty histogram.
    pub fn percentile(&self, p: f64) -> Option<u64> {
        if self.count == 0 {
            return None;
        }
        let rank = ((p * self.count as f64).ceil() as u64).clamp(1, self.count);
        let mut remaining = rank;
        for (value, &count) in self.exact.iter().enumerate() {
            if count == 0 {
                continue;
            }
            if remaining <= count {
                return Some(value as u64);
            }
            remaining -= count;
        }
        for (&idx, &count) in &self.buckets {
            if remaining <= count {
                return Some(bucket_representative(idx));
            }
            remaining -= count;
        }
        self.max
    }
}

impl Default for Histogram {
    fn default() -> Self {
        Histogram::new()
    }
}

fn bucket_index(value: u64) -> u32 {
    let ratio = value as f64 / EXACT_LIMIT as f64;
    (ratio.ln() / LOG_BASE.ln()).floor() as u32
}

fn bucket_representative(idx: u32) -> u64 {
    (EXACT_LIMIT as f64 * LOG_BASE.powf(idx as f64 + 0.5)).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_histogram_has_no_stats() {
        let hist = Histogram::new();
        assert_eq!(hist.count(), 0);
        assert_eq!(hist.min(), None);
        assert_eq!(hist.max(), None);
        assert_eq!(hist.mean(), None);
        assert_eq!(hist.percentile(0.5), None);
    }

    #[test]
    fn tracks_exact_min_max_mean_and_count() {
        let mut hist = Histogram::new();
        for value in [10, 3, 7, 25, 1000] {
            hist.record(value);
        }
        assert_eq!(hist.count(), 5);
        assert_eq!(hist.min(), Some(3));
        assert_eq!(hist.max(), Some(1000));
        assert_eq!(hist.mean(), Some((10 + 3 + 7 + 25 + 1000) as f64 / 5.0));
    }

    #[test]
    fn percentiles_are_exact_below_the_log_bucket_threshold() {
        let mut hist = Histogram::new();
        for value in 0..EXACT_LIMIT {
            hist.record(value);
        }
        assert_eq!(hist.percentile(0.0), Some(0));
        assert_eq!(hist.percentile(1.0), Some(EXACT_LIMIT - 1));
        // Nearest-rank median of 0..16: rank = ceil(0.5 * 16) = 8th value (1-indexed) = 7.
        assert_eq!(hist.percentile(0.5), Some(7));
    }

    #[test]
    fn single_value_percentiles_return_that_value() {
        let mut hist = Histogram::new();
        hist.record(42);
        assert_eq!(hist.percentile(0.0), Some(42));
        assert_eq!(hist.percentile(0.5), Some(42));
        assert_eq!(hist.percentile(1.0), Some(42));
    }

    #[test]
    fn large_values_stay_within_the_documented_error_bound() {
        let mut hist = Histogram::new();
        let mut exact_values = Vec::new();
        for i in 0..10_000u64 {
            // A spread of magnitudes so buckets across the whole log range get exercised.
            let value = 20 + (i * i) % 50_000;
            hist.record(value);
            exact_values.push(value);
        }
        exact_values.sort_unstable();

        for p in [0.5, 0.9, 0.99] {
            let rank = ((p * exact_values.len() as f64).ceil() as usize).clamp(1, exact_values.len());
            let expected = exact_values[rank - 1] as f64;
            let got = hist.percentile(p).unwrap() as f64;
            let relative_error = (got - expected).abs() / expected;
            assert!(
                relative_error <= 0.05,
                "p{}: expected {expected}, got {got}, relative error {relative_error}",
                (p * 100.0) as u32
            );
        }
    }

    #[test]
    fn bucket_index_is_monotonic_in_value() {
        let mut prev = bucket_index(EXACT_LIMIT);
        for value in (EXACT_LIMIT + 1)..1_000_000 {
            let idx = bucket_index(value);
            assert!(idx >= prev, "bucket index decreased at {value}");
            prev = idx;
        }
    }
}
