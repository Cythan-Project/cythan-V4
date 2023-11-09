use std::collections::{HashMap, HashSet};

use crate::MirCodeBlock;


pub fn remove_no_reads(inputmir: MirCodeBlock) -> MirCodeBlock {
    let reads: std::collections::HashSet<u32> = inputmir.get_reads();
    fn inner(codeblock: MirCodeBlock, reads: &HashSet<u32>) -> MirCodeBlock {
        let mut newblock = MirCodeBlock::default();
        // Remove all elements that write to a register that is not read
        for i in codeblock {
            match i {
                crate::Mir::Set(a,b) => {
                    if reads.contains(&a) {
                        newblock.push(crate::Mir::Set(a,b));
                    }
                }
                crate::Mir::Copy(a,b) => {
                    if reads.contains(&a) {
                        newblock.push(crate::Mir::Copy(a,b));
                    }
                }
                crate::Mir::Increment(a) => {
                    if reads.contains(&a) {
                        newblock.push(crate::Mir::Increment(a));
                    }
                }
                crate::Mir::Decrement(a) => {
                    if reads.contains(&a) {
                        newblock.push(crate::Mir::Decrement(a));
                    }
                }
                crate::Mir::If0(a,b,c) => {
                    newblock.push(crate::Mir::If0(a,inner(b,reads),inner(c,reads)));
                }
                crate::Mir::Loop(a) => {
                    newblock.push(crate::Mir::Loop(inner(a,reads)));
                }
                crate::Mir::Break => {
                    newblock.push(crate::Mir::Break);
                }
                crate::Mir::Continue => 
                    newblock.push(crate::Mir::Continue),
                crate::Mir::Stop => 
                    newblock.push(crate::Mir::Stop),
                crate::Mir::ReadRegister(a,b) => {
                    if reads.contains(&a) {
                        newblock.push(crate::Mir::ReadRegister(a,b));
                    }
                }
                crate::Mir::WriteRegister(a, b) => {
                    newblock.push(crate::Mir::WriteRegister(a,b));
                }
                crate::Mir::Skip => {
                    newblock.push(crate::Mir::Skip);
                }
                crate::Mir::Block(a) => {
                    newblock.push(crate::Mir::Block(inner(a,reads)));
                }
                crate::Mir::Match(a,b) => {
                    newblock.push(crate::Mir::Match(a,b.into_iter().map(|(a,b)| (inner(a,reads), b)).collect()));
                }
            }
        }
        newblock
    }
    inner(inputmir, &reads)
}