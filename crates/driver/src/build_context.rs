use std::process::exit;

use errors::{report, Error, Span, SpannedObject};
use mir::{Mir, MirCodeBlock};

use crate::STACK_SIZE;
use cythan_frontend::{
    compiler::{
        class_loader::ClassLoader,
        state::{code_manager::CodeManager, local_state::LocalState},
    },
    natives::load_natives,
    parser::ty::Type,
};

pub fn compile(class_name: String, optimize: bool) -> MirCodeBlock {
    let child = std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(move || generate_mir(&class_name))
        .unwrap();
    let k = child.join().unwrap();
    let count = k.instr_count();
    let k = if optimize { k.optimize_code_new() } else { k };
    let ncount = k.instr_count();
    eprintln!(
        "Optimized from {} to {} ({:.02}%)",
        count,
        ncount,
        if count > 0 {
            (count - ncount) as f64 / count as f64 * 100.
        } else {
            0.0
        }
    );
    k
}

fn generate_mir_(class_name: &str) -> Result<MirCodeBlock, Error> {
    let mut cl = ClassLoader::new();
    for file in std::fs::read_dir("std").unwrap() {
        cl.load_string(
            &std::fs::read_to_string(file.as_ref().unwrap().path()).unwrap(),
            &file
                .as_ref()
                .unwrap()
                .path()
                .as_os_str()
                .to_str()
                .unwrap()
                .to_owned(),
        )?;
    }
    load_natives(&mut cl);

    let rs = cl
        .view(&Type::simple(class_name, Span::default()))?
        .method_view(&SpannedObject(Span::default(), "main".to_owned()), &None)?
        .execute(&mut LocalState::new(), &mut CodeManager::new(cl), vec![])?;
    let mut mir = rs.mir;
    mir.add_mir(Mir::Stop);
    Ok(mir)
}

fn generate_mir(class_name: &str) -> MirCodeBlock {
    let r = generate_mir_(class_name);
    match r {
        Ok(e) => e,
        Err(e) => {
            report(e);
            exit(0);
        }
    }
}
