use crate::{
    mir::Mir,
    optimizer::old::{self, get_reads_from_block, keep_block, OptimizerState, REMOVE_UNUSED_VARS},
    skip_status::SkipStatus,
    state::MirState,
};

#[derive(PartialEq, Clone, Hash, Debug)]
pub struct MirCodeBlock(pub Vec<Mir>);

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

impl From<Vec<Mir>> for MirCodeBlock {
    fn from(v: Vec<Mir>) -> Self {
        Self(v)
    }
}

impl MirCodeBlock {
    #[allow(dead_code)]
    pub fn optimize(&self) -> Self {
        let before = self.instr_count();
        let mut after = self.clone();
        let mut bf = before;
        let mut cafter = 0;
        //let mut i = 0;
        while bf != cafter {
            bf = cafter;
            let opt = old::optimize_block(&after, &mut OptimizerState::new());
            after = if REMOVE_UNUSED_VARS {
                keep_block(&opt, &mut get_reads_from_block(&opt))
                // new_optimizer::remove_unused_vars(&opt)
            } else {
                opt
            };
            /* println!("OPT STEP");
            after = new_optimizer::opt(&after);
            after = new_optimizer::remove_unused_vars(&after); */
            cafter = after.instr_count();
            //i += 1;
            //break;
        }

        /* println!(
            "Optimized from {} to {} ({}%) in {} iter",
            before,
            cafter,
            100 - 100 * cafter / before,
            i
        ); */
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
