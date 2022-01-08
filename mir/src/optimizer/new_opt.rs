use std::{
    collections::{HashMap, HashSet},
    ops::RangeBounds,
};

// 7715

use either::Either;

use crate::{Mir, MirCodeBlock};

#[derive(Clone)]
struct OptContext {
    variables: HashMap<u32, VariableStatus>,
}

impl OptContext {
    fn new() -> Self {
        OptContext {
            variables: HashMap::new(),
        }
    }

    fn remove(&mut self, id: u32) {
        let k: Vec<u32> = self
            .variables
            .iter()
            .filter(|(_, v)| match v {
                VariableStatus::Ref(a) => *a == id,
                _ => false,
            })
            .map(|(a, _)| *a)
            .collect();
        k.iter().for_each(|x| {
            self.remove(*x);
        });
        self.variables.remove(&id);
    }

    fn set_or_remove(&mut self, id: u32, status: Option<VariableStatus>) {
        if let Some(e) = status {
            self.set(id, e);
        } else {
            self.remove(id);
        }
    }

    fn get_raw(&mut self, id: u32) -> Option<VariableStatus> {
        self.variables.get(&id).copied()
    }

    fn get_value(&mut self, id: u32) -> Option<u8> {
        match self.get_raw(id) {
            Some(VariableStatus::Ref(a)) => self.get_value(a),
            Some(VariableStatus::Value(a)) => Some(a),
            None => None,
        }
    }

    fn get_flatten(&mut self, id: u32) -> Option<VariableStatus> {
        match self.get_raw(id) {
            Some(VariableStatus::Ref(a)) => Some(match self.get_flatten(a) {
                Some(a) => a,
                None => VariableStatus::Ref(a),
            }),
            e => e,
        }
    }

    fn set(&mut self, id: u32, status: VariableStatus) {
        let k: Vec<u32> = self
            .variables
            .iter()
            .filter(|(_, v)| match v {
                VariableStatus::Ref(a) => *a == id,
                _ => false,
            })
            .map(|(a, _)| *a)
            .collect();
        k.iter().for_each(|x| {
            self.remove(*x);
        });
        self.variables.insert(id, status);
    }

    fn merge(&self, other: &Self) -> Self {
        let mut variables = HashMap::new();
        for (k, v) in other.variables.iter() {
            if let Some(e) = self.variables.get(k) {
                if e == v {
                    variables.insert(*k, *v);
                }
            }
        }
        OptContext { variables }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum VariableStatus {
    Value(u8),
    Ref(u32),
}

pub fn optimize_code(mir: MirCodeBlock) -> MirCodeBlock {
    let mut context = OptContext::new();
    let k = MirCodeBlock(optimize_block(mir, &mut context));
    let j = k.get_reads();
    remove_unread(k, &j)
}

fn optimize_block(mir: MirCodeBlock, context: &mut OptContext) -> Vec<Mir> {
    mir.0
        .into_iter()
        .map(|x| optimize(x, context))
        .flatten()
        .collect()
}

fn optimize(mir: Mir, context: &mut OptContext) -> Vec<Mir> {
    match mir {
        Mir::Set(a, b) => {
            context.set(a, VariableStatus::Value(b));
            vec![Mir::Set(a, b)]
        }
        Mir::Copy(a, b) => {
            let flt = context.get_flatten(b);
            if let Some(c) = flt {
                context.set(a, c);
                return vec![match c {
                    VariableStatus::Value(c) => Mir::Set(a, c),
                    VariableStatus::Ref(c) => Mir::Copy(a, c),
                }];
            } else {
                context.set(a, VariableStatus::Ref(b));
            }
            vec![Mir::Copy(a, b)]
        }
        Mir::Increment(a) => {
            if let Some(e) = context.get_value(a) {
                context.set(a, VariableStatus::Value((e + 1) % 16));
            } else {
                context.remove(a);
            }
            vec![Mir::Increment(a)]
        }
        Mir::Decrement(a) => {
            if let Some(e) = context.get_value(a) {
                context.set(a, VariableStatus::Value(e.wrapping_sub(1) % 16));
            } else {
                context.remove(a);
            }

            vec![Mir::Decrement(a)]
        }
        Mir::If0(a, b, c) => {
            if let Some(e) = context.get_value(a) {
                if e == 0 {
                    return optimize_block(b, context);
                } else {
                    return optimize_block(c, context);
                }
            } else {
                let mut cb = context.clone();
                let mut cc = context.clone();
                let b = MirCodeBlock(optimize_block(b, &mut cb));
                let c = MirCodeBlock(optimize_block(c, &mut cc));
                *context = cb.merge(&cc);
                vec![Mir::If0(a, b, c)]
            }
        }
        Mir::Loop(a) => {
            for w in a.get_writes() {
                context.remove(w);
            }
            let a = MirCodeBlock(optimize_block(a, &mut context.clone()));
            vec![Mir::Loop(a)]
        }
        Mir::Break => vec![Mir::Break],
        Mir::Continue => vec![Mir::Continue],
        Mir::Stop => vec![Mir::Stop],
        Mir::ReadRegister(a, b) => {
            context.remove(a);
            vec![Mir::ReadRegister(a, b)]
        }
        Mir::WriteRegister(a, b) => match b {
            Either::Left(c) => vec![Mir::WriteRegister(a, Either::Left(c))],
            Either::Right(c) => {
                if let Some(e) = context.get_value(c) {
                    vec![Mir::WriteRegister(a, Either::Left(e))]
                } else {
                    vec![Mir::WriteRegister(a, Either::Right(c))]
                }
            }
        },
        Mir::Skip => vec![Mir::Skip],
        Mir::Block(a) => {
            let mut cb = context.clone();
            let a = MirCodeBlock(optimize_block(a, &mut cb));
            vec![Mir::Block(a)]
        }
    }
}

fn remove_unread(codeblock: MirCodeBlock, reads: &HashSet<u32>) -> MirCodeBlock {
    MirCodeBlock(
        codeblock
            .0
            .into_iter()
            .flat_map(|x| {
                match &x {
                    Mir::Set(a, _) => {
                        if !reads.contains(a) {
                            return None;
                        }
                    }
                    Mir::Copy(a, _) => {
                        if !reads.contains(a) {
                            return None;
                        }
                    }
                    Mir::Increment(a) => {
                        if !reads.contains(a) {
                            return None;
                        }
                    }
                    Mir::Decrement(a) => {
                        if !reads.contains(a) {
                            return None;
                        }
                    }
                    _ => (),
                }
                match x {
                    Mir::If0(a, b, c) => Some(Mir::If0(
                        a,
                        remove_unread(b, reads),
                        remove_unread(c, reads),
                    )),
                    Mir::Loop(c) => Some(Mir::Loop(remove_unread(c, reads))),
                    Mir::Block(c) => Some(Mir::Block(remove_unread(c, reads))),
                    e => Some(e),
                }
            })
            .collect(),
    )
}
