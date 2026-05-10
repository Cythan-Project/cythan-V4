use std::collections::HashSet;

use either::Either;

use crate::{Mir, MirCodeBlock};

impl MirCodeBlock {
    pub fn get_reads(&self) -> HashSet<u32> {
        fn inner(mir: &Mir, muts: &mut HashSet<u32>) {
            match mir {
                Mir::Copy(_, a) | Mir::WriteRegister(_, Either::Right(a)) => {
                    muts.insert(*a);
                }
                Mir::If0(c, a, b) => {
                    muts.insert(*c);
                    a.iter().for_each(|x| inner(x, muts));
                    b.iter().for_each(|x| inner(x, muts));
                }
                Mir::Loop(a) | Mir::Block(a) => {
                    a.iter().for_each(|x| inner(x, muts));
                }
                Mir::Break => (),
                Mir::Continue => (),
                Mir::Stop => (),
                Mir::Skip => (),
                Mir::Set(_, _) => (),
                Mir::MapValue(src, _, _) => {
                    // Reads src; if dst != src the dst is purely a write.
                    // (Inc/Dec have src == dst — both happen.)
                    muts.insert(*src);
                }
                Mir::ReadRegister(_, _) => (),
                Mir::WriteRegister(_, Either::Left(_)) => (),
                Mir::Match(a, b) => {
                    muts.insert(*a);
                    b.iter().for_each(|(b, _)| {
                        b.iter().for_each(|x| inner(x, muts));
                    });
                }
            }
        }
        let mut set = HashSet::new();
        self.iter().for_each(|x| inner(x, &mut set));
        set
    }
}
