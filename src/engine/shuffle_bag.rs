use rand::seq::SliceRandom;
use rand::thread_rng;

#[derive(Debug, Clone)]
pub struct ShuffleBag {
    items: Vec<usize>,
    bag: Vec<usize>,
    last_picked: Option<usize>,
}

impl ShuffleBag {
    pub fn new(count: usize) -> Self {
        let items: Vec<usize> = (0..count).collect();
        let mut bag = Self {
            items,
            bag: Vec::new(),
            last_picked: None,
        };
        bag.refill_and_shuffle();
        bag
    }

    fn refill_and_shuffle(&mut self) {
        if self.items.is_empty() {
            self.bag.clear();
            return;
        }

        let mut rng = thread_rng();
        let mut new_bag = self.items.clone();
        new_bag.shuffle(&mut rng);

        // Items are picked from the end, so make sure the next pick isn't a repeat of the last one
        if new_bag.len() > 1 && new_bag.last().copied() == self.last_picked {
            let last_idx = new_bag.len() - 1;
            new_bag.swap(0, last_idx);
        }

        self.bag = new_bag;
    }

    pub fn pick_next(&mut self) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }

        if self.bag.is_empty() {
            self.refill_and_shuffle();
        }

        let picked = self.bag.pop();
        if let Some(p) = picked {
            self.last_picked = Some(p);
        }
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_repeats_back_to_back() {
        for n in 2..=6 {
            let mut bag = ShuffleBag::new(n);
            let mut prev = bag.pick_next();
            for _ in 0..10_000 {
                let next = bag.pick_next();
                assert_ne!(prev, next, "repeat with n={}", n);
                prev = next;
            }
        }
    }

    #[test]
    fn every_item_once_per_cycle() {
        let n = 5;
        let mut bag = ShuffleBag::new(n);
        for _ in 0..100 {
            let mut seen: Vec<usize> = (0..n).map(|_| bag.pick_next().unwrap()).collect();
            seen.sort();
            assert_eq!(seen, (0..n).collect::<Vec<_>>());
        }
    }

    #[test]
    fn edge_sizes() {
        assert_eq!(ShuffleBag::new(0).pick_next(), None);
        let mut one = ShuffleBag::new(1);
        assert_eq!(one.pick_next(), Some(0));
        assert_eq!(one.pick_next(), Some(0));
    }
}
