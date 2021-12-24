use std::collections::{HashMap, HashSet};

use either::Either;

use crate::{block::MirCodeBlock, mir::Mir};

pub struct OptConfig {
    pub inline_loops: bool,
    pub inline_blocks: bool,
    pub reorganize_ifs: bool,
    pub remove_unused_vars: bool,
}

impl Default for OptConfig {
    fn default() -> Self {
        Self {
            inline_loops: false,
            inline_blocks: true,
            reorganize_ifs: true,
            remove_unused_vars: true,
        }
    }
}

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
            return Some(Mir::If0(*a, keep_block(b, reads), keep_block(c, reads)));
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

pub fn improve_code_flow(block: Vec<Mir>, opt: &OptConfig) -> Vec<Mir> {
    if !opt.reorganize_ifs {
        return block;
    }
    let mut out = Vec::new();
    let mut k = block.into_iter();
    while let Some(e) = k.next() {
        if let Mir::If0(a, b, c) = &e {
            if b.0.iter().any(|x| always_terminate(x)) {
                out.push(Mir::If0(
                    *a,
                    b.clone(),
                    MirCodeBlock(improve_code_flow(
                        c.0.iter().cloned().chain(k).collect(),
                        opt,
                    )),
                ));
                return out;
            } else if c.0.iter().any(|x| always_terminate(x)) {
                out.push(Mir::If0(
                    *a,
                    MirCodeBlock(improve_code_flow(
                        b.0.iter().cloned().chain(k).collect(),
                        opt,
                    )),
                    c.clone(),
                ));
                return out;
            }
        }
        out.push(e);
    }
    out
}

pub fn optimize_block(
    instruction: &MirCodeBlock,
    state: &mut OptimizerState,
    opt: &OptConfig,
) -> MirCodeBlock {
    MirCodeBlock(improve_code_flow(
        instruction
            .0
            .iter()
            .map(|x| optimize(x, state, opt))
            .flatten()
            .collect(),
        opt,
    ))
}

pub fn optimize(instruction: &Mir, state: &mut OptimizerState, opt: &OptConfig) -> Vec<Mir> {
    match &instruction {
        Mir::Set(a, b) => {
            state.set_var(*a, VarState::Values(vec![*b]));
        }
        Mir::Copy(a, b) => match state.get_var_value(*b) {
            Either::Left(x) => {
                state.set_var(*a, VarState::Values(vec![x]));
                return vec![Mir::Set(*a, x)];
            }
            Either::Right(x) => {
                state.set_var(*a, VarState::Ref(*b));
                return vec![Mir::Copy(*a, x)];
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
                        return optimize_block(b, state, opt).0;
                    }
                } else {
                    return optimize_block(c, state, opt).0;
                }
            }
            let mut state1 = state.clone();
            let mut state2 = state.clone();
            state1.remove_possibilities(*a, &(1..16).collect::<Vec<_>>());
            state2.remove_possibilities(*a, &[0]);
            let b1 = optimize_block(b, &mut state1, opt);
            let b2 = optimize_block(c, &mut state2, opt);
            let a = state.get_var_value(*a).right().unwrap();
            *state = state1.merge(&state2);
            return vec![Mir::If0(a, b1, b2)];
        }
        Mir::Loop(a) => {
            if opt.inline_loops {
                let mut state1 = state.clone();
                let (c, d) = try_unroll_loop(&mut state1, &a.0, opt);
                if c {
                    *state = state1;
                    //state.remove_vars(&get_muts_from_block(a));
                    return d;
                }
            }
            state.remove_vars(&get_muts_from_block(a));
            return vec![Mir::Loop(optimize_block(a, &mut state.clone(), opt))];
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
                        return vec![Mir::WriteRegister(*b, Either::Left(values[0]))];
                    }
                }
            }
        },
        Mir::Skip => (),
        Mir::Block(a) => {
            let k = optimize_block(a, state, opt);
            if opt.inline_blocks {
                let mut out = vec![];
                for i in &k.0 {
                    if does_skip_in_all_cases(i) {
                        if let Some(e) = remove_skips(i) {
                            out.push(e);
                        }
                        return out;
                    } else if contains_skip(i) {
                        break;
                    } else {
                        out.push(i.clone());
                    }
                }
            }
            //state.remove_vars(&get_muts_from_block(a));
            return vec![Mir::Block(k)];
        }
    };
    vec![instruction.clone()]
}

fn try_unroll_loop(state: &mut OptimizerState, lp: &[Mir], opt: &OptConfig) -> (bool, Vec<Mir>) {
    let mut o = vec![];
    'r: for _ in 0..16 {
        for i in lp {
            for i in optimize(i, state, opt) {
                if contains_continues(&i) || contains_skip(&i) {
                    break 'r;
                }
                o.push(remove_breaks(&i));
                if does_break_in_all_cases(&i) {
                    return (true, vec![Mir::Block(MirCodeBlock(o))]);
                }
            }
        }
    }
    (false, vec![Mir::Loop(MirCodeBlock(lp.to_vec()))])
}

fn remove_skips(mir: &Mir) -> Option<Mir> {
    match mir {
        Mir::If0(c, a, b) => Some(Mir::If0(
            *c,
            MirCodeBlock(a.0.iter().flat_map(|x| remove_skips(x)).collect()),
            MirCodeBlock(b.0.iter().flat_map(|x| remove_skips(x)).collect()),
        )),
        Mir::Skip => None,
        Mir::Loop(a) => Some(Mir::Block(MirCodeBlock(
            a.0.iter().flat_map(|x| remove_skips(x)).collect(),
        ))),
        e => Some(e.clone()),
    }
}

fn always_terminate(mir: &Mir) -> bool {
    match mir {
        Mir::If0(_, a, b) => {
            a.0.iter().any(|x| always_terminate(x)) && b.0.iter().any(|x| always_terminate(x))
        }
        Mir::Skip | Mir::Continue | Mir::Break | Mir::Stop => true,
        _ => false,
    }
}

fn remove_breaks(mir: &Mir) -> Mir {
    match mir {
        Mir::If0(c, a, b) => Mir::If0(
            *c,
            MirCodeBlock(a.0.iter().map(remove_breaks).collect()),
            MirCodeBlock(b.0.iter().map(remove_breaks).collect()),
        ),
        Mir::Break => Mir::Skip,
        Mir::Block(a) => Mir::Block(MirCodeBlock(a.0.iter().map(remove_breaks).collect())),
        e => e.clone(),
    }
}

fn contains_skip(mir: &Mir) -> bool {
    match mir {
        Mir::If0(_, a, b) => {
            a.0.iter().any(|x| contains_skip(x)) || b.0.iter().any(|x| contains_skip(x))
        }
        Mir::Skip => true,
        Mir::Loop(a) => a.0.iter().any(|x| contains_skip(x)),
        _ => false,
    }
}

fn contains_continues(mir: &Mir) -> bool {
    match mir {
        Mir::If0(_, a, b) => {
            a.0.iter().any(|x| contains_continues(x)) || b.0.iter().any(|x| contains_continues(x))
        }
        Mir::Continue => true,
        Mir::Block(a) => a.0.iter().any(|x| contains_continues(x)),
        _ => false,
    }
}

fn does_skip_in_all_cases(mir: &Mir) -> bool {
    match mir {
        Mir::If0(_, a, b) => {
            a.0.iter().any(|x| does_skip_in_all_cases(x))
                && b.0.iter().any(|x| does_skip_in_all_cases(x))
        }
        Mir::Continue => true,
        Mir::Break => true,
        Mir::Skip => true,
        Mir::Stop => true,
        _ => false,
    }
}

fn does_break_in_all_cases(mir: &Mir) -> bool {
    match mir {
        Mir::If0(_, a, b) => {
            a.0.iter().any(|x| does_break_in_all_cases(x))
                && b.0.iter().any(|x| does_break_in_all_cases(x))
        }
        Mir::Break => true,
        Mir::Stop => true,
        _ => false,
    }
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

    fn get_var_raw(&self, id: u32) -> VarState {
        self.variables
            .get(&id)
            .unwrap_or(&VarState::Unknown)
            .clone()
    }

    fn get_var(&self, id: u32) -> VarState {
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

    fn set_var(&mut self, id: u32, state: VarState) {
        self.variables
            .iter()
            .filter(|(id, x)| matches!(x, VarState::Ref(j) if j == *id))
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
