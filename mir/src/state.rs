use lir::{AsmValue, CompilableInstruction, Counter, Label, Number, Var};

#[derive(Default)]
pub struct MirState {
    pub count: Counter,
    pub instructions: Vec<CompilableInstruction>,
    pub loops: Vec<Label>,
    pub blocks: Vec<Label>,
}

impl MirState {
    pub fn opt_asm(&mut self) {
        self.instructions = CompilableInstruction::optimize(self.instructions.clone());
    }
    pub fn jump(&mut self, label: Label) {
        self.instructions.push(CompilableInstruction::Jump(label));
    }
    pub fn dec(&mut self, variable: Var) {
        self.instructions
            .push(CompilableInstruction::Decrement(variable));
    }
    pub fn inc(&mut self, variable: Var) {
        self.instructions
            .push(CompilableInstruction::Increment(variable));
    }
    pub fn if0(&mut self, variable: Var, label: Label) {
        self.instructions
            .push(CompilableInstruction::If0(variable, label));
    }
    pub fn copy(&mut self, variable: Var, value: AsmValue) {
        self.instructions
            .push(CompilableInstruction::Copy(variable, value));
    }
    pub fn get_reg(&mut self, variable: Var, reg: Number) {
        self.instructions
            .push(CompilableInstruction::ReadRegister(variable, reg));
    }
    pub fn set_reg(&mut self, reg: Number, value: AsmValue) {
        self.instructions
            .push(CompilableInstruction::WriteRegister(reg, value));
    }
    pub fn stop(&mut self) {
        self.instructions.push(CompilableInstruction::Stop);
    }
    pub fn label(&mut self, label: Label) {
        self.instructions.push(CompilableInstruction::Label(label));
    }
}
