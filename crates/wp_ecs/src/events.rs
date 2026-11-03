/// A per-tick event queue stored as a resource. Events sent during a tick are
/// readable until the queue is cleared at the start of the next tick.
pub struct Events<T> {
    items: Vec<T>,
}

impl<T> Default for Events<T> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl<T> Events<T> {
    pub fn send(&mut self, event: T) {
        self.items.push(event);
    }
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter()
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn clear(&mut self) {
        self.items.clear();
    }
}
