use std::io::Write;

use either::Either;

use crate::{Mir, MirCodeBlock};

// TODO: Move this code toward a better place
pub struct StdIoContext;

pub trait RunContext {
    fn input(&mut self) -> u8;
    fn print(&mut self, i: char);
}

impl RunContext for StdIoContext {
    fn input(&mut self) -> u8 {
        let mut string = String::new();
        std::io::stdin().read_line(&mut string).unwrap();
        string.bytes().next().unwrap()
    }

    fn print(&mut self, i: char) {
        print!("{}", i);
        std::io::stdout().flush().unwrap();
    }
}

pub struct MemoryState {
    pub memory: Vec<u8>,
    pub registers: Vec<u8>,
    pub instr_count: usize,
}

impl MemoryState {
    pub fn new(memory_size: usize, register_size: usize) -> MemoryState {
        MemoryState {
            memory: vec![0; memory_size],
            registers: vec![0; register_size],
            instr_count: 0,
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
        match mir {
            Mir::Set(a, b) => self.set_mem(*a, *b),
            Mir::Copy(a, b) => self.set_mem(*a, self.get_mem(*b)),
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
                        let char = ((a % 16) * 16) + (b % 16);
                        printer.print(char as char);
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
