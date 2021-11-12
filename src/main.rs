#![feature(box_syntax)]
#![feature(try_blocks)]

use std::rc::Rc;

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
    let mut cl = ClassLoader::new();
    cl.load(Class {
        name: "Array".to_owned(),
        annotations: vec![],
        template: Some(TemplateDefinition(vec!["T".to_string(), "E".to_string()])),
        fields: vec![],
        methods: vec![
            Method {
                name: "set".to_owned(),
                annotations: vec![],
                return_type: None,
                arguments: vec![
                    (
                        Type {
                            name: "Array".to_owned(),
                            template: Some(vec![Type::simple("T"), Type::simple("E")]),
                        },
                        "self".to_owned(),
                    ),
                    (
                        Type {
                            name: "T".to_owned(),
                            template: None,
                        },
                        "a".to_owned(),
                    ),
                ],
                template: Some(TemplateDefinition(vec!["N".to_owned()])),
                code: Either::Right(Rc::new(Box::new(|ls, cm, mv| {
                    let position: u32 = mv.template.as_ref().unwrap()[0].name[1..].parse().unwrap();
                    let size: u32 = mv.arguments[0].0.template.as_ref().unwrap()[1].name[1..]
                        .parse()
                        .unwrap();
                    if position >= size {
                        panic!("Index out of bounds");
                    }

                    let ty = &mv.arguments[0].0.template.as_ref().unwrap()[0];
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

                    let from = &ls.get_var("a").unwrap().locations;

                    mir.copy_bulk(&to, from);

                    OutputData::new(mir, None)
                }))),
            },
            Method {
                name: "get".to_owned(),
                annotations: vec![],
                return_type: Some(Type {
                    name: "T".to_owned(),
                    template: None,
                }),
                arguments: vec![(
                    Type {
                        name: "Array".to_owned(),
                        template: Some(vec![Type::simple("T"), Type::simple("E")]),
                    },
                    "self".to_owned(),
                )],
                template: Some(TemplateDefinition(vec!["N".to_owned()])),
                code: Either::Right(Rc::new(Box::new(|ls, cm, mv| {
                    let position: u32 = mv.template.as_ref().unwrap()[0].name[1..].parse().unwrap();
                    let size: u32 = mv.arguments[0].0.template.as_ref().unwrap()[1].name[1..]
                        .parse()
                        .unwrap();
                    if position >= size {
                        panic!("Index out of bounds");
                    }

                    let ty = &mv.arguments[0].0.template.as_ref().unwrap()[0];
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
                }))),
            },
            Method {
                name: "setDyn".to_owned(),
                annotations: vec![],
                return_type: None,
                arguments: vec![
                    (
                        Type {
                            name: "Array".to_owned(),
                            template: Some(vec![Type::simple("T"), Type::simple("E")]),
                        },
                        "self".to_owned(),
                    ),
                    (
                        Type {
                            name: "Val".to_owned(),
                            template: None,
                        },
                        "pos".to_owned(),
                    ),
                    (
                        Type {
                            name: "T".to_owned(),
                            template: None,
                        },
                        "a".to_owned(),
                    ),
                ],
                template: None,
                code: Either::Right(Rc::new(Box::new(|ls, cm, mv| {
                    //let position: u32 = mv.template.as_ref().unwrap()[0].name[1..].parse().unwrap();
                    let size: u32 = mv.arguments[0].0.template.as_ref().unwrap()[1].name[1..]
                        .parse()
                        .unwrap();
                    let ty = &mv.arguments[0].0.template.as_ref().unwrap()[0];
                    let unit_size = cm.cl.view(ty).size(&cm.cl);
                    let mpos = cm.alloc();
                    let mut mircb = MirCodeBlock::new();
                    mircb.copy(mpos, ls.get_var("pos").unwrap().locations[0]);
                    let from = ls.get_var("a").unwrap().locations.clone();
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
                }))),
            },
            Method {
                name: "getDyn".to_owned(),
                annotations: vec![],
                return_type: Some(Type::simple("T")),
                arguments: vec![
                    (
                        Type {
                            name: "Array".to_owned(),
                            template: Some(vec![Type::simple("T"), Type::simple("E")]),
                        },
                        "self".to_owned(),
                    ),
                    (
                        Type {
                            name: "Val".to_owned(),
                            template: None,
                        },
                        "pos".to_owned(),
                    ),
                ],
                template: None,
                code: Either::Right(Rc::new(Box::new(|ls, cm, mv| {
                    //let position: u32 = mv.template.as_ref().unwrap()[0].name[1..].parse().unwrap();
                    let size: u32 = mv.arguments[0].0.template.as_ref().unwrap()[1].name[1..]
                        .parse()
                        .unwrap();
                    let ty = &mv.arguments[0].0.template.as_ref().unwrap()[0];
                    let unit_size = cm.cl.view(ty).size(&cm.cl);
                    let mpos = cm.alloc();
                    let mut mircb = MirCodeBlock::new();
                    mircb.copy(mpos, ls.get_var("pos").unwrap().locations[0]);
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
                }))),
            },
        ],
        superclass: None,
    });
    cl.load(Class {
        name: "System".to_owned(),
        annotations: vec![],
        template: None,
        fields: vec![],
        methods: vec![
            Method {
                name: "getRegister".to_owned(),
                annotations: vec![],
                return_type: Some(Type {
                    name: "Val".to_owned(),
                    template: None,
                }),
                arguments: vec![],
                template: Some(TemplateDefinition(vec!["N".to_owned()])),
                code: Either::Right(Rc::new(Box::new(|_ls, cm, mv| {
                    let asp = cm.alloc();
                    OutputData::new(
                        MirCodeBlock(vec![Mir::ReadRegister(
                            asp,
                            mv.template.as_ref().unwrap()[0].name[1..].parse().unwrap(),
                        )]),
                        Some(TypedMemory::new(Type::simple("Val"), vec![asp])),
                    )
                }))),
            },
            Method {
                name: "setRegister".to_owned(),
                annotations: vec![],
                return_type: None,
                arguments: vec![(
                    Type {
                        name: "Val".to_owned(),
                        template: None,
                    },
                    "a".to_owned(),
                )],
                template: Some(TemplateDefinition(vec!["N".to_owned()])),
                code: Either::Right(Rc::new(Box::new(|ls, _cm, mv| {
                    OutputData::new(
                        MirCodeBlock(vec![Mir::WriteRegister(
                            mv.template.as_ref().unwrap()[0].name[1..].parse().unwrap(),
                            Either::Right(ls.get_var("a").unwrap().locations[0]),
                        )]),
                        None,
                    )
                }))),
            },
            Method {
                name: "debug".to_owned(),
                annotations: vec![],
                return_type: None,
                arguments: vec![(
                    Type {
                        name: "T".to_owned(),
                        template: None,
                    },
                    "a".to_owned(),
                )],
                template: Some(TemplateDefinition(vec!["T".to_owned()])),
                code: Either::Right(Rc::new(Box::new(|ls, _cm, _mv| {
                    let k = ls.get_var("a").unwrap();
                    println!("----DEBUGING----");
                    println!("Type: {:?}", k.ty);
                    println!("Locations: {:?}", k.locations);
                    println!("----------------");
                    OutputData::new(MirCodeBlock::new(), None)
                }))),
            },
        ],
        superclass: None,
    });
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
    cl.inject_method(
        "Val",
        Method {
            name: "dec".to_owned(),
            annotations: vec![],
            return_type: None,
            arguments: vec![(Type::simple("Val"), "self".to_owned())],
            template: None,
            code: Either::Right(Rc::new(Box::new(|ls, _cm, _mv| {
                let mut mir = MirCodeBlock::new();
                let loc = ls.get_var("self").unwrap().locations[0];
                mir.add_mir(Mir::Decrement(loc));
                OutputData::new(mir, None)
            }))),
        },
    );
    cl.inject_method(
        "Val",
        Method {
            name: "inc".to_owned(),
            annotations: vec![],
            return_type: None,
            arguments: vec![(Type::simple("Val"), "self".to_owned())],
            template: None,
            code: Either::Right(Rc::new(Box::new(|ls, _cm, _mv| {
                let mut mir = MirCodeBlock::new();
                let loc = ls.get_var("self").unwrap().locations[0];
                mir.add_mir(Mir::Increment(loc));
                OutputData::new(mir, None)
            }))),
        },
    );
    let rs = cl
        .view(&Type::simple("Counter"))
        .method_view("main", &None)
        .execute(&mut LocalState::new(), &mut CodeManager::new(cl), vec![]);
    let mut mir = rs.mir;
    mir.add_mir(Mir::WriteRegister(1, Either::Left(3u8)));
    mir.add_mir(Mir::WriteRegister(
        2,
        Either::Right(rs.return_value.unwrap().locations[0]),
    ));
    mir.add_mir(Mir::WriteRegister(0, Either::Left(1u8)));
    compile_and_run(&mir);
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
