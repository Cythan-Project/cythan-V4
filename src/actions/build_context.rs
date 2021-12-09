use std::{ops::Range, process::exit};

use ariadne::Report;

use crate::{
    actions::natives::load_natives,
    compiler::{
        class_loader::ClassLoader,
        mir::{Mir, MirCodeBlock},
        state::{code_manager::CodeManager, local_state::LocalState},
    },
    errors::{reporting::report, Span},
    parser::{expression::SpannedObject, ty::Type},
    STACK_SIZE,
};

pub fn compile(class_name: String) -> MirCodeBlock {
    let child = std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(move || generate_mir(&class_name))
        .unwrap();
    child.join().unwrap()
}

fn generate_mir(class_name: &str) -> MirCodeBlock {
    let r: Result<(), Report<(String, Range<usize>)>> = try {
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
