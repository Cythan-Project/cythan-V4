use std::{rc::Rc, sync::Mutex};

use cythan::{Cythan, InterruptedCythan};
use lir::CompilableInstruction;
use mir::{MemoryState, MirCodeBlock, MirState, RunContext};

use crate::MIR_MODE;

pub fn run<T: RunContext + 'static>(mir: &MirCodeBlock, car: T) -> (usize, Rc<Mutex<T>>) {
    if MIR_MODE {
        let car = Rc::new(Mutex::new(car));
        let mut ms = MemoryState::new(2048, 8);
        ms.execute_block(mir, &mut *car.lock().unwrap());
        (ms.instr_count, car)
    } else {
        let mut mirstate = MirState::default();
        mir.to_asm(&mut mirstate);
        mirstate.opt_asm();
        let k = CompilableInstruction::compile_to_binary(mirstate.instructions);
        run_bin(&k, car)
    }
}

pub fn run_bin<T: RunContext + 'static>(k: &[usize], car: T) -> (usize, Rc<Mutex<T>>) {
    let car = Rc::new(Mutex::new(car));
    let car1 = car.clone();
    let car2 = car.clone();
    let mut machine = InterruptedCythan::new(
        k.to_vec(),
        4,
        2 * 2_usize.pow(4 /* base */) + 3,
        move |a| {
            car.lock().unwrap().print(a as char);
        },
        move || car1.lock().unwrap().input(),
    );
    let mut k = 0;
    loop {
        k += 1;
        let a = machine.cases.clone();
        machine.next();
        if a == machine.cases {
            break;
        }
    }
    (k, car2)
}
