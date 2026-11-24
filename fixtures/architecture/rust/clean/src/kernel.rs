pub mod timers;

pub struct Tasks {
    pub count: usize,
}

impl Tasks {
    pub fn spawn(&mut self, timers: &mut timers::Timers) {
        timers.arm();
        self.count += 1;
    }
}

pub fn tick(tasks: &mut Tasks, timers: &mut timers::Timers) {
    timers.fire();
    tasks.count += 1;
}
