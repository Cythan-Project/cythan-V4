use either::Either;

use crate::optimizer::old::{contains_skip, does_skip_in_all_cases, remove_skips};
use crate::optimizer::state::{OptimizerState, VarState};
use crate::{
    optimizer::old::{improve_code_flow, OptConfig},
    Mir, MirCodeBlock,
};

pub trait Optimize {
    fn optimize(&self, state: &mut OptimizerState, opt: &OptConfig) -> MirCodeBlock;
}
/*
impl Optimize for MirCodeBlock {
    fn optimize(&self, state: &mut OptimizerState, opt: &OptConfig) -> MirCodeBlock {
        MirCodeBlock(improve_code_flow(
            self.iter()
                .map(|x| x.optimize(state, opt))
                .flatten()
                .collect(),
            opt,
        ))
    }
} */
/*
impl Optimize for Mir {
    fn optimize(&self, state: &mut OptimizerState, opt: &OptConfig) -> MirCodeBlock {
        match &self {
            Mir::Set(a, b) => {
                state.set_var(*a, VarState::Values(vec![*b]));
            }
            Mir::Copy(a, b) => match state.get_var_value(*b) {
                Either::Left(x) => {
                    state.set_var(*a, VarState::Values(vec![x]));
                    return Mir::Set(*a, x).into();
                }
                Either::Right(x) => {
                    state.set_var(*a, VarState::Ref(*b));
                    return Mir::Copy(*a, x).into();
                }
            },
            Mir::Increment(a) => state.set_var(
                *a,
                match state.get_var(*a) {
                    VarState::Values(values) => {
                        VarState::Values(values.into_iter().map(|x| (x + 1) % 16).collect())
                    }
                    _ => VarState::Unknown,
                },
            ),
            Mir::Decrement(a) => state.set_var(
                *a,
                match state.get_var(*a) {
                    VarState::Values(values) => VarState::Values(
                        values
                            .into_iter()
                            .map(|x| if x == 0 { 15 } else { x - 1 })
                            .collect(),
                    ),
                    _ => VarState::Unknown,
                },
            ),
            Mir::If0(a, b, c) => {
                if let VarState::Values(values) = state.get_var(*a) {
                    if values.contains(&0) {
                        if values.len() == 1 {
                            return b.optimize(state, opt);
                        }
                    } else {
                        return c.optimize(state, opt);
                    }
                }
                let mut state1 = state.clone();
                let mut state2 = state.clone();
                state1.remove_possibilities(*a, &(1..16).collect::<Vec<_>>());
                state2.remove_possibilities(*a, &[0]);
                let b1 = b.optimize(&mut state1, opt);
                let b2 = c.optimize(&mut state2, opt);
                let a = state.get_var_value(*a).right().unwrap();
                *state = state1.merge(&state2);
                return Mir::If0(a, b1, b2).into();
            }
            Mir::Loop(a) => {
                if opt.inline_loops {
                    let mut state1 = state.clone();
                    let (c, d) = try_unroll_loop(&mut state1, a, opt);
                    if c {
                        *state = state1;
                        //state.remove_vars(&get_muts_from_block(a));
                        return d;
                    }
                }
                state.remove_vars(&a.get_writes());
                return Mir::Loop(a.optimize(&mut state.clone(), opt)).into();
            }
            Mir::Break => (),
            Mir::Continue => (),
            Mir::Stop => (),
            Mir::ReadRegister(a, _) => {
                state.set_var(*a, VarState::Unknown);
            }
            Mir::WriteRegister(b, a) => match a {
                Either::Left(_) => (),
                Either::Right(a) => {
                    if let VarState::Values(values) = state.get_var(*a) {
                        if values.len() == 1 {
                            return Mir::WriteRegister(*b, Either::Left(values[0])).into();
                        }
                    }
                }
            },
            Mir::Skip => (),
            Mir::Block(a) => {
                let k = a.optimize(state, opt);
                if opt.inline_blocks {
                    let mut out = vec![];
                    for i in &k.0 {
                        if does_skip_in_all_cases(i) {
                            if let Some(e) = remove_skips(i) {
                                out.push(e);
                            }
                            return MirCodeBlock(out);
                        } else if contains_skip(i) {
                            break;
                        } else {
                            out.push(i.clone());
                        }
                    }
                }
                //state.remove_vars(&get_muts_from_block(a));
                return Mir::Block(k).into();
            }
        };
        self.clone().into()
    }
}
 */
