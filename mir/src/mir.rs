use std::fmt::Display;

use either::Either;
use lir::{AsmValue, Label, LabelType, Number, Var};

use crate::{block::MirCodeBlock, skip_status::SkipStatus, state::MirState};

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
            Self::Copy(a, b) => write!(f, "v{} = v{}", *a, *b),
            Self::Increment(a) => write!(f, "v{}++", *a),
            Self::Decrement(a) => write!(f, "v{}--", *a),
            Self::If0(a, b, c) => {
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
            Self::Loop(a) => write!(
                f,
                "loop {{\n  {}\n}}",
                a.0.iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
                    .replace("\n", "\n  ")
            ),
            Self::Break => write!(f, "break"),
            Self::Continue => write!(f, "continue"),
            Self::Stop => write!(f, "stop"),
            Self::ReadRegister(a, b) => write!(f, "v{} = @{}", a, b),
            Self::WriteRegister(a, b) => write!(
                f,
                "{} <@ {}",
                a,
                match b {
                    Either::Right(a) => format!("v{}", a),
                    Either::Left(a) => a.to_string(),
                }
            ),
            Self::Set(a, b) => write!(f, "v{} = {}", a, b),
            Self::Skip => write!(f, "skip"),
            Self::Block(a) => write!(
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

impl Mir {
    pub fn to_asm(&self, state: &mut MirState) -> SkipStatus {
        match self {
            Self::Copy(a, b) => {
                if a == b {
                    return SkipStatus::None;
                }
                state.copy(Var(*a as usize), AsmValue::Var(Var(*b as usize)))
            }
            Self::Increment(a) => state.inc(Var(*a as usize)),
            Self::Decrement(a) => state.dec(Var(*a as usize)),
            Self::If0(a, b, c) => {
                if b == c {
                    return b.to_asm(state);
                }
                let end = Label::alloc(&mut state.count, LabelType::IfEnd);
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
            Self::Loop(a) => {
                // If this happens this means the program will do nothing forever.
                if a.0.is_empty() {
                    let looplabel = Label::alloc(&mut state.count, LabelType::LoopStart);
                    state.label(looplabel.clone());
                    state.jump(looplabel);
                    return SkipStatus::Stoped;
                }
                let loopstart = Label::alloc(&mut state.count, LabelType::LoopStart);
                let loopend = loopstart.derive(LabelType::LoopEnd);
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
            Self::Break => {
                state.jump(state.loops.last().unwrap().derive(LabelType::LoopEnd)); // TODO: Add error here
                return SkipStatus::Break;
            }
            Self::Continue => {
                state.jump(state.loops.last().unwrap().derive(LabelType::LoopStart)); // TODO: Add error here
                return SkipStatus::Continue;
            }
            Self::Stop => {
                state.stop();
                return SkipStatus::Stoped;
            }
            Self::ReadRegister(a, b) => state.get_reg(Var(*a as usize), Number(*b)),
            Self::WriteRegister(a, b) => state.set_reg(
                Number(*a),
                match b {
                    Either::Left(a) => AsmValue::Number(Number(*a)),
                    Either::Right(a) => AsmValue::Var(Var(*a as usize)),
                },
            ),
            Self::Set(a, b) => state.copy(Var(*a as usize), AsmValue::Number(Number(*b))),
            Self::Skip => {
                state.jump(state.blocks.last().unwrap().derive(LabelType::BlockEnd));
                return SkipStatus::Skipped;
            }
            Self::Block(a) => {
                let blockend = Label::alloc(&mut state.count, LabelType::BlockEnd);
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
