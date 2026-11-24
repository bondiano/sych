pub mod table;

pub struct Inventory {
    pub count: usize,
}

impl Inventory {
    pub fn m0(&self) -> usize {
        self.count + 0
    }

    pub fn m1(&self) -> usize {
        self.count + 1
    }

    pub fn m2(&self) -> usize {
        self.count + 2
    }

    pub fn m3(&self) -> usize {
        self.count + 3
    }

    pub fn m4(&self) -> usize {
        self.count + 4
    }

    pub fn m5(&self) -> usize {
        self.count + 5
    }

    pub fn m6(&self) -> usize {
        self.count + 6
    }

    pub fn m7(&self) -> usize {
        self.count + 7
    }
}
