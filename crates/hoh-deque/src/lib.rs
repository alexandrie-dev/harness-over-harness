//! Circular deque with immediate reference release and quarter-full compaction.
const MIN_CAPACITY: usize = 16;

pub struct Deque<T> {
    buffer: Vec<Option<T>>,
    head: usize,
    count: usize,
}
impl<T> Default for Deque<T> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T> Deque<T> {
    pub fn new() -> Self {
        Self {
            buffer: (0..MIN_CAPACITY).map(|_| None).collect(),
            head: 0,
            count: 0,
        }
    }
    pub fn size(&self) -> u64 {
        self.count as u64
    }
    /// Backing slot count, exposed for storage-retention checks.
    pub fn capacity(&self) -> u64 {
        self.buffer.len() as u64
    }
    pub fn push_back(&mut self, value: T) {
        self.ensure_capacity();
        let tail = (self.head + self.count) % self.buffer.len();
        self.buffer[tail] = Some(value);
        self.count += 1;
    }
    pub fn push_front(&mut self, value: T) {
        self.ensure_capacity();
        self.head = if self.head == 0 {
            self.buffer.len() - 1
        } else {
            self.head - 1
        };
        self.buffer[self.head] = Some(value);
        self.count += 1;
    }
    pub fn pop_front(&mut self) -> Option<T> {
        if self.count == 0 {
            return None;
        }
        let value = self.buffer[self.head].take();
        self.head = (self.head + 1) % self.buffer.len();
        self.count -= 1;
        if self.count == 0 {
            self.head = 0;
        } else if self.buffer.len() > MIN_CAPACITY && self.count <= self.buffer.len() / 4 {
            self.resize(MIN_CAPACITY.max(self.buffer.len() / 2));
        }
        value
    }
    pub fn clear(&mut self) {
        *self = Self::new();
    }
    fn ensure_capacity(&mut self) {
        if self.count == self.buffer.len() {
            self.resize(self.buffer.len() * 2);
        }
    }
    fn resize(&mut self, capacity: usize) {
        let mut next: Vec<Option<T>> = (0..capacity).map(|_| None).collect();
        let len = self.buffer.len();
        for (index, slot) in next.iter_mut().take(self.count).enumerate() {
            *slot = self.buffer[(self.head + index) % len].take();
        }
        self.buffer = next;
        self.head = 0;
    }
}
