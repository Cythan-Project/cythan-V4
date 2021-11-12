use crate::{
    compiler::compiler_states::TypedMemory,
    parser::{expression::Expr, ty::Type},
};

use super::{
    compiler_states::{CodeManager, LocalState, OutputData},
    mir::{Mir, MirCodeBlock},
};

pub fn compile_code_block(expr: &[Expr], ls: &mut LocalState, cm: &mut CodeManager) -> OutputData {
    expr.iter().fold(
        OutputData::new(MirCodeBlock::new(), None),
        |mut acc, expr| {
            let expr = compile(expr, ls, cm);
            acc.mir.add(expr.mir);
            acc.return_value = expr.return_value;
            acc
        },
    )
}

pub fn compile(expr: &Expr, ls: &mut LocalState, cm: &mut CodeManager) -> OutputData {
    let mut mir = MirCodeBlock::new();
    match expr {
        Expr::New { class, fields } => {
            let fields = fields
                .iter()
                .map(|(a, b)| (a, compile(b, ls, cm)))
                .collect::<Vec<_>>();
            let view = cm.cl.view(class);
            let instr = view
                .fields
                .iter()
                .map(|x| {
                    let field = fields
                        .iter()
                        .find(|(a, _)| *a == &x.name)
                        .expect("Can't find field");
                    field
                        .1
                        .return_value
                        .as_ref()
                        .expect("Argument is not a value")
                        .locations
                        .clone()
                })
                .flatten()
                .collect();
            OutputData::new(
                MirCodeBlock::from(fields.into_iter().map(|x| x.1.mir.0).flatten().collect()),
                Some(TypedMemory::new(class.clone(), instr)),
            )
        }
        Expr::If {
            condition,
            then,
            or_else,
        } => {
            //let alloc = ls.cm.alloc
            let er = compile(condition, ls, cm);
            let loc = if let Some(TypedMemory { locations, ty }) = &er.return_value {
                if locations.len() != 1 {
                    panic!("Condition must return a single value");
                }
                if ty.name != "Bool" {
                    panic!("Condition must return a bool");
                }
                locations[0]
            } else {
                panic!("Condition must return a single value");
            };
            mir.add(er.mir);

            let then_r = compile_code_block(then, &mut ls.shadow(), cm);
            let else_r = or_else
                .as_ref()
                .map(|x| compile_code_block(x, &mut ls.shadow(), cm));
            let (output, tlr, elr) = if let Some(else_r) = else_r {
                if let Some(b) = then_r.return_value {
                    let a = else_r
                        .return_value
                        .expect("Else should have a return value");
                    if a.ty != b.ty {
                        panic!("If branches must have the same type");
                    }
                    let alc = cm.alloc_type(&a.ty).unwrap();
                    let mut tr = then_r.mir;
                    tr.copy_bulk(&alc, &b.locations);
                    let mut er = else_r.mir;
                    er.copy_bulk(&alc, &a.locations);

                    (Some(TypedMemory::new(a.ty.clone(), alc)), tr, er)
                } else {
                    (None, then_r.mir, else_r.mir)
                }
            } else {
                (None, then_r.mir, MirCodeBlock::new())
            };
            mir.add_mir(Mir::If0(loc, tlr, elr));
            OutputData::new(mir, output)
        }
        Expr::Number(a) => {
            // TODO add more options (type choice automatically)
            let (tn, alc) = if *a < 16 && *a >= 0 {
                let alc = cm.alloc();
                mir.add_mir(Mir::Set(alc, *a as u8));
                ("Val", vec![alc])
            } else if *a < 16 * 16 && *a > 0 {
                let alc = cm.alloc();
                let alc1 = cm.alloc();
                mir.add_mir(Mir::Set(alc, *a as u8 % 16));
                mir.add_mir(Mir::Set(alc1, *a as u8 / 16));
                ("Byte", vec![alc, alc1])
            } else if *a < 16 * 16 * 16 * 16 && *a > 0 {
                let alc = cm.alloc();
                let alc1 = cm.alloc();
                let alc2 = cm.alloc();
                let alc3 = cm.alloc();
                let mut a = *a;
                mir.add_mir(Mir::Set(alc, a as u8 % 16));
                a /= 16;
                mir.add_mir(Mir::Set(alc1, a as u8 % 16));
                a /= 16;
                mir.add_mir(Mir::Set(alc2, a as u8 % 16));
                a /= 16;
                mir.add_mir(Mir::Set(alc3, a as u8 % 16));
                ("Short", vec![alc, alc1])
            } else {
                panic!("Number too big {}", a);
            };
            OutputData::new(
                mir,
                Some(TypedMemory::new(
                    Type {
                        name: tn.to_owned(),
                        template: None,
                    },
                    alc,
                )),
            )
        }
        Expr::Variable(a) => OutputData::new(
            mir,
            Some(ls.get_var(a).expect("Variable not found").clone()),
        ),
        Expr::Type(_a) => {
            panic!("Expected something else than Type in expression")
        }
        Expr::Field { source, name } => {
            let out = compile(&*source, ls, cm);
            let rtv = out
                .return_value
                .as_ref()
                .expect("Field source must be a value");
            let (ty, locs) =
                cm.location_and_type_of_field(&rtv.locations, cm.cl.view(&rtv.ty), name);
            OutputData::new(out.mir, Some(TypedMemory::new(ty, locs)))
        }
        Expr::Method {
            source,
            name,
            arguments,
            template,
        } => {
            if let Expr::Type(a) = &**source {
                let arguments = arguments
                    .iter()
                    .map(|x| {
                        let k = compile(x, ls, cm);
                        mir.add(k.mir);
                        k.return_value.expect("Argument must be a value")
                    })
                    .collect::<Vec<_>>();
                let k = cm
                    .cl
                    .view(a)
                    .method_view(name, template)
                    .execute(ls, cm, arguments);
                mir.add(k.mir);
                OutputData::new(mir, k.return_value)
            } else {
                let aj = compile(source, ls, cm);
                mir.add(aj.mir);
                let ah = aj.return_value.expect("Method source must be a value");
                let a = &ah.ty;
                let mut arguments = arguments
                    .iter()
                    .map(|x| {
                        let k = compile(x, ls, cm);
                        mir.add(k.mir);
                        k.return_value.expect("Argument must be a value")
                    })
                    .collect::<Vec<_>>();
                arguments.insert(0, ah.clone());
                let k = cm
                    .cl
                    .view(a)
                    .method_view(name, template)
                    .execute(ls, cm, arguments);
                mir.add(k.mir);
                OutputData::new(mir, k.return_value)
            }
        }
        Expr::NamedResource { vtype, name } => {
            let k = ls.new_var(cm, name, vtype.clone(), &mut mir);
            OutputData::new(mir, Some(k))
        }
        Expr::Assignement { target, to } => {
            let ret = compile(target, ls, cm);
            let ret1 = compile(to, ls, cm);
            let rt = ret
                .return_value
                .expect("Assignement target must be a value");
            let rt1 = ret1
                .return_value
                .expect("Assignement value must be a value");
            if rt.ty != rt1.ty {
                panic!("Assignement types must match");
            }
            mir.add(ret1.mir);
            mir.add(ret.mir);
            mir.copy_bulk(&rt.locations, &rt1.locations);
            OutputData::new(mir, None)
        }
        Expr::Block(a) => compile_code_block(a, &mut ls.shadow(), cm),
        Expr::Return(a) => {
            if let Some(e) = a {
                let ret = compile(e, ls, cm);
                let rl = ls
                    .return_loc
                    .as_ref()
                    .expect("A return value wasn't expected");
                let rt = ret.return_value.expect("Return value must be a value");
                if rl.ty != rt.ty {
                    panic!("Return type must match");
                }
                mir.add(ret.mir);
                mir.copy_bulk(&rl.locations, &rt.locations);
                mir.add_mir(Mir::Skip);
                OutputData::new(mir, None)
            } else if ls.return_loc.is_some() {
                panic!("A return value was expected");
            } else {
                mir.add_mir(Mir::Skip);
                OutputData::new(mir, None)
            }
        }
        Expr::Cast { source, target } => {
            let ret = compile(source, ls, cm);
            mir.add(ret.mir);
            let rt = ret.return_value.expect("Cast source must be a value");
            let source_view = cm.cl.view(&rt.ty);
            let target_view = cm.cl.view(&target);
            if source_view.size(&cm.cl) != target_view.size(&cm.cl) {
                panic!("Type size cast must match, {:?} and {:?}", source, target);
            }
            OutputData::new(mir, Some(TypedMemory::new(target.clone(), rt.locations)))
        }
        Expr::Loop(a) => {
            let k = compile_code_block(a, &mut ls.shadow(), cm);
            mir.add_mir(Mir::Loop(k.mir));
            OutputData::new(mir, None)
        }
        Expr::Break => OutputData::new(MirCodeBlock::from(vec![Mir::Break]), None),
        Expr::Continue => OutputData::new(MirCodeBlock::from(vec![Mir::Continue]), None),
    }
}
