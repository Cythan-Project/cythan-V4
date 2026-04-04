use std::collections::HashSet;

use crate::{Mir, MirCodeBlock};

impl MirCodeBlock {
    pub fn get_writes(&self) -> HashSet<u32> {
        fn inner(mir: &Mir, muts: &mut HashSet<u32>) {
            match mir {
                Mir::Set(a, _)
                | Mir::Copy(a, _)
                | Mir::ReadRegister(a, _)
                | Mir::Increment(a)
                | Mir::Decrement(a) => {
                    muts.insert(*a);
                }
                Mir::If0(_, a, b) => {
                    a.iter().for_each(|x| inner(x, muts));
                    b.iter().for_each(|x| inner(x, muts));
                }
                Mir::Loop(a) | Mir::Block(a) => {
                    a.iter().for_each(|x| inner(x, muts));
                }
                Mir::Break => (),
                Mir::Continue => (),
                Mir::Stop => (),
                Mir::WriteRegister(_, _) => (),
                Mir::Skip => (),
                Mir::Match(_, b) => {
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
