use std::{rc::Rc, sync::Mutex};

use cythan::{Cythan, InterruptedCythan};
use lir::CompilableInstruction;
use mir::{MemoryState, MirCodeBlock, MirState, RunContext};

const MIR_MODE: bool = false;

pub fn run<T: RunContext + 'static>(mir: &MirCodeBlock, car: T) -> (usize, Rc<Mutex<T>>) {
    run_with_limit(mir, car, 0)
}

pub fn run_with_limit<T: RunContext + 'static>(
    mir: &MirCodeBlock,
    car: T,
    max_steps: usize,
) -> (usize, Rc<Mutex<T>>) {
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
        run_bin_with_limit(&k, car, max_steps)
    }
}

pub fn compute_max_bin(k: &[usize]) -> (usize, Vec<usize>) {
    let mut machine =
        InterruptedCythan::new_stdio(k.to_vec(), 4, 2 * 2_usize.pow(4 /* base */) + 3);
    let mut k = 0;
    loop {
        k += 1;
        let a = machine.cases.clone();
        if machine.next_get_interupt() || a == machine.cases {
            return (k, a);
        }
    }
}

pub fn run_bin<T: RunContext + 'static>(k: &[usize], car: T) -> (usize, Rc<Mutex<T>>) {
    run_bin_with_limit(k, car, 0)
}

pub fn run_bin_with_limit<T: RunContext + 'static>(
    k: &[usize],
    car: T,
    max_steps: usize,
) -> (usize, Rc<Mutex<T>>) {
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
        if max_steps > 0 && k > max_steps {
            panic!("Execution exceeded step limit of {} steps", max_steps);
        }
        let a = machine.cases.clone();
        machine.next();
        if a == machine.cases {
            break;
        }
    }
    (k, car2)
}
