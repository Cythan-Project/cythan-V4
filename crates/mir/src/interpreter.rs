use std::io::Write;

use either::Either;

use crate::{Mir, MirCodeBlock};

// TODO: Move this code toward a better place
pub struct StdIoContext;

pub trait RunContext {
    fn input(&mut self) -> u8;
    /// Consume one byte from the running program. Byte-based (not
    /// `char`-based) on purpose: MIR WriteRegister(0, 1) emits raw
    /// bytes, and casting to `char` would promote values ≥ 128 to
    /// Latin-1 codepoints that UTF-8 re-encoding of `String::push`
    /// would expand into two bytes — corrupting multi-byte input
    /// like `é` (C3 A9) into `Ã©` (C3 83 C2 A9).
    fn print(&mut self, byte: u8);
}

impl RunContext for StdIoContext {
    fn input(&mut self) -> u8 {
        let mut string = String::new();
        std::io::stdin().read_line(&mut string).unwrap();
        string.bytes().next().unwrap()
    }

    fn print(&mut self, byte: u8) {
        std::io::stdout().write_all(&[byte]).unwrap();
        std::io::stdout().flush().unwrap();
    }
}

pub struct MemoryState {
    pub memory: Vec<u8>,
    pub registers: Vec<u8>,
    pub instr_count: usize,
    /// Abort the interpreter after this many MIR ops have executed.
    /// `0` disables the limit. Tests should set a finite value so a
    /// runaway loop fails the test quickly instead of hanging CI.
    pub step_limit: usize,
    /// Set to true when `step_limit` is exceeded. The interpreter
    /// stops executing further ops once this flips (via the same
    /// `SkipStatus::End` path as a `Stop` op).
    pub aborted_by_limit: bool,
}

impl MemoryState {
    pub fn new(memory_size: usize, register_size: usize) -> MemoryState {
        MemoryState {
            memory: vec![0; memory_size],
            registers: vec![0; register_size],
            instr_count: 0,
            step_limit: 0,
            aborted_by_limit: false,
        }
    }

    /// Build a memory state that bounds total MIR steps. Use this in
    /// tests to guarantee termination — exceeding the limit returns
    /// `SkipStatus::End` and flips `aborted_by_limit = true`.
    pub fn new_with_limit(
        memory_size: usize,
        register_size: usize,
        step_limit: usize,
    ) -> MemoryState {
        MemoryState {
            memory: vec![0; memory_size],
            registers: vec![0; register_size],
            instr_count: 0,
            step_limit,
            aborted_by_limit: false,
        }
    }

    pub fn set_mem(&mut self, index: u32, value: u8) {
        if self.memory.len() <= index as usize {
            self.memory.append(
                &mut (0..=(index as usize - self.memory.len()))
                    .map(|_| 0)
                    .collect(),
            );
        }
        self.memory[index as usize] = value;
    }

    pub fn get_mem(&self, index: u32) -> u8 {
        if let Some(e) = self.memory.get(index as usize).copied() {
            e
        } else {
            panic!("Variable wasn't found: {}", index);
        }
    }

    pub fn execute_block(
        &mut self,
        block: &MirCodeBlock,
        printer: &mut impl RunContext,
    ) -> SkipStatus {
        for instruction in block.0.iter() {
            match self.execute(instruction, printer) {
                SkipStatus::None => continue,
                e => return e,
            }
        }
        SkipStatus::None
    }

    pub fn execute(&mut self, mir: &Mir, printer: &mut impl RunContext) -> SkipStatus {
        self.instr_count += 1;
        if self.step_limit > 0 && self.instr_count > self.step_limit {
            self.aborted_by_limit = true;
            return SkipStatus::End;
        }
        match mir {
            // Each memory cell holds a u4 (0..=15). `Set` masks the
            // literal to the cell's range so that e.g. `Set(slot, 42)`
            // stores `10` — otherwise the cell escapes the range the
            // `if_zero` Match (`[0]` vs `1..=15`) can dispatch on and
            // we'd fall through silently inside loops.
            Mir::Set(a, b) => self.set_mem(*a, *b & 0x0F),
            Mir::Copy(a, b) => self.set_mem(*a, self.get_mem(*b) & 0x0F),
            Mir::Increment(a) => self.set_mem(*a, self.get_mem(*a).wrapping_add(1) % 16),
            Mir::Decrement(a) => self.set_mem(*a, self.get_mem(*a).wrapping_sub(1) % 16),
            Mir::If0(a, b, c) => {
                if self.get_mem(*a) == 0 {
                    return self.execute_block(b, printer);
                } else {
                    return self.execute_block(c, printer);
                }
            }
            Mir::Loop(a) => loop {
                match self.execute_block(a, printer) {
                    SkipStatus::None | SkipStatus::Continue => continue,
                    SkipStatus::Break => return SkipStatus::None,
                    e => return e,
                }
            },
            Mir::Break => return SkipStatus::Break,
            Mir::Continue => return SkipStatus::Continue,
            Mir::Stop => return SkipStatus::End,
            Mir::ReadRegister(a, b) => self.set_mem(*a, self.registers[*b as usize]),
            Mir::WriteRegister(a, b) => {
                let p = match b {
                    Either::Left(e) => *e,
                    Either::Right(e) => self.get_mem(*e),
                };
                if *a == 0 {
                    if p == 1 {
                        let a = self.registers[1];
                        let b = self.registers[2];
                        let byte = ((a % 16) * 16) + (b % 16);
                        printer.print(byte);
                    } else if p == 2 {
                        let o: u8 = printer.input();
                        let a = o % 16u8;
                        let b = o / 16u8;
                        self.registers[1] = b;
                        self.registers[2] = a;
                    }
                }
                self.registers[*a as usize] = p
            }
            Mir::Skip => return SkipStatus::Skip,
            Mir::Block(a) => match self.execute_block(a, printer) {
                SkipStatus::Skip => return SkipStatus::None,
                e => return e,
            },
            Mir::Match(a, b) => {
                let k = self.get_mem(*a);
                for (a, b) in b.iter() {
                    if b.contains(&k) {
                        return self.execute_block(a, printer);
                    }
                }
            }
        }
        SkipStatus::None
    }
}

pub enum SkipStatus {
    Break,
    Continue,
    Skip,
    None,
    End,
}
