use std::{
    collections::{HashMap, HashSet},
    iter::once,
};

use either::Either;

use crate::compiler::mir::{Mir, MirCodeBlock};

#[derive(Debug, Clone)]
struct State {
    variables: HashMap<u32, Variable>,
}

impl State {
    fn new() -> Self {
        Self {
            variables: HashMap::new(),
        }
    }
    fn set(&mut self, id: u32, value: Variable) {
        let k: Vec<u32> = self
            .variables
            .iter()
            .filter(|(_, x)| {
                if let Variable::Reference(e) = x {
                    *e == id
                } else {
                    false
                }
            })
            .map(|x| *x.0)
            .collect();
        // TODO: Add instead the value before modification.
        k.iter().for_each(|x| {
            self.set(*x, Variable::Unknown);
        });
        self.variables.insert(id, value);
    }

    fn get(&self, id: u32) -> Variable {
        match self.get_raw(id) {
            Variable::Reference(a) => self.get(a),
            e => e,
        }
    }

    fn get_as_value(&self, id: u32) -> Either<u32, u8> {
        match self.get_raw(id) {
            Variable::Value(a) => Either::Right(a),
            Variable::Reference(a) => self.get_as_value(a),
            Variable::Unknown => Either::Left(id),
        }
    }

    fn remove(&mut self, vars: &[u32]) {
        vars.iter().for_each(|x| {
            self.variables.remove(x);
        });
    }

    fn get_raw(&self, id: u32) -> Variable {
        self.variables
            .get(&id)
            .cloned()
            .unwrap_or(Variable::Unknown)
    }

    fn merge(&self, other: &State) -> State {
        let mut new_state = Self::new();
        for (k, v) in self.variables.iter() {
            if let Some(v1) = other.variables.get(k) {
                if v == v1 {
                    new_state.variables.insert(*k, v.clone());
                }
            }
        }
        new_state
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Variable {
    Value(u8),
    Reference(u32),
    Unknown,
}

pub fn opt(block: &MirCodeBlock) -> MirCodeBlock {
    let mut state = State::new();
    optimize_block(block, &mut state)
}

fn optimize_block(block: &MirCodeBlock, state: &mut State) -> MirCodeBlock {
    MirCodeBlock(
        block
            .0
            .iter()
            .map(|x| optimize(x, state))
            .flatten()
            .collect(),
    )
}

fn optimize(instruction: &Mir, state: &mut State) -> Vec<Mir> {
    vec![match instruction {
        Mir::Set(a, b) => {
            state.set(*a, Variable::Value(*b));
            Mir::Set(*a, *b)
        }
        Mir::Copy(a, b) => match state.get_as_value(*b) {
            Either::Left(b) => {
                state.set(*a, Variable::Reference(b));
                Mir::Copy(*a, b)
            }
            Either::Right(b) => {
                return optimize(&Mir::Set(*a, b), state);
            }
        },
        Mir::Increment(a) => {
            if let Variable::Value(b) = state.get(*a) {
                let b = b + 1;
                return optimize(&Mir::Set(*a, b), state);
            } else {
                state.set(*a, Variable::Unknown);
                Mir::Increment(*a)
            }
        }
        Mir::Decrement(a) => {
            if let Variable::Value(b) = state.get(*a) {
                let b = if b == 0 { 15 } else { b - 1 };
                return optimize(&Mir::Set(*a, b), state);
            } else {
                state.set(*a, Variable::Unknown);
                Mir::Decrement(*a)
            }
        }
        Mir::If0(a, b, c) => {
            if let Variable::Value(d) = state.get(*a) {
                if d == 0 {
                    return optimize_block(b, state).0;
                } else {
                    return optimize_block(c, state).0;
                }
            } else {
                let mut state1 = state.clone();
                let mut state2 = state.clone();
                let b = optimize_block(b, &mut state1);
                let c = optimize_block(c, &mut state2);
                *state = state1.merge(&state2);
                Mir::If0(*a, b, c)
            }
        }
        Mir::Loop(a) => {
            state.remove(&get_muts_from_block(&a));
            Mir::Loop(optimize_block(a, &mut state.clone()))
        }
        Mir::Break => Mir::Break,
        Mir::Continue => Mir::Continue,
        Mir::Stop => Mir::Stop,
        Mir::ReadRegister(a, b) => {
            state.set(*a, Variable::Unknown);
            Mir::ReadRegister(*a, *b)
        }
        Mir::WriteRegister(a, b) => Mir::WriteRegister(
            *a,
            match b {
                Either::Left(b) => Either::Left(*b),
                Either::Right(b) => state.get_as_value(*b).flip(),
            },
        ),
        Mir::Skip => Mir::Skip,
        Mir::Block(a) => {
            state.remove(&get_muts_from_block(&a));
            Mir::Block(optimize_block(a, &mut state.clone()))
        }
    }]
}

fn get_muts_from_block(block: &MirCodeBlock) -> Vec<u32> {
    block.0.iter().flat_map(|x| get_muts(x)).collect()
}

fn get_muts(block: &Mir) -> Vec<u32> {
    match block {
        Mir::Block(a) | Mir::Loop(a) => get_muts_from_block(a),
        Mir::ReadRegister(a, _)
        | Mir::Decrement(a)
        | Mir::Increment(a)
        | Mir::Set(a, _)
        | Mir::Copy(a, _) => vec![*a],
        Mir::If0(_, a, b) => get_muts_from_block(a)
            .into_iter()
            .chain(get_muts_from_block(b).into_iter())
            .collect(),
        Mir::Break | Mir::Continue | Mir::Stop | Mir::WriteRegister(_, _) | Mir::Skip => vec![],
    }
}

fn get_reads_from_block(block: &MirCodeBlock) -> Vec<u32> {
    block.0.iter().flat_map(|x| get_reads(x)).collect()
}

fn get_reads(block: &Mir) -> Vec<u32> {
    match block {
        Mir::Block(a) | Mir::Loop(a) => get_reads_from_block(a),
        Mir::Copy(_, a) | Mir::WriteRegister(_, Either::Right(a)) => vec![*a],
        Mir::If0(c, a, b) => get_reads_from_block(a)
            .into_iter()
            .chain(get_reads_from_block(b).into_iter())
            .chain(once(*c))
            .collect(),
        Mir::Break
        | Mir::Continue
        | Mir::Stop
        | Mir::Skip
        | Mir::Set(_, _)
        | Mir::Increment(_)
        | Mir::Decrement(_)
        | Mir::WriteRegister(_, Either::Left(_))
        | Mir::ReadRegister(_, _) => vec![],
    }
}

pub fn remove_unused_vars(block: &MirCodeBlock) -> MirCodeBlock {
    let mut needed = get_reads_from_block(block)
        .into_iter()
        .collect::<HashSet<u32>>();

    MirCodeBlock(
        block
            .0
            .iter()
            .flat_map(|x| fix_mir(x, &mut needed))
            .collect(),
    )
}

pub fn fix_mir(mir: &Mir, set: &HashSet<u32>) -> Option<Mir> {
    match mir {
        Mir::Set(a, _)
        | Mir::Copy(a, _)
        | Mir::Increment(a)
        | Mir::Decrement(a)
        | Mir::ReadRegister(a, _) => {
            if !set.contains(a) {
                return None;
            }
        }
        Mir::If0(a, b, c) => {
            return Some(Mir::If0(
                *a,
                MirCodeBlock(b.0.iter().flat_map(|x| fix_mir(x, set)).collect()),
                MirCodeBlock(c.0.iter().flat_map(|x| fix_mir(x, set)).collect()),
            ));
        }
        Mir::Loop(a) => {
            return Some(Mir::Loop(MirCodeBlock(
                a.0.iter().flat_map(|x| fix_mir(x, set)).collect(),
            )));
        }
        Mir::WriteRegister(_, _) => (),
        Mir::Block(a) => {
            return Some(Mir::Block(MirCodeBlock(
                a.0.iter().flat_map(|x| fix_mir(x, set)).collect(),
            )));
        }
        Mir::Break => (),
        Mir::Continue => (),
        Mir::Stop => (),
        Mir::Skip => (),
    }
    Some(mir.clone())
}

pub fn remove_unused(block: &MirCodeBlock) -> MirCodeBlock {
    let mut needed = HashSet::new();
    remove_in_block(block, &mut needed)
}

fn remove_in_block(mir: &MirCodeBlock, needed: &mut HashSet<u32>) -> MirCodeBlock {
    MirCodeBlock(
        mir.0
            .iter()
            .rev()
            .collect::<Vec<_>>()
            .iter()
            .map(|x| {
                /* println!("========================",);
                println!("{}", x);
                println!("{:?}", needed); */
                let out = flatten(x, needed);
                /* println!("{:?}", needed);
                if let Some(_) = &out {
                    println!("Element kept");
                } else {
                    println!("Element removed");
                }
                println!("========================"); */
                out
            })
            .flatten()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect(),
    )
}

fn flatten(mir: &Mir, needed: &mut HashSet<u32>) -> Option<Mir> {
    match mir {
        Mir::Set(a, _) => {
            if !needed.remove(a) {
                println!("Removing {}", mir);
                return None;
            }
        }
        Mir::Copy(a, b) => {
            if !needed.remove(a) {
                println!("Removing {}", mir);
                return None;
            }
            needed.insert(*b);
        }
        Mir::Increment(a) | Mir::Decrement(a) => {
            if !needed.contains(a) {
                println!("Removing {}", mir);
                return None;
            }
        }
        Mir::If0(a, b, c) => {
            let mut needed1 = needed.clone();
            let mut needed2 = needed.clone();

            let k1 = remove_in_block(b, &mut needed1);
            let k2 = remove_in_block(c, &mut needed2);

            *needed = needed1.union(&needed2).copied().collect::<HashSet<_>>();
            needed.insert(*a);
            return Some(Mir::If0(*a, k1, k2));
        }
        Mir::Loop(a) => {
            let mut needed1 = needed.clone();
            needed1.extend(get_reads_from_block(a).into_iter());
            let tmp = Mir::Loop(remove_in_block(a, &mut needed1));
            *needed = needed.union(&needed1).copied().collect::<HashSet<_>>();
            return Some(tmp);
        }
        Mir::Break => (),
        Mir::Continue => (),
        Mir::Stop => (),
        Mir::ReadRegister(a, _) => {
            if !needed.remove(a) {
                println!("Removing {}", mir);
                return None;
            }
        }
        Mir::WriteRegister(_, a) => {
            if let Either::Right(a) = a {
                needed.insert(*a);
            }
        }
        Mir::Skip => (),
        Mir::Block(a) => {
            return Some(Mir::Block(remove_in_block(a, needed)));
        }
    }
    Some(mir.clone())
}
