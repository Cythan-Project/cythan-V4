use std::{
    collections::{HashMap, HashSet},
    ops::RangeBounds,
    slice::SliceIndex,
};

// 7715
// 7242

use either::Either;

use crate::{Mir, MirCodeBlock};

#[derive(Clone)]
struct OptContext {
    variables: HashMap<u32, VariableStatus>,
}

pub fn get_static_vars(cb: &MirCodeBlock) -> HashMap<u32, u8> {
    fn inner(cb: &MirCodeBlock, vars: &mut HashMap<u32, u8>) {
        cb.iter().for_each(|x| match x {
            Mir::Set(a, b) => {
                vars.insert(*a, *b);
            }
            Mir::If0(_, b, c) => {
                inner(b, vars);
                inner(c, vars);
            }
            Mir::Block(a) => {
                inner(a, vars);
            }
            _ => (),
        });
    }
    fn remove_inner(cb: &MirCodeBlock, vars: &mut HashMap<u32, u8>, in_rm: bool) {
        cb.iter().for_each(|x| match x {
            Mir::Set(a, b) => {
                if in_rm {
                    if vars.get(a) != Some(b) {
                        vars.remove(a);
                    }
                }
            }
            Mir::Copy(a, _) => {
                vars.remove(a);
            }
            Mir::Increment(a) => {
                vars.remove(a);
            }
            Mir::Decrement(a) => {
                vars.remove(a);
            }
            Mir::If0(_, b, c) => {
                remove_inner(b, vars, in_rm);
                remove_inner(c, vars, in_rm);
            }
            Mir::Loop(a) => {
                remove_inner(a, vars, true);
            }
            Mir::Break => (),
            Mir::Continue => (),
            Mir::Stop => (),
            Mir::ReadRegister(a, _) => {
                vars.remove(a);
            }
            Mir::WriteRegister(_, _) => (),
            Mir::Skip => (),
            Mir::Block(a) => {
                remove_inner(a, vars, in_rm);
            }
            Mir::Match(a, b) => {
                vars.remove(a);
                b.iter().for_each(|(a, _)| {
                    remove_inner(a, vars, in_rm);
                });
            }
        });
    }
    let mut vars = HashMap::new();
    inner(cb, &mut vars);
    remove_inner(cb, &mut vars, false);
    vars
}

fn apply_static_vars(cb: MirCodeBlock, vars: &HashMap<u32, u8>) -> MirCodeBlock {
    MirCodeBlock(
        cb.into_iter()
            .flat_map(|x| {
                vec![match x {
                    Mir::Set(a, b) => Mir::Set(a, b),
                    Mir::Copy(a, b) => {
                        if let Some(e) = vars.get(&b) {
                            Mir::Set(a, *e)
                        } else {
                            Mir::Copy(a, b)
                        }
                    }
                    Mir::Increment(a) => Mir::Increment(a),
                    Mir::Decrement(a) => Mir::Decrement(a),
                    Mir::If0(a, b, c) => {
                        if let Some(e) = vars.get(&a) {
                            if e == &0 {
                                return apply_static_vars(b, &vars).0;
                            } else {
                                return apply_static_vars(c, &vars).0;
                            }
                        } else {
                            Mir::If0(a, apply_static_vars(b, &vars), apply_static_vars(c, &vars))
                        }
                    }
                    Mir::Loop(a) => Mir::Loop(apply_static_vars(a, &vars)),
                    Mir::Break => Mir::Break,
                    Mir::Continue => Mir::Continue,
                    Mir::Stop => Mir::Stop,
                    Mir::ReadRegister(a, b) => Mir::ReadRegister(a, b),
                    Mir::WriteRegister(a, b) => match b {
                        Either::Right(b) => {
                            if let Some(e) = vars.get(&b) {
                                Mir::WriteRegister(a, Either::Left(*e))
                            } else {
                                Mir::WriteRegister(a, Either::Right(b))
                            }
                        }
                        Either::Left(b) => Mir::WriteRegister(a, Either::Left(b)),
                    },
                    Mir::Skip => Mir::Skip,
                    Mir::Block(a) => Mir::Block(apply_static_vars(a, &vars)),
                    Mir::Match(a, b) => {
                        // TODO
                        Mir::Match(a, b)
                    }
                }]
            })
            .collect(),
    )
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
        let j = self.variables.remove(&id);
        k.iter().for_each(|x| {
            self.set_or_remove(*x, j);
        });
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
        let j = self.variables.remove(&id);
        k.iter().for_each(|x| {
            self.set_or_remove(*x, j);
        });
        self.variables.insert(id, status);
    }

    fn merge(&mut self, other: &mut Self) -> Self {
        let mut variables = HashMap::new();
        for a in other.variables.keys().copied().collect::<Vec<_>>() {
            if let Some(aa) = other.get_flatten(a) {
                if let Some(ab) = self.get_flatten(a) {
                    if aa == ab {
                        variables.insert(a, aa);
                    }
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

pub fn optimize_code(mir: MirCodeBlock, current_count: usize) -> MirCodeBlock {
    let mut context = OptContext::new();
    let k = MirCodeBlock(optimize_block(mir, &mut context));
    let statics = get_static_vars(&k);
    let k = apply_static_vars(k, &statics);
    let j = k.get_reads();
    let k = remove_unread(k, &j);
    let ic = k.instr_count();
    if ic == current_count {
        k
    } else {
        optimize_code(k, ic)
    }
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
        Mir::If0(a, b, c) => match context.get_flatten(a) {
            Some(VariableStatus::Ref(e)) => {
                let mut cb = context.clone();
                let mut cc = context.clone();
                let b = MirCodeBlock(optimize_block(b, &mut cb));
                let c = MirCodeBlock(optimize_block(c, &mut cc));
                *context = cb.merge(&mut cc);
                vec![Mir::If0(e, b, c)]
            }
            Some(VariableStatus::Value(e)) => {
                if e == 0 {
                    optimize_block(b, context)
                } else {
                    optimize_block(c, context)
                }
            }
            None => {
                let mut cb = context.clone();
                let mut cc = context.clone();
                let b = MirCodeBlock(optimize_block(b, &mut cb));
                let c = MirCodeBlock(optimize_block(c, &mut cc));
                *context = cb.merge(&mut cc);
                vec![Mir::If0(a, b, c)]
            }
        },
        Mir::Match(a, b) => match context.get_flatten(a) {
            Some(VariableStatus::Ref(e)) => {
                let mut cur_state: Option<OptContext> = None;
                let m: Vec<_> = b
                    .into_iter()
                    .map(|(x, y)| {
                        let mut cb = context.clone();
                        let x = MirCodeBlock(optimize_block(x, &mut cb));
                        cur_state = match &mut cur_state {
                            Some(a) => Some(a.merge(&mut cb)),
                            None => Some(cb),
                        };
                        (x, y)
                    })
                    .collect();

                *context = cur_state.unwrap();
                vec![Mir::Match(e, m)]
            }
            Some(VariableStatus::Value(e)) => {
                for j in b {
                    if j.1.contains(&e) {
                        return optimize_block(j.0, context);
                    }
                }
                return vec![];
            }
            None => {
                let mut cur_state: Option<OptContext> = None;
                let m: Vec<_> = b
                    .into_iter()
                    .map(|(x, y)| {
                        let mut cb = context.clone();
                        let x = MirCodeBlock(optimize_block(x, &mut cb));
                        cur_state = match &mut cur_state {
                            Some(a) => Some(a.merge(&mut cb)),
                            None => Some(cb),
                        };
                        (x, y)
                    })
                    .collect();

                *context = cur_state.unwrap();
                vec![Mir::Match(a, m)]
            }
        },
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
            let muts = a.get_writes();
            let a = MirCodeBlock(optimize_block(a, &mut cb));
            muts.iter().for_each(|x| context.remove(*x));
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
