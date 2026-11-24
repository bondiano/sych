pub struct Timers {
    pub armed: usize,
}

impl Timers {
    pub fn arm(&mut self) {
        self.armed += 1;
    }

    pub fn fire(&mut self) {
        self.armed -= 1;
    }
}
