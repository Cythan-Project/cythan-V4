use crate::compiler::mir::{Mir, MirCodeBlock};

pub fn remove_skips(mir: Vec<Mir>, in_loop: bool) -> Vec<Mir> {
    let mut new_mir = vec![];
    for block in mir {
        new_mir.push(match block {
            Mir::If0(c, a, b) => Mir::If0(
                c,
                MirCodeBlock(remove_skips(a.0, in_loop)),
                MirCodeBlock(remove_skips(b.0, in_loop)),
            ),
            Mir::Loop(a) => Mir::Loop(MirCodeBlock(remove_skips(a.0, true))),
            Mir::Skip => {
                if in_loop {
                    Mir::Break
                } else {
                    continue;
                }
            }
            e => e,
        });
    }
    new_mir
}
pub fn need_block(mir: &[Mir]) -> bool {
    fn inner(mir: &[Mir], first_layer: bool) -> bool {
        if mir.is_empty() {
            return false;
        }
        for i in &mir[0..mir.len() - if first_layer { 1 } else { 0 }] {
            match i {
                Mir::If0(_, a, b) => {
                    if inner(&a.0, false) || inner(&b.0, false) {
                        return true;
                    }
                }
                Mir::Loop(a) => {
                    if inner(&a.0, false) {
                        return true;
                    }
                }
                Mir::Skip => return true,
                _ => (),
            }
        }
        if first_layer {
            match mir.last().unwrap() {
                Mir::If0(_, a, b) => {
                    if inner(&a.0, first_layer) || inner(&b.0, first_layer) {
                        return true;
                    }
                }
                _ => (),
            }
            false
        } else {
            false
        }
    }
    inner(mir, true)
}
