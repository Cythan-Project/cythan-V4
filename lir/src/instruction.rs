use std::{borrow::Cow, collections::HashSet, fmt::Display};

use crate::{label::Label, number::Number, optimizer, value::AsmValue, var::Var};

use super::template::Template;

#[derive(Default)]
pub struct Context {
    variables: HashSet<usize>,
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
    ReadRegister(Var, Number),
    WriteRegister(Number, AsmValue),
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
        cythan_compiler::compile(&Self::compile_to_string(instrs)).unwrap()
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
