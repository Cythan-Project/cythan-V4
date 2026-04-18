//! HIR interpreter.
//!
//! Executes `HirFunction`s directly (no MIR / bytecode). Each function has a
//! local slot array sized by `HirFunction::slot_count`. Calls dispatch to
//! another `HirFunction` looked up by `FnSig`; args are copied into the
//! callee's input slots, output slots are copied back into the caller's
//! `ret` list.
//!
//! I/O follows the same register protocol as the MIR interpreter:
//!   - `WriteRegister(0, 1)` → print char composed from registers[1..=2]
//!   - `WriteRegister(0, 2)` → read a byte, split nibbles into registers[1..=2]

use std::collections::{HashMap, VecDeque};

use either::Either;

use crate::ir::*;

// ---- I/O trait ------------------------------------------------------------

pub trait IoContext {
    fn input(&mut self) -> u8;
    /// Byte-oriented (see `mir::RunContext::print` for rationale —
    /// char-based printing corrupts bytes ≥ 128 via UTF-8 re-encoding).
    fn print(&mut self, byte: u8);
}

/// Captures stdout into a byte buffer and reads stdin bytes from a
/// queue. Ideal for tests. The byte buffer preserves raw output
/// bytes exactly; decode via `String::from_utf8_lossy` or the
/// `stdout_str()` helper at assertion time.
pub struct CapturedIo {
    pub stdin: VecDeque<u8>,
    pub stdout: Vec<u8>,
}

impl CapturedIo {
    pub fn new() -> Self {
        Self {
            stdin: VecDeque::new(),
            stdout: Vec::new(),
        }
    }

    pub fn with_input(bytes: impl IntoIterator<Item = u8>) -> Self {
        Self {
            stdin: bytes.into_iter().collect(),
            stdout: Vec::new(),
        }
    }

    /// Lossy UTF-8 view — safe for assertions on ASCII-only
    /// transcripts; multi-byte sequences decode normally.
    pub fn stdout_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.stdout)
    }
}

impl Default for CapturedIo {
    fn default() -> Self {
        Self::new()
    }
}

impl IoContext for CapturedIo {
    fn input(&mut self) -> u8 {
        self.stdin.pop_front().unwrap_or(0)
    }

    fn print(&mut self, byte: u8) {
        self.stdout.push(byte);
    }
}

// ---- interpreter ----------------------------------------------------------

#[derive(Debug, Clone)]
pub enum InterpError {
    UnknownFunction(String, String),
    TargetNotSimple(String, String),
    /// Step ceiling hit — almost always indicates an infinite loop.
    /// Carries the step count so callers can log a useful diagnostic.
    StepLimit(usize),
    Custom(String),
}

impl std::fmt::Display for InterpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownFunction(t, m) => write!(f, "unknown function {}::{}", t, m),
            Self::StepLimit(n) => write!(
                f,
                "HIR interpreter exceeded step limit ({} ops) — likely infinite loop",
                n
            ),
            Self::TargetNotSimple(t, m) => write!(
                f,
                "call target {}::{} is templated (not yet monomorphized)",
                t, m
            ),
            Self::Custom(s) => f.write_str(s),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    None,
    Break,
    Continue,
    Stop,
    Skip,
}

pub struct Interpreter<'a, C: IoContext> {
    functions: HashMap<crate::FnSigKey, HirFunction>,
    ctx: &'a mut C,
    registers: [u8; 4],
    /// Operations executed so far — compared against `step_limit` to
    /// bound runaway programs.
    step_count: usize,
    /// Max ops to run before bailing with `InterpError::StepLimit`.
    /// `0` disables the limit.
    pub step_limit: usize,
}

/// Re-exported from `typer::FnSig` for convenience so call sites don't need
/// to import both crates. Kept as a local alias so the interpreter API reads
/// cleanly; note that in `*.rs` files that already use `typer::FnSig`, you
/// may prefer `typer::FnSig` directly.
pub type FnSigKey = typer::FnSig;

impl<'a, C: IoContext> Interpreter<'a, C> {
    /// Default op ceiling for the HIR interpreter. Tests that compile
    /// real programs can top out in the low millions; beyond that is
    /// virtually always an infinite loop.
    pub const DEFAULT_STEP_LIMIT: usize = 2_000_000;

    pub fn new(functions: HashMap<FnSigKey, HirFunction>, ctx: &'a mut C) -> Self {
        Self {
            functions,
            ctx,
            registers: [0; 4],
            step_count: 0,
            step_limit: Self::DEFAULT_STEP_LIMIT,
        }
    }

    /// Override the default step limit. Pass `0` to disable (only
    /// sensible for interactive runs — tests should always cap).
    pub fn with_step_limit(mut self, limit: usize) -> Self {
        self.step_limit = limit;
        self
    }

    /// Run the function identified by `entry`, passing `args` as the input
    /// slot values (must match `entry`'s `input_count`). Returns the output
    /// slot values.
    pub fn run(&mut self, entry: &FnSigKey, args: &[u8]) -> Result<Vec<u8>, InterpError> {
        let f = self
            .functions
            .get(entry)
            .cloned()
            .ok_or_else(|| InterpError::UnknownFunction(entry.type_name.clone(), entry.method_name.clone()))?;
        if args.len() as u32 != f.sig.input_count {
            return Err(InterpError::Custom(format!(
                "{}::{}: expected {} input cells, got {}",
                entry.type_name,
                entry.method_name,
                f.sig.input_count,
                args.len(),
            )));
        }
        let mut slots: Vec<u8> = vec![0; f.slot_count as usize];
        for (i, v) in args.iter().enumerate() {
            slots[i] = *v;
        }
        let _ = self.exec_block(&f.body, &mut slots)?;
        let mut out = Vec::with_capacity(f.sig.output_count as usize);
        for i in 0..f.sig.output_count {
            out.push(slots[(f.sig.input_count + i) as usize]);
        }
        Ok(out)
    }

    fn exec_block(
        &mut self,
        block: &HirBlock,
        slots: &mut Vec<u8>,
    ) -> Result<Flow, InterpError> {
        for op in &block.ops {
            match self.exec_op(op, slots)? {
                Flow::None => continue,
                other => return Ok(other),
            }
        }
        Ok(Flow::None)
    }

    fn exec_op(&mut self, op: &HirOp, slots: &mut Vec<u8>) -> Result<Flow, InterpError> {
        self.step_count += 1;
        if self.step_limit > 0 && self.step_count > self.step_limit {
            return Err(InterpError::StepLimit(self.step_count));
        }
        match op {
            HirOp::Set(s, v) => {
                slots[s.0 as usize] = *v;
            }
            HirOp::Copy(dst, src) => {
                slots[dst.0 as usize] = slots[src.0 as usize];
            }
            HirOp::Inc(s) => {
                let cur = slots[s.0 as usize];
                slots[s.0 as usize] = cur.wrapping_add(1) % 16;
            }
            HirOp::Dec(s) => {
                let cur = slots[s.0 as usize];
                slots[s.0 as usize] = cur.wrapping_sub(1) % 16;
            }
            HirOp::Loop(body) => loop {
                match self.exec_block(body, slots)? {
                    Flow::None | Flow::Continue => continue,
                    Flow::Break => return Ok(Flow::None),
                    other => return Ok(other),
                }
            },
            HirOp::Break => return Ok(Flow::Break),
            HirOp::Continue => return Ok(Flow::Continue),
            HirOp::Stop => return Ok(Flow::Stop),
            HirOp::Skip => return Ok(Flow::Skip),
            HirOp::ReadRegister(dst, reg) => {
                slots[dst.0 as usize] = self.registers[*reg as usize];
            }
            HirOp::WriteRegister(reg, src) => {
                let v = match src {
                    Either::Left(imm) => *imm,
                    Either::Right(s) => slots[s.0 as usize],
                };
                // Mirror MIR's special-cases on register 0 (command register).
                if *reg == 0 {
                    if v == 1 {
                        let high = self.registers[1];
                        let low = self.registers[2];
                        let byte = (high % 16) * 16 + (low % 16);
                        self.ctx.print(byte);
                    } else if v == 2 {
                        let byte = self.ctx.input();
                        self.registers[1] = byte / 16;
                        self.registers[2] = byte % 16;
                    }
                }
                self.registers[*reg as usize] = v;
            }
            HirOp::Block(b) => {
                match self.exec_block(b, slots)? {
                    Flow::Skip => return Ok(Flow::None),
                    other => return Ok(other),
                }
            }
            HirOp::Match(s, arms) => {
                let v = slots[s.0 as usize];
                for (arm_block, discs) in arms {
                    if discs.contains(&v) {
                        return self.exec_block(arm_block, slots);
                    }
                }
                // No arm matched: fall through (no behavior change).
            }
            HirOp::Call { target, args, ret } => {
                let key = match &target.trait_name {
                    Some(t) => typer::FnSig::new_trait(
                        &target.type_name,
                        &target.method_name,
                        t,
                    ),
                    None => typer::FnSig::new(&target.type_name, &target.method_name),
                };
                let callee = self.functions.get(&key).cloned().ok_or_else(|| {
                    InterpError::UnknownFunction(target.type_name.clone(), target.method_name.clone())
                })?;
                // Pack args.
                let mut callee_slots: Vec<u8> = vec![0; callee.slot_count as usize];
                if args.len() as u32 != callee.sig.input_count {
                    return Err(InterpError::Custom(format!(
                        "{}::{}: expected {} args, got {}",
                        target.type_name,
                        target.method_name,
                        callee.sig.input_count,
                        args.len()
                    )));
                }
                for (i, a) in args.iter().enumerate() {
                    callee_slots[i] = slots[a.0 as usize];
                }
                // Execute body.
                let _ = self.exec_block(&callee.body, &mut callee_slots)?;
                // Unpack returns.
                for (i, r) in ret.iter().enumerate() {
                    slots[r.0 as usize] =
                        callee_slots[(callee.sig.input_count + i as u32) as usize];
                }
            }
        }
        Ok(Flow::None)
    }
}
