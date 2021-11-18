pub mod optimize;

use std::fmt::Display;

use either::Either;

use crate::compiler::{
    asm::{AsmValue, LabelType},
    mir::optimize::REMOVE_UNUSED_VARS,
};

use self::optimize::{get_reads_from_block, keep_block, OptimizerState};

use super::asm::{optimizer::opt_asm, CompilableInstruction, Label, Number, Var};

#[derive(PartialEq, Clone, Hash, Debug)]
#[allow(dead_code)]
pub enum Mir {
    Set(u32, u8),
    Copy(u32, u32),                       // to, from - from isn't mutated
    Increment(u32),                       // in, in is mutated
    Decrement(u32),                       // in, in is mutated
    If0(u32, MirCodeBlock, MirCodeBlock), // Jumps to the label if the thing is equals to 0
    Loop(MirCodeBlock),
    Break,
    Continue,
    Stop,
    ReadRegister(u32, u8),
    WriteRegister(u8, Either<u8, u32>),
    Skip,
    Block(MirCodeBlock),
}

impl Display for Mir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mir::Copy(a, b) => write!(f, "v{} = v{}", *a, *b),
            Mir::Increment(a) => write!(f, "v{}++", *a),
            Mir::Decrement(a) => write!(f, "v{}--", *a),
            Mir::If0(a, b, c) => {
                if b.0.is_empty() {
                    write!(
                        f,
                        "if !v{} {{\n  {}\n}}",
                        a,
                        c.0.iter()
                            .map(|x| x.to_string())
                            .collect::<Vec<_>>()
                            .join("\n")
                            .replace("\n", "\n  ")
                    )
                } else if c.0.is_empty() {
                    write!(
                        f,
                        "if v{} {{\n  {}\n}}",
                        a,
                        b.0.iter()
                            .map(|x| x.to_string())
                            .collect::<Vec<_>>()
                            .join("\n")
                            .replace("\n", "\n  ")
                    )
                } else {
                    write!(
                        f,
                        "if v{} {{\n  {}\n}} else {{\n  {}\n}}",
                        a,
                        b.0.iter()
                            .map(|x| x.to_string())
                            .collect::<Vec<_>>()
                            .join("\n")
                            .replace("\n", "\n  "),
                        c.0.iter()
                            .map(|x| x.to_string())
                            .collect::<Vec<_>>()
                            .join("\n")
                            .replace("\n", "\n  ")
                    )
                }
            }
            Mir::Loop(a) => write!(
                f,
                "loop {{\n  {}\n}}",
                a.0.iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
                    .replace("\n", "\n  ")
            ),
            Mir::Break => write!(f, "break"),
            Mir::Continue => write!(f, "continue"),
            Mir::Stop => write!(f, "stop"),
            Mir::ReadRegister(a, b) => write!(f, "v{} = @{}", a, b),
            Mir::WriteRegister(a, b) => write!(
                f,
                "{} <@ {}",
                a,
                match b {
                    Either::Right(a) => format!("v{}", a),
                    Either::Left(a) => a.to_string(),
                }
            ),
            Mir::Set(a, b) => write!(f, "v{} = {}", a, b),
            Mir::Skip => write!(f, "skip"),
            Mir::Block(a) => write!(
                f,
                "block {{\n  {}\n}}",
                a.0.iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
                    .replace("\n", "\n  ")
            ),
        }
    }
}

#[derive(PartialEq, Clone, Hash, Debug)]
pub struct MirCodeBlock(pub Vec<Mir>);

impl MirCodeBlock {
    pub fn to_asm(&self, state: &mut MirState) -> SkipStatus {
        for i in &self.0 {
            match i.to_asm(state) {
                SkipStatus::None => (),
                e => return e,
            }
        }
        SkipStatus::None
    }
}

impl MirCodeBlock {
    pub fn optimize(&self) -> Self {
        let before = self.instr_count();
        let mut after = self.clone();
        let mut bf = before;
        let mut cafter = 0;
        let mut i = 0;
        while bf != cafter {
            bf = cafter;
            let opt = optimize::optimize_block(&after, &mut OptimizerState::new());
            after = if REMOVE_UNUSED_VARS {
                keep_block(&opt, &mut get_reads_from_block(&opt))
            } else {
                opt
            };
            cafter = after.instr_count();
            i += 1;
        }

        println!(
            "Optimized from {} to {} ({}%) in {} iter",
            before,
            cafter,
            100 - 100 * cafter / before,
            i
        );
        after
    }

    pub fn instr_count(&self) -> usize {
        self.0
            .iter()
            .map(|x| match x {
                Mir::If0(_, a, b) => a.instr_count() + b.instr_count() + 1,
                Mir::Loop(a) | Mir::Block(a) => 1 + a.instr_count(),
                _ => 1,
            })
            .sum()
    }
    pub fn from(mir: Vec<Mir>) -> Self {
        Self(mir)
    }
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn add(&mut self, mut mir: MirCodeBlock) -> &mut Self {
        self.0.append(&mut mir.0);
        self
    }

    pub fn add_mir(&mut self, mir: Mir) -> &mut Self {
        self.0.push(mir);
        self
    }

    pub fn copy(&mut self, to: u32, from: u32) -> &mut Self {
        self.0.push(Mir::Copy(to, from));
        self
    }

    pub fn copy_bulk(&mut self, to: &[u32], from: &[u32]) -> &mut Self {
        if to.len() != from.len() {
            panic!("Invalid copy operation");
        }
        to.iter().zip(from.iter()).for_each(|(to, from)| {
            self.0.push(Mir::Copy(*to, *from));
        });
        self
    }

    pub fn set_bulk(&mut self, to: &[u32], from: &[u8]) -> &mut Self {
        if to.len() != from.len() {
            panic!("Invalid set operation");
        }
        to.iter().zip(from.iter()).for_each(|(to, from)| {
            self.0.push(Mir::Set(*to, *from));
        });
        self
    }

    #[allow(dead_code)]
    pub fn set(&mut self, to: u32, value: u8) -> &mut Self {
        self.0.push(Mir::Set(to, value));
        self
    }
}

#[derive(Default)]
pub struct MirState {
    pub count: usize,
    pub instructions: Vec<CompilableInstruction>,
    loops: Vec<Label>,
    blocks: Vec<Label>,
}

impl MirState {
    pub fn opt_asm(&mut self) {
        self.instructions = opt_asm(self.instructions.clone());
    }
    pub fn count(&mut self) -> usize {
        self.count += 1;
        self.count
    }
    pub fn jump(&mut self, label: Label) {
        self.instructions.push(CompilableInstruction::Jump(label));
    }
    pub fn dec(&mut self, variable: Var) {
        self.instructions
            .push(CompilableInstruction::Decrement(variable));
    }
    pub fn inc(&mut self, variable: Var) {
        self.instructions
            .push(CompilableInstruction::Increment(variable));
    }
    pub fn if0(&mut self, variable: Var, label: Label) {
        self.instructions
            .push(CompilableInstruction::If0(variable, label));
    }
    pub fn copy(&mut self, variable: Var, value: AsmValue) {
        self.instructions
            .push(CompilableInstruction::Copy(variable, value));
    }
    pub fn get_reg(&mut self, variable: Var, reg: Number) {
        self.instructions
            .push(CompilableInstruction::ReadRegister(variable, reg));
    }
    pub fn set_reg(&mut self, reg: Number, value: AsmValue) {
        self.instructions
            .push(CompilableInstruction::WriteRegister(reg, value));
    }
    pub fn stop(&mut self) {
        self.instructions.push(CompilableInstruction::Stop);
    }
    pub fn label(&mut self, label: Label) {
        self.instructions.push(CompilableInstruction::Label(label));
    }
}

pub enum SkipStatus {
    Stoped,
    Continue,
    Break,
    None,
    Skipped,
}

impl SkipStatus {
    fn lightest(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::None, _) => Self::None,
            (_, Self::None) => Self::None,
            (Self::Continue, _) => Self::Continue,
            (_, Self::Continue) => Self::Continue,
            (Self::Break, _) => Self::Break,
            (_, Self::Break) => Self::Break,
            (Self::Skipped, _) => Self::Skipped,
            (_, Self::Skipped) => Self::Skipped,
            _ => Self::Stoped,
        }
    }
}

impl Mir {
    pub fn to_asm(&self, state: &mut MirState) -> SkipStatus {
        match self {
            Mir::Copy(a, b) => {
                if a == b {
                    return SkipStatus::None;
                }
                state.copy(Var(*a as usize), AsmValue::Var(Var(*b as usize)))
            }
            Mir::Increment(a) => state.inc(Var(*a as usize)),
            Mir::Decrement(a) => state.dec(Var(*a as usize)),
            Mir::If0(a, b, c) => {
                if b == c {
                    return b.to_asm(state);
                }
                let end = Label::alloc(state, crate::compiler::asm::LabelType::IfEnd);
                if b.0.is_empty() {
                    state.if0(Var(*a as usize), end.clone());
                    b.to_asm(state);
                    state.label(end);
                } else {
                    let start = end.derive(LabelType::IfStart);
                    state.if0(Var(*a as usize), start.clone());
                    let if1 = c.to_asm(state);
                    state.jump(end.clone());
                    state.label(start);
                    let if2 = b.to_asm(state);
                    state.label(end);
                    return if1.lightest(&if2);
                }
            }
            Mir::Loop(a) => {
                // If this happens this means the program will do nothing forever.
                if a.0.is_empty() {
                    let looplabel = Label::alloc(state, crate::compiler::asm::LabelType::LoopStart);
                    state.label(looplabel.clone());
                    state.jump(looplabel);
                    return SkipStatus::Stoped;
                }
                let loopstart = Label::alloc(state, crate::compiler::asm::LabelType::LoopStart);
                let loopend = loopstart.derive(crate::compiler::asm::LabelType::LoopEnd);
                state.label(loopstart.clone());
                state.loops.push(loopstart.clone());
                let k = a.to_asm(state);
                state.loops.pop();
                state.jump(loopstart);
                state.label(loopend);
                if matches!(k, SkipStatus::Stoped) {
                    return SkipStatus::Stoped;
                }
            }
            Mir::Break => {
                state.jump(state.loops.last().unwrap().derive(LabelType::LoopEnd)); // TODO: Add error here
                return SkipStatus::Break;
            }
            Mir::Continue => {
                state.jump(state.loops.last().unwrap().derive(LabelType::LoopStart)); // TODO: Add error here
                return SkipStatus::Continue;
            }
            Mir::Stop => {
                state.stop();
                return SkipStatus::Stoped;
            }
            Mir::ReadRegister(a, b) => state.get_reg(Var(*a as usize), Number(*b)),
            Mir::WriteRegister(a, b) => state.set_reg(
                Number(*a),
                match b {
                    Either::Left(a) => AsmValue::Number(Number(*a)),
                    Either::Right(a) => AsmValue::Var(Var(*a as usize)),
                },
            ),
            Mir::Set(a, b) => state.copy(Var(*a as usize), AsmValue::Number(Number(*b))),
            Mir::Skip => {
                state.jump(state.blocks.last().unwrap().derive(LabelType::BlockEnd));
                return SkipStatus::Skipped;
            }
            Mir::Block(a) => {
                let blockend = Label::alloc(state, crate::compiler::asm::LabelType::BlockEnd);
                state.blocks.push(blockend.clone());
                let k = a.to_asm(state);
                state.blocks.pop();
                state.label(blockend);
                if matches!(k, SkipStatus::Stoped) {
                    return SkipStatus::Stoped;
                }
                if matches!(k, SkipStatus::Break) {
                    return SkipStatus::Break;
                }
                if matches!(k, SkipStatus::Continue) {
                    return SkipStatus::Continue;
                }
            }
        }
        SkipStatus::None
    }
}
