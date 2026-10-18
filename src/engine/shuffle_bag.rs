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

        // If the first element of the new shuffle matches the last picked element, swap it
        if new_bag.len() > 1 && Some(new_bag[0]) == self.last_picked {
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

    pub fn reset_with_count(&mut self, count: usize) {
        self.items = (0..count).collect();
        self.bag.clear();
        self.last_picked = None;
        self.refill_and_shuffle();
    }
}

