#![feature(box_syntax)]
#![feature(try_blocks)]

use std::{ops::Range, rc::Rc};

use ariadne::{Config, Label, Report, ReportKind, Source};
use compiler::{
    asm::Context, asm_interpreter::MemoryState, mir::MirState, template::Template, ClassLoader,
};

use cythan::{Cythan, InterruptedCythan};
use either::Either;

use crate::{
    compiler::{
        compiler_states::{CodeManager, LocalState, OutputData, TypedMemory},
        mir::{Mir, MirCodeBlock},
    },
    errors::Span,
    parser::{
        class::Class,
        method::Method,
        ty::{TemplateDefinition, Type},
    },
};

mod compiler;
mod errors;
mod mir_utils;
mod parser;

fn main() {
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
            );
        }
        cl.get_class_mut("Val").get_method_mut("dec").code =
            Either::Right(Rc::new(Box::new(|ls, _cm, _mv| {
                let mut mir = MirCodeBlock::new();
                let loc = ls.get_var("self").unwrap().locations[0];
                mir.add_mir(Mir::Decrement(loc));
                OutputData::new(mir, None)
            })));
        cl.get_class_mut("Val").get_method_mut("inc").code =
            Either::Right(Rc::new(Box::new(|ls, _cm, _mv| {
                let mut mir = MirCodeBlock::new();
                let loc = ls.get_var("self").unwrap().locations[0];
                mir.add_mir(Mir::Increment(loc));
                OutputData::new(mir, None)
            })));
        cl.get_class_mut("System").get_method_mut("debug").code =
            Either::Right(Rc::new(Box::new(|ls, _cm, _mv| {
                let k = ls.get_var("a").unwrap();
                println!("----DEBUGING----");
                println!("Type: {:?}", k.ty);
                println!("Locations: {:?}", k.locations);
                println!("----------------");
                OutputData::new(MirCodeBlock::new(), None)
            })));
        cl.get_class_mut("System")
            .get_method_mut("getRegister")
            .code = Either::Right(Rc::new(Box::new(|_ls, cm, mv| {
            let asp = cm.alloc();
            OutputData::new(
                MirCodeBlock(vec![Mir::ReadRegister(
                    asp,
                    mv.template.as_ref().unwrap().1[0].name.1[1..]
                        .parse()
                        .unwrap(),
                )]),
                Some(TypedMemory::new(
                    mv.return_type.as_ref().unwrap().clone(),
                    vec![asp],
                )),
            )
        })));
        cl.get_class_mut("System")
            .get_method_mut("setRegister")
            .code = Either::Right(Rc::new(Box::new(|ls, _cm, mv| {
            OutputData::new(
                MirCodeBlock(vec![Mir::WriteRegister(
                    mv.template.as_ref().unwrap().1[0].name.1[1..]
                        .parse()
                        .unwrap(),
                    Either::Right(ls.get_var("value").unwrap().locations[0]),
                )]),
                None,
            )
        })));
        cl.get_class_mut("Array").get_method_mut("setDyn").code =
            Either::Right(Rc::new(Box::new(|ls, cm, mv| {
                //let position: u32 = mv.template.as_ref().unwrap()[0].name[1..].parse().unwrap();
                let size: u32 = mv.arguments[0].0.template.as_ref().unwrap().1[1].name.1[1..]
                    .parse()
                    .unwrap();
                let ty = &mv.arguments[0].0.template.as_ref().unwrap().1[0];
                let unit_size = cm.cl.view(ty).size(&cm.cl);
                let mpos = cm.alloc();
                let mut mircb = MirCodeBlock::new();
                mircb.copy(mpos, ls.get_var("index").unwrap().locations[0]);
                let from = ls.get_var("value").unwrap().locations.clone();
                let mut mir = MirCodeBlock::new();
                for position in (0..size).rev() {
                    let to = ls
                        .get_var("self")
                        .unwrap()
                        .locations
                        .iter()
                        .skip((unit_size * position) as usize)
                        .take(unit_size as usize)
                        .copied()
                        .collect::<Vec<_>>();
                    let mut cb = MirCodeBlock::new();
                    cb.copy_bulk(&to, &from);
                    mir.0.insert(0, Mir::Decrement(mpos));
                    mir = MirCodeBlock::from(vec![Mir::If0(mpos, cb, mir)]);
                }
                mircb.add(mir);

                OutputData::new(mircb, None)
            })));
        cl.get_class_mut("Array").get_method_mut("get").code =
            Either::Right(Rc::new(Box::new(|ls, cm, mv| {
                let position: u32 = mv.template.as_ref().unwrap().1[0].name.1[1..]
                    .parse()
                    .unwrap();
                let size: u32 = mv.arguments[0].0.template.as_ref().unwrap().1[1].name.1[1..]
                    .parse()
                    .unwrap();
                if position >= size {
                    panic!("Index out of bounds");
                }

                let ty = &mv.arguments[0].0.template.as_ref().unwrap().1[0];
                let unit_size = cm.cl.view(ty).size(&cm.cl);

                let data_loc = ls.get_var("self").unwrap();

                let ps = data_loc
                    .locations
                    .iter()
                    .skip((unit_size * position) as usize)
                    .take(unit_size as usize)
                    .copied()
                    .collect::<Vec<_>>();

                OutputData::new(MirCodeBlock::new(), Some(TypedMemory::new(ty.clone(), ps)))
            })));
        cl.get_class_mut("Array").get_method_mut("getDyn").code =
            Either::Right(Rc::new(Box::new(|ls, cm, mv| {
                //let position: u32 = mv.template.as_ref().unwrap()[0].name[1..].parse().unwrap();
                let size: u32 = mv.arguments[0].0.template.as_ref().unwrap().1[1].name.1[1..]
                    .parse()
                    .unwrap();
                let ty = &mv.arguments[0].0.template.as_ref().unwrap().1[0];
                let unit_size = cm.cl.view(ty).size(&cm.cl);
                let mpos = cm.alloc();
                let mut mircb = MirCodeBlock::new();
                mircb.copy(mpos, ls.get_var("index").unwrap().locations[0]);
                let to = cm.alloc_block(unit_size as usize);
                let mut mir = MirCodeBlock::new();
                for position in (0..size).rev() {
                    let from = ls
                        .get_var("self")
                        .unwrap()
                        .locations
                        .iter()
                        .skip((unit_size * position) as usize)
                        .take(unit_size as usize)
                        .copied()
                        .collect::<Vec<_>>();
                    let mut cb = MirCodeBlock::new();
                    cb.copy_bulk(&to, &from);
                    mir.0.insert(0, Mir::Decrement(mpos));
                    mir = MirCodeBlock::from(vec![Mir::If0(mpos, cb, mir)]);
                }
                mircb.add(mir);

                OutputData::new(mircb, Some(TypedMemory::new(ty.clone(), to)))
            })));
        cl.get_class_mut("Array").get_method_mut("set").code =
            Either::Right(Rc::new(Box::new(|ls, cm, mv| {
                let position: u32 = mv.template.as_ref().unwrap().1[0].name.1[1..]
                    .parse()
                    .unwrap();
                let size: u32 = mv.arguments[0].0.template.as_ref().unwrap().1[1].name.1[1..]
                    .parse()
                    .unwrap();
                if position >= size {
                    panic!("Index out of bounds");
                }

                let ty = &mv.arguments[0].0.template.as_ref().unwrap().1[0];
                let unit_size = cm.cl.view(ty).size(&cm.cl);

                let data_loc = ls.get_var("self").unwrap();

                let mut mir = MirCodeBlock::new();

                let to = data_loc
                    .locations
                    .iter()
                    .skip((unit_size * position) as usize)
                    .take(unit_size as usize)
                    .copied()
                    .collect::<Vec<_>>();

                let from = &ls.get_var("value").unwrap().locations;

                mir.copy_bulk(&to, from);

                OutputData::new(mir, None)
            })));
        let rs = cl
            .view(&Type::simple(
                "Counter",
                Span {
                    file: "<empty>".to_owned(),
                    start: 0,
                    end: 0,
                },
            ))
            .method_view("main", &None)
            .execute(&mut LocalState::new(), &mut CodeManager::new(cl), vec![])?;
        let mut mir = rs.mir;
        mir.add_mir(Mir::WriteRegister(1, Either::Left(3u8)));
        mir.add_mir(Mir::WriteRegister(
            2,
            Either::Right(rs.return_value.unwrap().locations[0]),
        ));
        mir.add_mir(Mir::WriteRegister(0, Either::Left(1u8)));
        compile_and_run(&mir);
    };
    if let Err(e) = r {
        let file_name = MirrorReport::from(&e).location.0.clone();
        e.eprint((
            file_name.clone(),
            Source::from(std::fs::read_to_string(&file_name).unwrap()),
        ))
        .unwrap();
    }
}

pub struct MirrorReport<S: ariadne::Span = Range<usize>> {
    kind: ReportKind,
    code: Option<u32>,
    msg: Option<String>,
    note: Option<String>,
    location: (<S::SourceId as ToOwned>::Owned, usize),
    labels: Vec<Label<S>>,
    config: Config,
}

impl<S: ariadne::Span> MirrorReport<S> {
    fn from(r: &Report<S>) -> &Self {
        unsafe { std::mem::transmute(r) }
    }
}

const MIR_MODE: bool = false;

fn compile_and_run(mir: &MirCodeBlock) {
    /* println!(
        "{}",
        mir.0
            .iter()
            .map(|m| format!("{}", m))
            .collect::<Vec<_>>()
            .join("\n")
    ); */
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
        MemoryState::new(2048, 8).execute_block(mir);
    } else {
        let mut mirstate = MirState::default();
        mir.to_asm(&mut mirstate);
        let mut compile_state = Template::default();
        let mut ctx = Context::default();
        mirstate
            .instructions
            .iter()
            .for_each(|i| i.compile(&mut compile_state, &mut ctx));
        let k = compile_state.build();
        std::fs::write("out.ct", &k).unwrap();
        let k = cythan_compiler::compile(&k).unwrap();
        let mut machine = InterruptedCythan::new_stdio(k, 4, 2 * 2_usize.pow(4 /* base */) + 3);
        'a: loop {
            for _ in 0..10000 {
                machine.next();
            }

            std::fs::write(
                "out.txt",
                machine
                    .cases
                    .iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
            )
            .unwrap();

            let o = machine.cases.clone();

            for _ in 0..10 {
                machine.next();

                if o == machine.cases {
                    break 'a;
                }
            }
        }
    }
}

#[test]
fn test() {
    compile_and_run(&MirCodeBlock(vec![Mir::WriteRegister(
        0,
        Either::Left(1u8),
    )]));
}
