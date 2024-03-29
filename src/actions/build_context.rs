use std::process::exit;

use errors::{report, Error, Span, SpannedObject};
use mir::{Mir, MirCodeBlock};

use crate::{
    actions::natives::load_natives,
    compiler::{
        class_loader::ClassLoader,
        state::{code_manager::CodeManager, local_state::LocalState},
    },
    parser::ty::Type,
    STACK_SIZE,
};

pub fn compile(class_name: String, optimize: bool) -> MirCodeBlock {
    let child = std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(move || generate_mir(&class_name))
        .unwrap();
    let k = child.join().unwrap();
    std::fs::write(
        "before.mir",
        k.0.iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("Could not write file");
    let count = k.instr_count();
    let k = if optimize { k.optimize_code_new() } else { k };
    std::fs::write(
        "after.mir",
        k.0.iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("Could not write file");
    let ncount = k.instr_count();
    println!(
        "Optimized from {} to {} ({:.02}%)",
        count,
        ncount,
        (count - ncount) as f64 / count as f64 * 100.
    );
    k
}

fn generate_mir(class_name: &str) -> MirCodeBlock {
    let r: Result<(), Error> = try {
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
        return mir;
    };
    if let Err(e) = r {
        report(e);
        exit(0);
    };
    panic!();
}
