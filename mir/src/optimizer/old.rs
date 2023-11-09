use std::collections::HashSet;

use crate::{
    block::MirCodeBlock,
    mir::Mir,
};

pub struct OptConfig {
    pub inline_loops: bool,
    pub inline_blocks: bool,
    pub reorganize_ifs: bool,
    pub remove_unused_vars: bool,
}

impl Default for OptConfig {
    fn default() -> Self {
        Self {
            inline_loops: true,
            inline_blocks: true,
            reorganize_ifs: true,
            remove_unused_vars: true,
        }
    }
}

pub fn keep_block(mir: &MirCodeBlock, reads: &mut HashSet<u32>) -> MirCodeBlock {
    let mut old1: Option<Mir> = None;
    let mut out = Vec::new();
    for i in mir.iter().flat_map(|x| keep(x, reads)) {
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
pub fn keep(mir: &Mir, reads: &mut HashSet<u32>) -> Option<Mir> {
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
        Mir::Match(a, b) => {
            return Some(Mir::Match(
                *a,
                b.iter()
                    .map(|(x, y)| (keep_block(x, reads), y.clone()))
                    .collect(),
            ))
        }
    }
    Some(mir.clone())
}

pub fn improve_code_flow(block: Vec<Mir>, opt: &OptConfig) -> Vec<Mir> {
    if !opt.reorganize_ifs {
        return block;
    }
    let mut out = Vec::new();
    let mut k = block.into_iter();
    while let Some(e) = k.next() {
        if let Mir::If0(a, b, c) = &e {
            if b.iter().any(|x| always_terminate(x)) {
                out.push(Mir::If0(
                    *a,
                    b.clone(),
                    MirCodeBlock(improve_code_flow(c.iter().cloned().chain(k).collect(), opt)),
                ));
                return out;
            } else if c.iter().any(|x| always_terminate(x)) {
                out.push(Mir::If0(
                    *a,
                    MirCodeBlock(improve_code_flow(b.iter().cloned().chain(k).collect(), opt)),
                    c.clone(),
                ));
                return out;
            }
        }
        out.push(e);
    }
    out
}

/* pub fn try_unroll_loop(
    state: &mut OptimizerState,
    lp: &[Mir],
    opt: &OptConfig,
) -> (bool, MirCodeBlock) {
    let mut o = vec![];
    'r: for _ in 0..16 {
        for i in lp {
            for i in i.optimize(state, opt) {
                if contains_continues(&i) || contains_skip(&i) {
                    break 'r;
                }
                o.push(remove_breaks(&i));
                if does_break_in_all_cases(&i) {
                    return (true, Mir::Block(MirCodeBlock(o)).into());
                }
            }
        }
    }
    (false, Mir::Loop(MirCodeBlock(lp.to_vec())).into())
} */

pub fn remove_skips(mir: &Mir) -> Option<Mir> {
    match mir {
        Mir::If0(c, a, b) => Some(Mir::If0(
            *c,
            MirCodeBlock(a.iter().flat_map(|x| remove_skips(x)).collect()),
            MirCodeBlock(b.iter().flat_map(|x| remove_skips(x)).collect()),
        )),
        Mir::Skip => None,
        Mir::Loop(a) => Some(Mir::Block(MirCodeBlock(
            a.iter().flat_map(|x| remove_skips(x)).collect(),
        ))),
        e => Some(e.clone()),
    }
}

pub fn always_terminate(mir: &Mir) -> bool {
    match mir {
        Mir::If0(_, a, b) => {
            a.iter().any(|x| always_terminate(x)) && b.iter().any(|x| always_terminate(x))
        }
        Mir::Skip | Mir::Continue | Mir::Break | Mir::Stop => true,
        _ => false,
    }
}

pub fn remove_breaks(mir: &Mir) -> Mir {
    match mir {
        Mir::If0(c, a, b) => Mir::If0(
            *c,
            MirCodeBlock(a.iter().map(remove_breaks).collect()),
            MirCodeBlock(b.iter().map(remove_breaks).collect()),
        ),
        Mir::Break => Mir::Skip,
        Mir::Block(a) => Mir::Block(MirCodeBlock(a.iter().map(remove_breaks).collect())),
        e => e.clone(),
    }
}

pub fn contains_skip(mir: &Mir) -> bool {
    match mir {
        Mir::If0(_, a, b) => {
            a.iter().any(|x| contains_skip(x)) || b.iter().any(|x| contains_skip(x))
        }
        Mir::Skip => true,
        Mir::Loop(a) => a.iter().any(|x| contains_skip(x)),
        _ => false,
    }
}

pub fn contains_continues(mir: &Mir) -> bool {
    match mir {
        Mir::If0(_, a, b) => {
            a.iter().any(|x| contains_continues(x)) || b.iter().any(|x| contains_continues(x))
        }
        Mir::Continue => true,
        Mir::Block(a) => a.iter().any(|x| contains_continues(x)),
        _ => false,
    }
}

pub fn does_skip_in_all_cases(mir: &Mir) -> bool {
    match mir {
        Mir::If0(_, a, b) => {
            a.iter().any(|x| does_skip_in_all_cases(x))
                && b.iter().any(|x| does_skip_in_all_cases(x))
        }
        Mir::Continue => true,
        Mir::Break => true,
        Mir::Skip => true,
        Mir::Stop => true,
        _ => false,
    }
}

pub fn does_break_in_all_cases(mir: &Mir) -> bool {
    match mir {
        Mir::If0(_, a, b) => {
            a.iter().any(|x| does_break_in_all_cases(x))
                && b.iter().any(|x| does_break_in_all_cases(x))
        }
        Mir::Break => true,
        Mir::Stop => true,
        _ => false,
    }
}
