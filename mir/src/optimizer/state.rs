use std::collections::{HashMap, HashSet};

use either::Either;

#[derive(Clone)]
pub struct OptimizerState {
    variables: HashMap<u32, VarState>,
}

impl OptimizerState {
    pub fn new() -> Self {
        OptimizerState {
            variables: HashMap::new(),
        }
    }

    pub fn get_var_raw(&self, id: u32) -> VarState {
        self.variables
            .get(&id)
            .unwrap_or(&VarState::Unknown)
            .clone()
    }

    pub fn get_var(&self, id: u32) -> VarState {
        match self.get_var_raw(id) {
            VarState::Ref(a) => self.get_var(a),
            e => e,
        }
    }

    pub fn get_var_value(&self, id: u32) -> Either<u8, u32> {
        match self.get_var_raw(id) {
            VarState::Ref(a) => self.get_var_value(a),
            VarState::Values(a) => {
                if a.len() == 1 {
                    Either::Left(a[0])
                } else {
                    Either::Right(id)
                }
            }
            VarState::Unknown => Either::Right(id),
        }
    }

    pub fn remove_vars(&mut self, ids: &HashSet<u32>) {
        for id in ids {
            self.variables.remove(id);
        }
    }

    pub fn remove_possibilities(&mut self, id: u32, pos: &[u8]) {
        self.set_var(
            id,
            VarState::Values(match self.get_var(id) {
                VarState::Ref(_) => unreachable!(),
                VarState::Values(a) => a.into_iter().filter(|&x| !pos.contains(&x)).collect(),
                VarState::Unknown => (0..16).filter(|&x| !pos.contains(&x)).collect(),
            }),
        )
    }

    pub fn set_var(&mut self, id: u32, state: VarState) {
        self.variables
            .iter()
            .filter(|(id, x)| matches!(x, VarState::Ref(j) if j == *id))
            .map(|x| *x.0)
            .collect::<Vec<u32>>()
            .into_iter()
            .for_each(|x| self.set_var(x, self.get_var(id)));

        self.variables.insert(id, state);
    }

    pub fn merge(&self, other: &Self) -> Self {
        let mut variables = HashMap::new();
        for (id, state) in other.variables.iter() {
            if self.variables.get(id) == Some(state) {
                variables.insert(*id, state.clone());
            }
        }
        OptimizerState { variables }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum VarState {
    Ref(u32),
    Values(Vec<u8>),
    Unknown,
}
