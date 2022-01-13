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
    Increment(Var),      // in, in is mutated
    Decrement(Var),      // in, in is mutated
    Jump(Label),         // Goto a label
    Label(Label),        // Defines a label
    If0(Var, Label),     // Jumps to the label if the thing is equals to 0
    Stop,
    Match(Var, [Option<Label>; 16]),
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
        std::fs::write("target/v3.ct", &ks);
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
            Self::Increment(a) => {
                Self::check_compile_var(a, template, ctx);
                template.add_code(Cow::Owned(format!("inc({})", a)))
            }
            Self::Decrement(a) => {
                Self::check_compile_var(a, template, ctx);
                template.add_code(Cow::Owned(format!("dec({})", a)))
            }
            Self::Jump(a) => template.add_code(Cow::Owned(format!("jump({})", a))),
            Self::Label(a) => template.add_code(Cow::Owned(format!("{}:no_op", a))),
            Self::If0(a, b) => {
                Self::check_compile_var(a, template, ctx);
                template.add_code(Cow::Owned(format!("if_0({} {})", a, b)))
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
            Self::Increment(a) => write!(f, "${}++", a.0,),
            Self::Decrement(a) => write!(f, "${}--", a.0,),
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
