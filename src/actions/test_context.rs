use std::collections::VecDeque;

use mir::RunContext;

impl RunContext for TestContext {
    fn input(&mut self) -> u8 {
        self.inputs.pop_front().unwrap()
    }

    fn print(&mut self, i: char) {
        self.print.push(i);
    }
}
pub struct TestContext {
    pub inputs: VecDeque<u8>,
    pub print: String,
}

impl TestContext {
    #[allow(dead_code)]
    pub fn new(inputs: &str) -> Self {
        Self {
            inputs: inputs.bytes().collect(),
            print: String::new(),
        }
    }
}
