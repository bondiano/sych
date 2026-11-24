pub struct Queue {
    pub items: Vec<String>,
    pub limit: usize,
    served: usize,
}

impl Queue {
    pub fn new() -> Queue {
        Queue {
            items: Vec::new(),
            limit: 3,
            served: 0,
        }
    }

    pub fn full(&self) -> bool {
        self.items.len() + self.served >= self.limit
    }

    pub fn items(&self) -> &Vec<String> {
        &self.items
    }

    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn starts_empty() {
        let mut q = super::Queue::new();
        q.items.push("t".into());
    }
}
