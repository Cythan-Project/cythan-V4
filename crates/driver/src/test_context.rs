use std::collections::VecDeque;

use mir::RunContext;

/// Test-only `RunContext` — scripted input + byte-level output
/// capture.
///
/// Output is kept as `Vec<u8>` so raw bytes from the running program
/// survive intact: otherwise UTF-8 multi-byte sequences like `é`
/// (`C3 A9`) would round-trip through `char` and re-encode to four
/// bytes. Tests compare via `String::from_utf8_lossy` or the
/// convenience `.as_str()` method below.
pub struct TestContext {
    pub inputs: VecDeque<u8>,
    pub print: Vec<u8>,
}

impl TestContext {
    pub fn new(inputs: &str) -> Self {
        Self {
            inputs: inputs.bytes().collect(),
            print: Vec::new(),
        }
    }

    /// Lossy UTF-8 view of captured output. Ideal for assertions on
    /// game transcripts — ASCII is byte-for-byte, and legitimate
    /// multi-byte UTF-8 sequences (if any escape via print) decode
    /// correctly.
    pub fn as_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.print)
    }
}

impl RunContext for TestContext {
    fn input(&mut self) -> u8 {
        self.inputs.pop_front().unwrap()
    }

    fn print(&mut self, byte: u8) {
        self.print.push(byte);
    }
}
