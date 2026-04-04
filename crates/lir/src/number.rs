#[derive(Debug, Clone, PartialEq, Hash)]
pub struct Number(pub u8);

#[derive(Default, Debug, Clone, PartialEq)]
pub struct Counter(usize);

impl Counter {
    pub fn new() -> Self {
        Self(0)
    }

    pub fn count(&mut self) -> usize {
        let r = self.0;
        self.0 += 1;
        r
    }
}
