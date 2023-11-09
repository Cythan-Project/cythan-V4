use std::{fmt::Display, collections::HashSet};

use either::Either;
use lir::{AsmValue, CompilableInstruction, Label, LabelType, Number, Var};

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
    Match(u32, Vec<(MirCodeBlock, Vec<u8>)>),
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
            Self::Match(a, b) => {
                let mut s = String::new();
                s.push_str(&format!("match v{} {{\n", a));
                for (_, (c, v)) in b.iter().enumerate() {
                    s.push_str(&format!("  {:?} => {{\n    ", v));
                    s.push_str(
                        &c.0.iter()
                            .map(|x| x.to_string())
                            .collect::<Vec<_>>()
                            .join("\n")
                            .replace("\n", "\n    "),
                    );
                    s.push_str("\n  }\n");
                }
                s.push_str("}");
                write!(f, "{}", s)
            }
        }
    }
}

impl Mir {
    pub fn get_acesses(&self) -> HashSet<u32> {
        let mut set = HashSet::new();
        match self {
            Mir::Set(a, _) => {
                set.insert(*a);
            }
            Mir::Copy(a, b) => {
                set.insert(*a);
                set.insert(*b);
            }
            Mir::Increment(a) => {
                set.insert(*a);
            }
            Mir::Decrement(a) => {
                set.insert(*a);
            }
            Mir::If0(a, b, c) => {
                set.insert(*a);
                set.extend(b.iter().flat_map(|x| x.get_acesses()));
                set.extend(c.iter().flat_map(|x| x.get_acesses()));
            }
            Mir::Loop(a) => {
                set.extend(a.iter().flat_map(|x| x.get_acesses()));
            }
            Mir::Break => {}
            Mir::Continue => {}
            Mir::Stop => {}
            Mir::ReadRegister(a, _) => {
                set.insert(*a);
            }
            Mir::WriteRegister(_, b) => {
                if let Either::Right(a) = b {
                    set.insert(*a);
                }
            }
            Mir::Skip => {}
            Mir::Block(a) => {
                set.extend(a.iter().flat_map(|x| x.get_acesses()));
            }
            Mir::Match(a, b) => {
                set.insert(*a);
                for (_, (c, _)) in b.iter().enumerate() {
                    set.extend(c.iter().flat_map(|x| x.get_acesses()));
                }
            }
        }
        set
    }
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
            Self::Match(a, b) => {
                let end = Label::alloc(&mut state.count, LabelType::Match);
                let mut g = [
                    None, None, None, None, None, None, None, None, None, None, None, None, None,
                    None, None, None,
                ];
                let mut k = Vec::new();
                for (_, b) in b {
                    let lbl = Label::alloc(&mut state.count, LabelType::Match);
                    for k in b {
                        g[*k as usize] = Some(lbl.clone());
                    }
                    k.push(lbl);
                }
                state
                    .instructions
                    .push(CompilableInstruction::Match(Var(*a as usize), g));

                state.jump(end.clone());

                let mut sk: Option<SkipStatus> = None;

                for ((a, _), p) in b.iter().zip(k.iter()) {
                    state.label(p.clone());
                    let k = a.to_asm(state);
                    sk = match sk {
                        Some(e) => Some(e.lightest(&k)),
                        None => Some(k),
                    };
                    state.jump(end.clone());
                }

                state.label(end);
                return sk.unwrap_or(SkipStatus::None);
            }
        }
        SkipStatus::None
    }
}
