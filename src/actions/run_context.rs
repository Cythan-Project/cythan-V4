use std::{io::Write, rc::Rc, sync::Mutex};

use cythan::{Cythan, InterruptedCythan};

use crate::{
    compiler::{
        asm::{interpreter::MemoryState, template::Template, Context},
        mir::{MirCodeBlock, MirState},
    },
    MIR_MODE,
};

pub struct StdIoContext;

pub trait RunContext {
    fn input(&mut self) -> u8;
    fn print(&mut self, i: char);
}

impl RunContext for StdIoContext {
    fn input(&mut self) -> u8 {
        let mut string = String::new();
        std::io::stdin().read_line(&mut string).unwrap();
        string.bytes().next().unwrap()
    }

    fn print(&mut self, i: char) {
        print!("{}", i);
        std::io::stdout().flush().unwrap();
    }
}

pub fn run<T: RunContext + 'static>(mir: &MirCodeBlock, car: T) -> (usize, Rc<Mutex<T>>) {
    let car = Rc::new(Mutex::new(car));
    let car1 = car.clone();
    let car2 = car.clone();
    std::fs::write(
        "out.mir",
        &mir.0
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    if MIR_MODE {
        let mut ms = MemoryState::new(2048, 8);
        ms.execute_block(mir, &mut *car.lock().unwrap());
        (ms.instr_count, car)
    } else {
        let mut mirstate = MirState::default();
        mir.to_asm(&mut mirstate);
        mirstate.opt_asm();
        let mut compile_state = Template::default();
        let mut ctx = Context::default();
        mirstate
            .instructions
            .iter()
            .for_each(|i| i.compile(&mut compile_state, &mut ctx));
        let k = compile_state.build();
        std::fs::write("out.ct", &k).unwrap();
        let k = cythan_compiler::compile(&k).unwrap();
        let mut machine = InterruptedCythan::new(
            k,
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
}