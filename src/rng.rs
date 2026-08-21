//! Seedable randomness for reproducible sampling: a small, fast PRNG and
//! Algorithm R reservoir sampling on top of it. No dependency on `rand`.

/// SplitMix64, the generator Vigna designed as the default seed spreader for
/// xoshiro/xoroshiro. It has no cryptographic pretensions, but it is fast,
/// has no known short cycles for the seeds we care about, and - the only
/// property `sample --seed` actually needs - is fully determined by its
/// 64-bit state.
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64 { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    /// A uniform value in `[0, bound)`. Uses Lemire's rejection method
    /// instead of `next_u64() % bound`, which would bias low values whenever
    /// `bound` does not divide 2^64 evenly - true for every bound we use.
    pub fn next_below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        let bound = bound as u64;
        let mut product = self.next_u64() as u128 * bound as u128;
        let mut low = product as u64;
        if low < bound {
            // Rejection threshold: the count of low values that would make
            // the [0, bound) buckets uneven. Only reached for small bounds
            // relative to 2^64, so the loop below almost never spins twice.
            let threshold = bound.wrapping_neg() % bound;
            while low < threshold {
                product = self.next_u64() as u128 * bound as u128;
                low = product as u64;
            }
        }
        (product >> 64) as usize
    }
}

/// Algorithm R reservoir sampling: a uniform sample of size `capacity` drawn
/// from a stream of unknown length, holding only `capacity` items at a time.
///
/// The reservoir does not preserve arrival order - item 0 can end up at any
/// slot, or none. A caller that wants output in original stream order (as
/// `sample` does) should pair each item with its position before adding it
/// and sort by that position after `into_items`.
pub struct Reservoir<T> {
    capacity: usize,
    items: Vec<T>,
    seen: usize,
}

impl<T> Reservoir<T> {
    pub fn new(capacity: usize) -> Self {
        Reservoir { capacity, items: Vec::with_capacity(capacity), seen: 0 }
    }

    /// Offers one more item from the stream. The first `capacity` items are
    /// always kept; after that, item number `n` (1-based) replaces a
    /// uniformly random slot with probability `capacity / n`.
    pub fn add(&mut self, item: T, rng: &mut SplitMix64) {
        self.seen += 1;
        if self.items.len() < self.capacity {
            self.items.push(item);
            return;
        }
        if self.capacity == 0 {
            return;
        }
        let slot = rng.next_below(self.seen);
        if slot < self.capacity {
            self.items[slot] = item;
        }
    }

    pub fn seen(&self) -> usize {
        self.seen
    }

    pub fn into_items(self) -> Vec<T> {
        self.items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_reproduces_the_sequence() {
        let mut a = SplitMix64::new(123);
        let mut b = SplitMix64::new(123);
        for _ in 0..10 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = SplitMix64::new(1);
        let mut b = SplitMix64::new(2);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn next_below_stays_in_bound() {
        let mut rng = SplitMix64::new(99);
        for _ in 0..2000 {
            assert!(rng.next_below(7) < 7);
        }
    }

    #[test]
    fn next_below_of_one_is_always_zero() {
        let mut rng = SplitMix64::new(5);
        for _ in 0..10 {
            assert_eq!(rng.next_below(1), 0);
        }
    }

    #[test]
    fn next_below_of_zero_is_zero() {
        let mut rng = SplitMix64::new(5);
        assert_eq!(rng.next_below(0), 0);
    }

    #[test]
    fn reservoir_keeps_everything_when_population_fits() {
        let mut reservoir = Reservoir::new(5);
        let mut rng = SplitMix64::new(1);
        for i in 0..5 {
            reservoir.add(i, &mut rng);
        }
        assert_eq!(reservoir.seen(), 5);
        let mut items = reservoir.into_items();
        items.sort_unstable();
        assert_eq!(items, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn reservoir_caps_at_capacity() {
        let mut reservoir = Reservoir::new(3);
        let mut rng = SplitMix64::new(42);
        for i in 0..1000 {
            reservoir.add(i, &mut rng);
        }
        assert_eq!(reservoir.seen(), 1000);
        assert_eq!(reservoir.into_items().len(), 3);
    }

    #[test]
    fn reservoir_of_zero_capacity_selects_nothing() {
        let mut reservoir = Reservoir::new(0);
        let mut rng = SplitMix64::new(7);
        for i in 0..10 {
            reservoir.add(i, &mut rng);
        }
        assert_eq!(reservoir.seen(), 10);
        assert!(reservoir.into_items().is_empty());
    }

    #[test]
    fn reservoir_sampling_is_approximately_uniform() {
        // 20-item population, reservoir of 5: each item should end up
        // selected in roughly capacity/population = 25% of trials.
        const POPULATION: usize = 20;
        const CAPACITY: usize = 5;
        const TRIALS: u64 = 4000;

        let mut counts = [0u32; POPULATION];
        for seed in 0..TRIALS {
            let mut rng = SplitMix64::new(seed * 2 + 1);
            let mut reservoir = Reservoir::new(CAPACITY);
            for i in 0..POPULATION {
                reservoir.add(i, &mut rng);
            }
            for item in reservoir.into_items() {
                counts[item] += 1;
            }
        }

        let expected = TRIALS as f64 * CAPACITY as f64 / POPULATION as f64;
        for (item, &count) in counts.iter().enumerate() {
            let ratio = count as f64 / expected;
            assert!(
                (0.8..1.2).contains(&ratio),
                "item {item} selected {count} times, expected around {expected}"
            );
        }
    }
}
