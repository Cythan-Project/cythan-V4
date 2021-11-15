use std::io::Write;

use either::Either;

use crate::compiler::mir::{Mir, MirCodeBlock};

pub struct MemoryState {
    pub memory: Vec<u8>,
    pub registers: Vec<u8>,
}

impl MemoryState {
    pub fn new(memory_size: usize, register_size: usize) -> MemoryState {
        MemoryState {
            memory: vec![0; memory_size],
            registers: vec![0; register_size],
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
        self.memory.get(index as usize).copied().unwrap_or(0)
    }

    pub fn execute_block(&mut self, block: &MirCodeBlock) -> SkipStatus {
        for instruction in block.0.iter() {
            match self.execute(instruction) {
                SkipStatus::None => continue,
                e => return e,
            }
        }
        SkipStatus::None
    }

    pub fn execute(&mut self, mir: &Mir) -> SkipStatus {
        match mir {
            Mir::Set(a, b) => self.set_mem(*a, *b),
            Mir::Copy(a, b) => self.set_mem(*a, self.get_mem(*b)),
            Mir::Increment(a) => self.set_mem(*a, self.get_mem(*a).wrapping_add(1)),
            Mir::Decrement(a) => self.set_mem(*a, self.get_mem(*a).wrapping_sub(1)),
            Mir::If0(a, b, c) => {
                if self.get_mem(*a) == 0 {
                    return self.execute_block(b);
                } else {
                    return self.execute_block(c);
                }
            }
            Mir::Loop(a) => loop {
                match self.execute_block(a) {
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
                        print!("{}", char as char);
                        std::io::stdout().flush().unwrap();
                    } else if p == 2 {
                        let mut k = String::new();
                        std::io::stdin().read_line(&mut k).unwrap();
                        let o: u8 = k.trim().parse().unwrap();
                        let a = o % 16u8;
                        let b = o / 16u8;
                        self.registers[1] = b;
                        self.registers[2] = a;
                    }
                }
                self.registers[*a as usize] = p
            }
            Mir::Skip => return SkipStatus::Skip,
            Mir::Block(a) => match self.execute_block(a) {
                SkipStatus::Skip => return SkipStatus::None,
                e => return e,
            },
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
