use std::collections::{HashMap, HashSet, VecDeque};

// 7715
// 7242

use either::Either;

use crate::{Mir, MirCodeBlock};

use super::old::does_break_in_all_cases;

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
    let k = mir;
    let k = MirCodeBlock(optimize_block(k, &mut context));
    let statics = get_static_vars(&k);
    let k = apply_static_vars(k, &statics);
    let j = k.get_reads();
    let k = remove_unread(k, &j);
    let k = set_in_if(k);
    let k = opt_lower_interupts_calls(k);
    let k = opt_not_read(k);
    let k = unwrap_if(k);
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
/*
if v89 {
      v88 = 1
      break
    } else {
      if v90 {
        v88 = 0
        break
      }
    }
*/
pub fn unwrap_if(mut code: MirCodeBlock) -> MirCodeBlock {
    MirCodeBlock(code.into_iter().flat_map(|x| {
        match x {
            Mir::If0(a, b, c) => {
                if b.iter().any(|x| does_break_in_all_cases(x)) {
                    let mut vec = Vec::with_capacity(1 + c.len());
                    vec.push(Mir::If0(a, b, MirCodeBlock(vec![])));
                    vec.extend(c.into_iter());
                    return vec;
                } else if c.iter().any(|x| does_break_in_all_cases(x)) {
                    let mut vec = Vec::with_capacity(1 + b.len());
                    vec.push(Mir::If0(a, MirCodeBlock(vec![]), c));
                    vec.extend(b.into_iter());
                    return vec;
                } else {
                    return vec![Mir::If0(a, b, c)];
                }
            }
            Mir::Block(a) => {
                let a = unwrap_if(a);
                vec![Mir::Block(a)]
            }
            Mir::Loop(a) => {
                let a = unwrap_if(a);
                vec![Mir::Loop(a)]
            }
            Mir::Match(a, b) => {
                let b: Vec<_> = b
                    .into_iter()
                    .map(|(a, b)| (unwrap_if(a), b))
                    .collect();
                vec![Mir::Match(a, b)]
            }
            x => vec![x],
        }
    }).collect())
}
pub fn opt_not_read(mut code: MirCodeBlock) -> MirCodeBlock {
    let mut wrote = HashSet::new();
    code.reverse();
    let mut out = code
        .into_iter()
        .flat_map(|x| match x {
            Mir::Set(a, b) => {
                if wrote.contains(&a) {
                    return vec![];
                }
                wrote.insert(a);
                vec![Mir::Set(a, b)]
            }
            Mir::Copy(a, b) => {
                if wrote.contains(&a) {
                    return vec![];
                }
                wrote.insert(a);
                wrote.remove(&b);
                vec![Mir::Copy(a, b)]
            }
            Mir::Increment(a) => {
                if wrote.contains(&a) {
                    return vec![];
                }
                vec![Mir::Increment(a)]
            }
            Mir::Decrement(a) => {
                if wrote.contains(&a) {
                    return vec![];
                }
                vec![Mir::Decrement(a)]
            }
            Mir::If0(a, b, c) => {
                wrote.clear();
                let b = opt_not_read(b);
                let c = opt_not_read(c);
                vec![Mir::If0(a, b, c)]
            }
            Mir::Loop(a) => {
                wrote.clear();
                let a = opt_not_read(a);
                vec![Mir::Loop(a)]
            }
            Mir::Break => {
                wrote.clear();
                vec![Mir::Break]
            }
            Mir::Continue => {
                wrote.clear();
                vec![Mir::Continue]
            }
            Mir::Stop => {
                wrote.clear();
                vec![Mir::Stop]
            }
            Mir::ReadRegister(a, b) => {

                wrote.insert(a);
                vec![Mir::ReadRegister(a, b)]
            }
            Mir::WriteRegister(a, b) => {
                wrote.clear();
                vec![Mir::WriteRegister(a, b)]
            }
            Mir::Skip => {
                wrote.clear();
                vec![Mir::Skip]
            }
            Mir::Block(a) => {
                wrote.clear();
                let a = opt_not_read(a);
                vec![Mir::Block(a)]
            }
            Mir::Match(a, b) => {
                wrote.clear();
                let b: Vec<_> = b.into_iter().map(|(a, b)| (opt_not_read(a), b)).collect();
                vec![Mir::Match(a, b)]
            }
        })
        .collect::<Vec<_>>();
    out.reverse();
    MirCodeBlock(out)
}
pub fn opt_lower_interupts_calls(code: MirCodeBlock) -> MirCodeBlock {
    let (mut code, to_exe) = lower_interupts_calls(code, Vec::new());
    code.0.extend(to_exe.into_iter());
    code
}
fn lower_interupts_calls(code: MirCodeBlock, mut to_lower: Vec<Mir>) -> (MirCodeBlock, Vec<Mir>) {
    let mut out = Vec::new();
    for i in code.into_iter() {
        let iboth = i.get_acesses();
        if to_lower.iter().any(|x| {
            let both = x.get_acesses();
            if both.iter().any(|x| iboth.contains(x)) {
                return true;
            }
            false
        }) {
            out.extend(to_lower.into_iter());
            to_lower = Vec::new();
            out.push(i);
        } else {
            match i {
                Mir::ReadRegister(a, b) => {
                    to_lower.push(Mir::ReadRegister(a, b));
                }
                Mir::WriteRegister(a, b) => {
                    to_lower.push(Mir::WriteRegister(a, b));
                }
                Mir::If0(a, b, c) => {
                    let (mut b, mut to_lower1) = lower_interupts_calls(b, to_lower.clone());
                    let (mut c, mut to_lower2) = lower_interupts_calls(c, to_lower.clone());
                    let mut tl4 = VecDeque::new();
                    while to_lower1.last() == to_lower2.last() && to_lower2.last().is_some() {
                        let k = to_lower1.pop().unwrap();
                        to_lower2.pop();
                        tl4.push_front(k);
                    }
                    to_lower = Vec::new();
                    to_lower.extend(tl4.into_iter());
                    b.0.extend(to_lower1.into_iter());
                    c.0.extend(to_lower2.into_iter());
                    out.push(Mir::If0(a, b, c));
                }
                Mir::Loop(a) => {
                    out.extend(to_lower.into_iter());
                    to_lower = Vec::new();
                    let (mut a, to_lower1) = lower_interupts_calls(a, Vec::new());
                    a.extend(to_lower1.into_iter());
                    out.push(Mir::Loop(a));
                }
                Mir::Break => {
                    out.extend(to_lower.into_iter());
                    to_lower = Vec::new();
                    out.push(Mir::Break);
                }
                Mir::Continue => {
                    out.extend(to_lower.into_iter());
                    to_lower = Vec::new();
                    out.push(Mir::Continue);
                }
                Mir::Stop => {
                    out.extend(to_lower.into_iter());
                    to_lower = Vec::new();
                    out.push(Mir::Stop);
                }
                Mir::Skip => {
                    out.extend(to_lower.into_iter());
                    to_lower = Vec::new();
                    out.push(Mir::Skip);
                }
                Mir::Block(a) => {
                    let (a, to_lower1) = lower_interupts_calls(a, to_lower.clone());
                    to_lower = to_lower1;
                    out.push(Mir::Block(a));
                }
                Mir::Match(a, b) => {
                    out.extend(to_lower.into_iter());
                    to_lower = Vec::new();
                    out.push(Mir::Match(
                        a,
                        b.into_iter()
                            .map(|(a, b)| {
                                let (mut a, to_lower1) = lower_interupts_calls(a, Vec::new());
                                a.extend(to_lower1.into_iter());
                                (a, b)
                            })
                            .collect(),
                    ));
                }
                e => {
                    out.push(e);
                }
            }
        }
    }
    (MirCodeBlock(out), to_lower)
}
#[test]
fn test_set_in_if() {
    let code = MirCodeBlock(vec![
        Mir::If0(
            68,
            MirCodeBlock(vec![Mir::Set(77, 15), Mir::Set(78, 4)]),
            MirCodeBlock(vec![Mir::Set(77, 8), Mir::Set(78, 5)]),
        ),
        Mir::Copy(81, 77),
        Mir::Copy(82, 78),
    ]);
    let code = set_in_if(code);
    println!("{:?}", code);
}
fn set_in_if(block: MirCodeBlock) -> MirCodeBlock {
    let mut out = Vec::new();
    let mut vecqueue = VecDeque::from(block.0);

    'a: while let Some(item) = vecqueue.pop_front() {
        match &item {
            Mir::Block(e) => {
                out.push(Mir::Block(set_in_if(e.clone())));
                continue 'a;
            }
            Mir::Loop(e) => {
                out.push(Mir::Loop(set_in_if(e.clone())));
                continue 'a;
            }
            Mir::Match(e, w) => {
                out.push(Mir::Match(
                    *e,
                    w.clone()
                        .into_iter()
                        .map(|(a, b)| (set_in_if(a), b))
                        .collect(),
                ));
                continue 'a;
            }
            Mir::If0(a, b, c) => {
                let mut map: HashMap<u32, Either<u8, u32>> = HashMap::new();
                let mut map1: HashMap<u32, Either<u8, u32>> = HashMap::new();
                let mut items = HashSet::new();
                for mir1 in b.iter() {
                    match mir1 {
                        Mir::Set(a, b) => {
                            map.insert(*a, Either::Left(*b));
                            items.insert(*a);
                        }
                        Mir::Copy(a, b) => {
                            map.insert(*a, Either::Right(*b));
                            items.insert(*a);
                        }
                        _ => {
                            out.push(Mir::If0(*a, set_in_if(b.clone()), set_in_if(c.clone())));
                            continue 'a;
                        }
                    }
                }
                let writes = b.get_writes();
                let reads = b.get_reads();
                for read in reads {
                    if writes.contains(&read) {
                        out.push(Mir::If0(*a, set_in_if(b.clone()), set_in_if(c.clone())));
                        continue 'a;
                    }
                }
                let writes = c.get_writes();
                let reads = c.get_reads();
                for read in reads {
                    if writes.contains(&read) {
                        out.push(Mir::If0(*a, set_in_if(b.clone()), set_in_if(c.clone())));
                        continue 'a;
                    }
                }
                for mir1 in c.iter() {
                    match mir1 {
                        Mir::Set(a, b) => {
                            map1.insert(*a, Either::Left(*b));
                            items.insert(*a);
                        }
                        Mir::Copy(a, b) => {
                            map1.insert(*a, Either::Right(*b));
                            items.insert(*a);
                        }
                        _ => {
                            out.push(Mir::If0(*a, set_in_if(b.clone()), set_in_if(c.clone())));
                            continue 'a;
                        }
                    }
                }
                let mut map2 = map.clone();
                let mut map3 = map1.clone();
                for field in &items {
                    if map.remove(field).is_none() || map1.remove(field).is_none() {
                        out.push(Mir::If0(*a, set_in_if(b.clone()), set_in_if(c.clone())));
                        continue 'a;
                    }
                }
                if !map.is_empty() || !map1.is_empty() {
                    out.push(Mir::If0(*a, set_in_if(b.clone()), set_in_if(c.clone())));
                    continue 'a;
                }
                let mut retreived1 = HashMap::new();
                let mut retreived2 = HashMap::new();
                let mut list = Vec::new();
                for _ in 0..items.len() {
                    if let Some(e) = vecqueue.pop_front() {
                        list.push(e.clone());
                        match e {
                            Mir::Copy(ac, bc) => {
                                if let (Some(e), Some(f)) = (map2.remove(&bc), map3.remove(&bc)) {
                                    retreived1.insert(ac, e);
                                    retreived2.insert(ac, f);
                                } else {
                                    out.push(Mir::If0(
                                        *a,
                                        set_in_if(b.clone()),
                                        set_in_if(c.clone()),
                                    ));
                                    for i in list.into_iter().rev() {
                                        vecqueue.push_front(i);
                                    }
                                    continue 'a;
                                }
                            }
                            _ => {
                                out.push(Mir::If0(*a, set_in_if(b.clone()), set_in_if(c.clone())));
                                for i in list.into_iter().rev() {
                                    vecqueue.push_front(i);
                                }
                                continue 'a;
                            }
                        }
                    } else {
                        out.push(Mir::If0(*a, set_in_if(b.clone()), set_in_if(c.clone())));
                        for i in list.into_iter().rev() {
                            vecqueue.push_front(i);
                        }
                        continue 'a;
                    }
                }
                if map2.is_empty() && map3.is_empty() {
                    out.push(Mir::If0(
                        *a,
                        MirCodeBlock({
                            let mut out = Vec::new();
                            for (a, b) in retreived1 {
                                match b {
                                    Either::Left(b) => out.push(Mir::Set(a, b)),
                                    Either::Right(b) => out.push(Mir::Copy(a, b)),
                                }
                            }
                            out
                        }),
                        MirCodeBlock({
                            let mut out = Vec::new();
                            for (a, b) in retreived2 {
                                match b {
                                    Either::Left(b) => out.push(Mir::Set(a, b)),
                                    Either::Right(b) => out.push(Mir::Copy(a, b)),
                                }
                            }
                            out
                        }),
                    ));
                    continue 'a;
                } else {
                    out.push(item);
                    continue 'a;
                }
            }
            _ => out.push(item),
        }
    }
    MirCodeBlock(out)
}

fn optimize(mir: Mir, context: &mut OptContext) -> Vec<Mir> {
    match mir {
        Mir::Set(a, b) => {
            if context.get_value(a) == Some(b) {
                return vec![];
            }
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
