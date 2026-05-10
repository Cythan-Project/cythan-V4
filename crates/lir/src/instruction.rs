use std::{borrow::Cow, collections::HashSet, fmt::Display};

use crate::{label::Label, number::Number, optimizer, value::AsmValue, var::Var, Counter};

use super::template::Template;

#[derive(Default)]
pub struct Context {
    variables: HashSet<usize>,
    counter: Counter,
}

#[derive(Debug, Clone)]
pub enum CompilableInstruction {
    Copy(Var, AsmValue), // to, from - from isn't mutated
    Jump(Label),         // Goto a label
    Label(Label),        // Defines a label
    If0(Var, Label),     // Jumps to the label if the thing is equals to 0
    Stop,
    Match(Var, [Option<Label>; 16]),
    /// `dst = table[src]`. Bytecode-level lookup primitive. For the
    /// canonical +1 / -1 tables this emits the tight `inc(src)` /
    /// `dec(src)` macros (~18 cells); for arbitrary tables it falls
    /// back to a 16-arm `Match` (~37 cells, grouped by output).
    Map(Var, Var, [u8; 16]),
    ReadRegister(Var, Number),
    WriteRegister(Number, AsmValue),
}

#[test]
fn test_v3() {
    let mut ctx = Context::default();
    let mut template = Template::default();
    let mut counter = crate::Counter::default();
    CompilableInstruction::Match(
        Var(10),
        [
            Some(Label::alloc(&mut counter, crate::LabelType::BlockEnd)),
            Some(Label::alloc(&mut counter, crate::LabelType::BlockEnd)),
            Some(Label::alloc(&mut counter, crate::LabelType::BlockEnd)),
            Some(Label::alloc(&mut counter, crate::LabelType::BlockEnd)),
            Some(Label::alloc(&mut counter, crate::LabelType::BlockEnd)),
            Some(Label::alloc(&mut counter, crate::LabelType::BlockEnd)),
            Some(Label::alloc(&mut counter, crate::LabelType::BlockEnd)),
            Some(Label::alloc(&mut counter, crate::LabelType::BlockEnd)),
            Some(Label::alloc(&mut counter, crate::LabelType::BlockEnd)),
            Some(Label::alloc(&mut counter, crate::LabelType::BlockEnd)),
            Some(Label::alloc(&mut counter, crate::LabelType::BlockEnd)),
            Some(Label::alloc(&mut counter, crate::LabelType::BlockEnd)),
            Some(Label::alloc(&mut counter, crate::LabelType::BlockEnd)),
            Some(Label::alloc(&mut counter, crate::LabelType::BlockEnd)),
            Some(Label::alloc(&mut counter, crate::LabelType::BlockEnd)),
            Some(Label::alloc(&mut counter, crate::LabelType::BlockEnd)),
        ],
    )
    .compile_inner(&mut template, &mut ctx);
    println!("{}", template.build());
}

impl CompilableInstruction {
    pub fn optimize(instrs: Vec<Self>) -> Vec<Self> {
        optimizer::opt_asm(instrs)
    }
    pub fn compile_to_string(instrs: Vec<Self>) -> String {
        let mut compile_state = Template::default();
        let mut ctx = Context::default();
        instrs
            .iter()
            .for_each(|i| i.compile_inner(&mut compile_state, &mut ctx));
        compile_state.build()
    }
    pub fn compile_to_binary(instrs: Vec<Self>) -> Vec<usize> {
        let ks = Self::compile_to_string(instrs);
        cythan_compiler::compile(&ks).unwrap()
    }
    fn check_compile_var(var: &Var, template: &mut Template, ctx: &mut Context) {
        if !ctx.variables.contains(&var.0) {
            ctx.variables.insert(var.0);
            template.add_section("VAR_DEF", Cow::Owned(format!("{}:0", var)));
        }
    }

    fn compile_inner(&self, template: &mut Template, ctx: &mut Context) {
        match self {
            Self::Copy(a, b) => {
                Self::check_compile_var(a, template, ctx);
                match b {
                    AsmValue::Var(b) => {
                        Self::check_compile_var(b, template, ctx);
                        template.add_code(Cow::Owned(format!("{} {}", b, a)));
                    }
                    AsmValue::Number(b) => {
                        template.add_code(Cow::Owned(format!("'#{} {}", b.0, a)));
                    }
                }
            }
            Self::Jump(a) => template.add_code(Cow::Owned(format!("jump({})", a))),
            Self::Label(a) => template.add_code(Cow::Owned(format!("{}:no_op", a))),
            Self::If0(a, b) => {
                Self::check_compile_var(a, template, ctx);
                template.add_code(Cow::Owned(format!("if_0({} {})", a, b)))
            }
            Self::Map(src, dst, table) => {
                // Tight 18-cell emission for any 1-cell→1-cell
                // lookup table. Generalises the inc/dec macros
                // (which were special-cased ±1 cycles) by writing
                // each output value directly to the cell whose
                // address equals its input — then a final indirect
                // copy through `'test` reads the slot off in O(1).
                //
                // Per-input `(value, target_cell)` pair:
                //   * value  = `'#T[X]`, which the cythan_compiler
                //              substitutes with `T[X]` (or 16 when
                //              `T[X] == 0`, since `'#0` is defined
                //              as 16 in the program header).
                //   * target = `X`, with `X == 0` mapped to cell 16
                //              so the cell-0 PC isn't clobbered.
                Self::check_compile_var(dst, template, ctx);
                if src.0 != dst.0 {
                    Self::check_compile_var(src, template, ctx);
                    template.add_code(Cow::Owned(format!("{} {}", src, dst)));
                }
                let k = ctx.counter.count();
                let mut body = format!("{} 'test_map_{k}", dst);
                for x in 0u8..=15 {
                    let target = if x == 0 { 16 } else { x as usize };
                    body.push_str(&format!("\n'#{} {}", table[x as usize], target));
                }
                body.push_str(&format!("\n'test_map_{k}:earasable {}", dst));
                template.add_code(Cow::Owned(body));
            }
            Self::Match(a, b) => {
                let k = ctx.counter.count();
                Self::check_compile_var(a, template, ctx);
                template.add_code(Cow::Owned(format!(
                    "{a} 'test_{k} \n{}\n'test_{k}:earasable 0\njump('end1_{k})\n{}\n'end_{k}:~+1\n'end1_{k}:no_op",
                    b.iter()
                        .enumerate()
                        .map(|(i, x)| match x {
                            Some(_) => format!("'pt{}_{k} {}", i, if i == 0 { 16 } else { i }),
                            None => format!("'end_{k} {}", i),
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                    b.iter()
                        .enumerate()
                        .map(|(i, x)| match x {
                            Some(x) => format!("'pt{}_{k}:{}", i, x),
                            None => format!("'pt{}_{k}:'end_{k}", i),
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                )))
            }
            Self::Stop => template.add_code(Cow::Borrowed("stop")),
            Self::ReadRegister(a, b) => {
                template.add_code(Cow::Owned(format!("'#return_{} {}", b.0, a)));
            }
            Self::WriteRegister(a, b) => match b {
                AsmValue::Var(b) => {
                    Self::check_compile_var(b, template, ctx);
                    template.add_code(Cow::Owned(format!("{} '#return_{}", b, a.0)));
                }
                AsmValue::Number(b) => {
                    template.add_code(Cow::Owned(format!("'#{} '#return_{}", b.0, a.0)));
                }
            },
        }
    }
}

impl Display for CompilableInstruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Copy(a, b) => write!(
                f,
                "${} = {}",
                a.0,
                match b {
                    AsmValue::Var(a) => format!("${}", a.0),
                    AsmValue::Number(a) => a.0.to_string(),
                }
            ),
            Self::Map(src, dst, table) => {
                write!(f, "${} = map(${}, {:?})", dst.0, src.0, table)
            }
            Self::Jump(a) => write!(f, "jmp {}", a),
            Self::Label(a) => write!(f, "{}", a),
            Self::If0(a, b) => write!(f, "if ${} {}", a.0, b),
            Self::Match(a, b) => write!(
                f,
                "match ${} ({})",
                a.0,
                b.iter()
                    .enumerate()
                    .map(|(i, x)| format!(
                        "{}={}",
                        i,
                        x.as_ref().map(|x| x.to_string()).unwrap_or_default()
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Stop => write!(f, "stop"),
            Self::ReadRegister(a, b) => write!(f, "${} = @{}", a.0, b.0),
            Self::WriteRegister(a, b) => write!(
                f,
                "@{} = {}",
                a.0,
                match b {
                    AsmValue::Var(a) => format!("${}", a.0),
                    AsmValue::Number(a) => a.0.to_string(),
                }
            ),
        }
    }
}
