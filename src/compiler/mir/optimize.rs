use std::collections::{HashMap, HashSet};

use either::Either;

use crate::parser::expression::CodeBlock;

use super::{Mir, MirCodeBlock};

pub fn get_reads_from_block(mir: &MirCodeBlock) -> HashSet<u32> {
    let mut set = HashSet::new();
    mir.0.iter().for_each(|x| get_reads(x, &mut set));
    set
}

fn get_reads(mir: &Mir, muts: &mut HashSet<u32>) {
    match mir {
        Mir::Copy(_, a) | Mir::WriteRegister(_, Either::Right(a)) => {
            muts.insert(*a);
        }
        Mir::If0(c, a, b) => {
            muts.insert(*c);
            a.0.iter().for_each(|x| get_reads(x, muts));
            b.0.iter().for_each(|x| get_reads(x, muts));
        }
        Mir::Loop(a) | Mir::Block(a) => {
            a.0.iter().for_each(|x| get_reads(x, muts));
        }
        Mir::Break => (),
        Mir::Continue => (),
        Mir::Stop => (),
        Mir::Skip => (),
        Mir::Set(_, _) => (),
        Mir::Increment(_) => (),
        Mir::Decrement(_) => (),
        Mir::ReadRegister(_, _) => (),
        Mir::WriteRegister(_, Either::Left(_)) => (),
    }
}

pub fn keep_block(mir: &MirCodeBlock, reads: &mut HashSet<u32>) -> MirCodeBlock {
    let mut old1: Option<Mir> = None;
    let mut out = Vec::new();
    for i in mir.0.iter().flat_map(|x| keep(x, reads)) {
        if let Some(old) = old1 {
            if let Mir::Copy(a, _) | Mir::Set(a, _) = &i {
                if let Mir::Set(c, _) | Mir::Copy(c, _) = &old {
                    if c == a {
                        old1 = Some(i);
                        continue;
                    }
                }
            }
            out.push(old);
        }
        old1 = Some(i);
    }
    if let Some(old) = old1 {
        out.push(old);
    }
    MirCodeBlock(out)
}
fn keep(mir: &Mir, reads: &mut HashSet<u32>) -> Option<Mir> {
    match &mir {
        Mir::Set(a, _)
        | Mir::Copy(a, _)
        | Mir::Increment(a)
        | Mir::Decrement(a)
        | Mir::ReadRegister(a, _) => {
            if !reads.contains(a) {
                return None;
            }
        }
        Mir::If0(a, b, c) => {
            return Some(Mir::If0(
                a.clone(),
                keep_block(b, reads),
                keep_block(c, reads),
            ));
        }
        Mir::Loop(a) => return Some(Mir::Loop(keep_block(a, reads))),
        Mir::Break => (),
        Mir::Continue => (),
        Mir::Stop => (),
        Mir::WriteRegister(_, _) => (),
        Mir::Skip => (),
        Mir::Block(a) => return Some(Mir::Block(keep_block(a, reads))),
    }
    Some(mir.clone())
}

fn get_muts_from_block(mir: &MirCodeBlock) -> HashSet<u32> {
    let mut set = HashSet::new();
    mir.0.iter().for_each(|x| get_muts(x, &mut set));
    set
}

fn get_muts(mir: &Mir, muts: &mut HashSet<u32>) {
    match mir {
        Mir::Set(a, _)
        | Mir::Copy(a, _)
        | Mir::ReadRegister(a, _)
        | Mir::Increment(a)
        | Mir::Decrement(a) => {
            muts.insert(*a);
        }
        Mir::If0(_, a, b) => {
            a.0.iter().for_each(|x| get_muts(x, muts));
            b.0.iter().for_each(|x| get_muts(x, muts));
        }
        Mir::Loop(a) | Mir::Block(a) => {
            a.0.iter().for_each(|x| get_muts(x, muts));
        }
        Mir::Break => (),
        Mir::Continue => (),
        Mir::Stop => (),
        Mir::WriteRegister(_, _) => (),
        Mir::Skip => (),
    }
}

pub fn optimize_block(instruction: &MirCodeBlock, state: &mut OptimizerState) -> MirCodeBlock {
    MirCodeBlock(
        instruction
            .0
            .iter()
            .map(|x| optimize(x, state))
            .flatten()
            .collect(),
    )
}

pub fn optimize(instruction: &Mir, state: &mut OptimizerState) -> Vec<Mir> {
    match &instruction {
        Mir::Set(a, b) => {
            state.set_var(*a, VarState::Values(vec![*b]));
        }
        Mir::Copy(a, b) => {
            state.set_var(*a, VarState::Ref(*b));
            match state.get_var_value(*b) {
                Either::Left(x) => {
                    return vec![Mir::Set(*a, x)];
                }
                Either::Right(x) => {
                    return vec![Mir::Copy(*a, x)];
                }
            }
        }
        Mir::Increment(a) => state.set_var(
            *a,
            match state.get_var(*a) {
                VarState::Values(values) => {
                    VarState::Values(values.into_iter().map(|x| (x + 1) % 16).collect())
                }
                e => e,
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
                e => e,
            },
        ),
        Mir::If0(a, b, c) => {
            if let VarState::Values(values) = state.get_var(*a) {
                if values.contains(&0) {
                    if values.len() == 1 {
                        return optimize_block(b, state).0;
                    }
                } else {
                    return optimize_block(c, state).0;
                }
            }
            let mut state1 = state.clone();
            let mut state2 = state.clone();
            state1.remove_possibilities(*a, &(1..16).collect::<Vec<_>>());
            state2.remove_possibilities(*a, &[0]);
            let b1 = optimize_block(b, &mut state1);
            let b2 = optimize_block(c, &mut state2);
            let a = state.get_var_value(*a).right().unwrap();
            *state = state1.merge(&state2);
            return vec![Mir::If0(a, b1, b2)];
        }
        Mir::Loop(a) => {
            state.remove_vars(&get_muts_from_block(a));
            return vec![Mir::Loop(optimize_block(a, &mut state.clone()))];
        }
        Mir::Break => (),
        Mir::Continue => (),
        Mir::Stop => (),
        Mir::ReadRegister(a, _) => {
            state.set_var(*a, VarState::Unknown);
        }
        Mir::WriteRegister(b, a) => match a {
            Either::Left(_) => (),
            Either::Right(a) => match state.get_var(*a) {
                VarState::Values(values) => {
                    if values.len() == 1 {
                        return vec![Mir::WriteRegister(*b, Either::Left(values[0]))];
                    }
                }
                _ => (),
            },
        },
        Mir::Skip => (),
        Mir::Block(a) => {
            let k = optimize_block(a, state);
            state.remove_vars(&get_muts_from_block(a));
            return vec![Mir::Block(k)];
        }
    };
    vec![instruction.clone()]
}

#[derive(Clone)]
pub struct OptimizerState {
    variables: HashMap<u32, VarState>,
}

impl OptimizerState {
    pub fn new() -> Self {
        OptimizerState {
            variables: HashMap::new(),
        }
    }

    pub fn get_var_raw(&self, id: u32) -> VarState {
        self.variables
            .get(&id)
            .unwrap_or(&VarState::Unknown)
            .clone()
    }

    pub fn get_var(&self, id: u32) -> VarState {
        match self.get_var_raw(id) {
            VarState::Ref(a) => self.get_var(a),
            e => e,
        }
    }

    pub fn get_var_value(&self, id: u32) -> Either<u8, u32> {
        match self.get_var_raw(id) {
            VarState::Ref(a) => self.get_var_value(a),
            VarState::Values(a) => {
                if a.len() == 1 {
                    Either::Left(a[0])
                } else {
                    Either::Right(id)
                }
            }
            VarState::Unknown => Either::Right(id),
        }
    }

    pub fn remove_vars(&mut self, ids: &HashSet<u32>) {
        for id in ids {
            self.variables.remove(id);
        }
    }

    pub fn remove_possibilities(&mut self, id: u32, pos: &[u8]) {
        self.set_var(
            id,
            VarState::Values(match self.get_var(id) {
                VarState::Ref(_) => unreachable!(),
                VarState::Values(a) => a.into_iter().filter(|&x| !pos.contains(&x)).collect(),
                VarState::Unknown => (0..16).filter(|&x| !pos.contains(&x)).collect(),
            }),
        )
    }

    pub fn set_var(&mut self, id: u32, state: VarState) {
        self.variables
            .iter()
            .filter(|(id, x)| match x {
                VarState::Ref(j) if j == *id => true,
                _ => false,
            })
            .map(|x| *x.0)
            .collect::<Vec<u32>>()
            .into_iter()
            .for_each(|x| self.set_var(x, self.get_var(id)));

        self.variables.insert(id, state);
    }

    pub fn merge(&self, other: &Self) -> Self {
        let mut variables = HashMap::new();
        for (id, state) in other.variables.iter() {
            if self.variables.get(id) == Some(state) {
                variables.insert(*id, state.clone());
            }
        }
        OptimizerState { variables }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum VarState {
    Ref(u32),
    Values(Vec<u8>),
    Unknown,
}
