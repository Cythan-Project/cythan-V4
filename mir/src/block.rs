use std::{
    ops::{Deref, DerefMut},
    vec::IntoIter,
};

use crate::optimizer::state::OptimizerState;

use crate::{
    mir::Mir,
    optimizer::{
        old::{keep_block, OptConfig},
        optimize::Optimize,
    },
    skip_status::SkipStatus,
    state::MirState,
};

#[derive(PartialEq, Clone, Hash, Debug)]
pub struct MirCodeBlock(pub Vec<Mir>);

impl From<Mir> for MirCodeBlock {
    fn from(a: Mir) -> Self {
        Self(vec![a])
    }
}
impl From<Vec<Mir>> for MirCodeBlock {
    fn from(a: Vec<Mir>) -> Self {
        Self(a)
    }
}

impl Deref for MirCodeBlock {
    type Target = Vec<Mir>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for MirCodeBlock {
    fn deref_mut(&mut self) -> &mut Vec<Mir> {
        &mut self.0
    }
}

impl IntoIterator for MirCodeBlock {
    type Item = Mir;

    type IntoIter = IntoIter<Mir>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl MirCodeBlock {
    pub fn to_asm(&self, state: &mut MirState) -> SkipStatus {
        for i in &self.0 {
            match i.to_asm(state) {
                SkipStatus::None => (),
                e => return e,
            }
        }
        SkipStatus::None
    }
}

impl Default for MirCodeBlock {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl MirCodeBlock {
    #[allow(dead_code)]
    pub fn optimize_code(&self) -> Self {
        let before = self.instr_count();
        let mut after = self.clone();
        let mut bf = before;
        let mut cafter = 0;

        let opti = OptConfig::default();
        while bf != cafter {
            bf = cafter;
            let opt = after.optimize(&mut OptimizerState::new(), &opti);
            after = if opti.remove_unused_vars {
                keep_block(&opt, &mut opt.get_reads())
            } else {
                opt
            };
            cafter = after.instr_count();
            println!("OPT Pass done {}", cafter);
        }
        std::fs::write(
            "target/opt-loop.mir",
            after
                .0
                .iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        after
    }
    #[allow(dead_code)]
    pub fn instr_count(&self) -> usize {
        self.0
            .iter()
            .map(|x| match x {
                Mir::If0(_, a, b) => a.instr_count() + b.instr_count() + 1,
                Mir::Loop(a) | Mir::Block(a) => 1 + a.instr_count(),
                _ => 1,
            })
            .sum()
    }

    pub fn add(&mut self, mut mir: MirCodeBlock) -> &mut Self {
        self.0.append(&mut mir.0);
        self
    }

    pub fn add_mir(&mut self, mir: Mir) -> &mut Self {
        self.0.push(mir);
        self
    }

    pub fn copy(&mut self, to: u32, from: u32) -> &mut Self {
        self.0.push(Mir::Copy(to, from));
        self
    }

    pub fn copy_bulk(&mut self, to: &[u32], from: &[u32]) -> &mut Self {
        if to.len() != from.len() {
            panic!("Invalid copy operation");
        }
        to.iter().zip(from.iter()).for_each(|(to, from)| {
            self.0.push(Mir::Copy(*to, *from));
        });
        self
    }

    pub fn set_bulk(&mut self, to: &[u32], from: &[u8]) -> &mut Self {
        if to.len() != from.len() {
            panic!("Invalid set operation");
        }
        to.iter().zip(from.iter()).for_each(|(to, from)| {
            self.0.push(Mir::Set(*to, *from));
        });
        self
    }

    #[allow(dead_code)]
    pub fn set(&mut self, to: u32, value: u8) -> &mut Self {
        self.0.push(Mir::Set(to, value));
        self
    }
}
