use errors::{
    method_return_type_invalid, report_similar, Error, Span, SpannedObject, SpannedVector,
};
use mir::{Mir, MirCodeBlock};

use crate::{
    compiler::state::typed_definition::{CheckAgainst, TypedMemory},
    parser::{
        expression::{BooleanOperator, Expr},
        ty::Type,
        NumberType,
    },
};

use super::state::{code_manager::CodeManager, local_state::LocalState, output_data::OutputData};

pub fn compile_code_block(
    expr: &SpannedVector<Expr>,
    ls: &mut LocalState,
    cm: &mut CodeManager,
    span: Span,
) -> Result<OutputData, Error> {
    expr.1.iter().fold(
        Ok(OutputData::new(MirCodeBlock::default(), span, None)),
        |acc, expr| {
            let mut acc = acc?;
            let expr = compile(expr, ls, cm, None)?;
            acc.mir.add(expr.mir);
            acc.return_value = expr.return_value;
            Ok(acc)
        },
    )
}

pub fn compile(
    expr: &Expr,
    ls: &mut LocalState,
    cm: &mut CodeManager,
    expected_type: Option<Type>,
) -> Result<OutputData, Error> {
    let mut mir = MirCodeBlock::default();
    match expr {
        Expr::New {
            span,
            class,
            fields,
        } => {
            // TODO: Set expected type to correct value.
            let class = class.apply_expected(&expected_type);
            let view = cm.cl.view(&class)?;
            let fields = fields
                .1
                .iter()
                .map(|(a, b)| {
                    Ok((
                        a,
                        compile(b, ls, cm, Some(view.get_field_type(a, b.span())?))?,
                    ))
                })
                .collect::<Result<Vec<_>, _>>()?;
            // get_field

            let instr = view
                .fields
                .iter()
                .map(|x| {
                    let field = if let Some(e) = fields.iter().find(|(a, _)| *a == &x.name.1) {
                        e
                    } else {
                        return Err(report_similar(
                            "field",
                            "fields",
                            span,
                            &x.name.1,
                            &view
                                .fields
                                .iter()
                                .map(|x| x.name.1.clone())
                                .collect::<Vec<_>>(),
                            14,
                        ));
                    };
                    Ok(field
                        .1
                        .return_value
                        .as_ref()
                        .expect("Argument is not a value")
                        .locations
                        .clone())
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect();
            Ok(OutputData::new(
                MirCodeBlock::from(
                    fields
                        .into_iter()
                        .map(|x| x.1.mir.0)
                        .flatten()
                        .collect::<Vec<_>>(),
                ),
                span.clone(),
                Some(TypedMemory::new(class, instr, span.clone())),
            ))
        }
        Expr::If {
            span,
            condition,
            then,
            or_else,
        } => {
            let er = compile(condition, ls, cm, None)?;
            let loc = er.check_against(&Type::native_simple("Bool"))?[0];
            mir.add(er.mir);

            let then_r = compile_code_block(then, &mut ls.shadow(), cm, span.clone())?;
            let else_r = if let Some(x) = or_else.as_ref() {
                Some(compile_code_block(x, &mut ls.shadow(), cm, span.clone())?)
            } else {
                None
            };
            let (output, tlr, elr) = if let Some(else_r) = else_r {
                if let Some(b) = then_r.return_value {
                    let a = else_r
                        .return_value
                        .expect("Else should have a return value");
                    if a.ty != b.ty {
                        panic!("If branches must have the same type {}", span.file);
                    }
                    let alc = cm.alloc_type(&a.ty)?;
                    let mut tr = then_r.mir;
                    tr.copy_bulk(&alc, &b.locations);
                    let mut er = else_r.mir;
                    er.copy_bulk(&alc, &a.locations);

                    (Some(TypedMemory::new(a.ty, alc, span.clone())), tr, er)
                } else {
                    (None, then_r.mir, else_r.mir)
                }
            } else {
                (None, then_r.mir, MirCodeBlock::default())
            };
            mir.add_mir(Mir::If0(loc, tlr, elr));
            Ok(OutputData::new(mir, span.clone(), output))
        }
        Expr::Number(span, a, t) => {
            // TODO add more options (type choice automatically)
            let (tn, alc) = if t == &NumberType::Val
                || (t == &NumberType::Auto && *a < 16 && *a >= 0)
            {
                let alc = cm.alloc();
                mir.add_mir(Mir::Set(alc, *a as u8));
                ("Val", vec![alc])
            } else if t == &NumberType::Byte || (t == &NumberType::Auto && *a < 16 * 16 && *a > 0) {
                let alc = cm.alloc();
                let alc1 = cm.alloc();
                mir.add_mir(Mir::Set(alc, *a as u8 % 16));
                mir.add_mir(Mir::Set(alc1, *a as u8 / 16));
                ("Byte", vec![alc, alc1])
            } else if t == &NumberType::Short
                || (t == &NumberType::Auto && *a < 16 * 16 * 16 * 16 && *a > 0)
            {
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
            Ok(OutputData::new(
                mir,
                span.clone(),
                Some(TypedMemory::new(
                    Type::simple(tn, span.clone()),
                    alc,
                    span.clone(),
                )),
            ))
        }
        Expr::Variable(span, a) => Ok(OutputData::new(
            mir,
            span.clone(),
            Some(ls.get_var(&SpannedObject(span.clone(), a.clone()))?.clone()),
        )),
        Expr::Type(_span, _a) => {
            panic!("Expected something else than Type in expression")
        }
        Expr::Field { span, source, name } => {
            let out = compile(&*source, ls, cm, None)?;
            let rtv = out
                .return_value
                .as_ref()
                .expect("Field source must be a value");
            let (ty, locs) = cm.location_and_type_of_field(
                &rtv.locations,
                cm.cl.view(&rtv.ty)?,
                &SpannedObject(span.clone(), name.clone()),
            )?;
            Ok(OutputData::new(
                out.mir,
                span.clone(),
                Some(TypedMemory::new(ty, locs, span.clone())),
            ))
        }
        Expr::Method {
            span,
            source,
            name,
            arguments,
            template,
        } => {
            if let Expr::Type(_tspan, a) = &**source {
                let a = a.apply_expected(&expected_type);
                let arguments = arguments
                    .1
                    .iter()
                    .map(|x| {
                        let k = compile(x, ls, cm, None)?;
                        mir.add(k.mir);
                        Ok(k.return_value.expect("Argument must be a value"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let k = cm
                    .cl
                    .view(&a)?
                    .method_view(name, template)?
                    .execute(ls, cm, arguments)?;
                mir.add(k.mir);
                Ok(OutputData::new(
                    mir,
                    span.clone(),
                    k.return_value.map(|mut x| {
                        x.span = span.clone();
                        x
                    }),
                ))
            } else {
                let aj = compile(source, ls, cm, None)?;
                mir.add(aj.mir);
                let ah = aj.return_value.expect("Method source must be a value");
                let a = &ah.ty;
                let mut arguments = arguments
                    .1
                    .iter()
                    .map(|x| {
                        let k = compile(x, ls, cm, None)?;
                        mir.add(k.mir);
                        Ok(k.return_value.expect("Argument must be a value"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                arguments.insert(0, ah.clone());
                let k = cm
                    .cl
                    .view(a)?
                    .method_view(name, template)?
                    .execute(ls, cm, arguments)?;
                mir.add(k.mir);
                Ok(OutputData::new(
                    mir,
                    span.clone(),
                    k.return_value.map(|mut x| {
                        x.span = span.clone();
                        x
                    }),
                ))
            }
        }
        Expr::NamedResource { span, vtype, name } => {
            let vtype = vtype.apply_expected(&expected_type);
            let k = ls.new_var(cm, &name.1, vtype, &mut mir, span.clone())?;
            Ok(OutputData::new(mir, span.clone(), Some(k)))
        }
        Expr::Assignement { span, target, to } => {
            let ret = compile(target, ls, cm, None)?;
            let rt = ret
                .return_value
                .expect("Assignement target must be a value");
            let ret1 = compile(to, ls, cm, Some(rt.ty.clone()))?;
            let rt1 = ret1
                .return_value
                .expect("Assignement value must be a value");
            if rt.ty != rt1.ty {
                panic!("Assignement types must match");
            }
            mir.add(ret1.mir);
            mir.add(ret.mir);
            mir.copy_bulk(&rt.locations, &rt1.locations);
            Ok(OutputData::new(mir, span.clone(), None))
        }
        Expr::Block(span, a) => compile_code_block(a, &mut ls.shadow(), cm, span.clone()),
        Expr::Return(span, a) => {
            if let Some(e) = a {
                let rl = ls
                    .return_loc
                    .as_ref()
                    .expect("A return value wasn't expected")
                    .clone();
                let ret = compile(e, ls, cm, Some(rl.ty.clone()))?;
                let rt = ret.return_value.expect("Return value must be a value");
                if rl.ty != rt.ty {
                    return Err(method_return_type_invalid(
                        e.span(),
                        &rl.ty.span,
                        &format!("{:?}", rl.ty),
                        &format!("{:?}", rt.ty),
                    ));
                }
                mir.add(ret.mir);
                mir.copy_bulk(&rl.locations, &rt.locations);
                mir.add_mir(Mir::Skip);
                Ok(OutputData::new(mir, span.clone(), None))
            } else if ls.return_loc.is_some() {
                panic!("A return value was expected");
            } else {
                mir.add_mir(Mir::Skip);
                Ok(OutputData::new(mir, span.clone(), None))
            }
        }
        Expr::Cast {
            span,
            source,
            target,
        } => {
            let target = target.apply_expected(&expected_type);
            let ret = compile(source, ls, cm, Some(target.clone()))?;
            mir.add(ret.mir);
            let rt = ret.return_value.expect("Cast source must be a value");
            let source_view = cm.cl.view(&rt.ty)?;
            let target_view = cm.cl.view(&target)?;
            if source_view.size(&cm.cl)? != target_view.size(&cm.cl)? {
                panic!("Type size cast must match, {:?} and {:?}", source, target);
            }
            Ok(OutputData::new(
                mir,
                span.clone(),
                Some(TypedMemory::new(target, rt.locations, span.clone())),
            ))
        }
        Expr::Loop(span, a) => {
            let k = compile_code_block(a, &mut ls.shadow(), cm, span.clone())?;
            mir.add_mir(Mir::Loop(k.mir));
            Ok(OutputData::new(mir, span.clone(), None))
        }
        Expr::Break(span) => Ok(OutputData::new(
            MirCodeBlock::from(vec![Mir::Break]),
            span.clone(),
            None,
        )),
        Expr::Continue(span) => Ok(OutputData::new(
            MirCodeBlock::from(vec![Mir::Continue]),
            span.clone(),
            None,
        )),
        Expr::BooleanExpression(span, a, bo, c) => {
            let alp = cm.alloc();
            let a = compile(a, ls, cm, None)?;
            let loca = if let Some(a) = a.return_value {
                if a.ty.name.1 != "Bool" {
                    panic!("Boolean expression must be a bool");
                }
                a.locations[0]
            } else {
                panic!("Boolean expression must return a value");
            };
            let b = compile(c, ls, cm, None)?;
            let locb = if let Some(b) = b.return_value {
                if b.ty.name.1 != "Bool" {
                    panic!("Boolean expression must be a bool");
                }
                b.locations[0]
            } else {
                panic!("Boolean expression must return a value");
            };
            mir.add(a.mir);
            let mut cb = MirCodeBlock::default();
            cb.add(b.mir);
            cb.copy(alp, locb);
            if bo == &BooleanOperator::And {
                mir.add_mir(Mir::If0(
                    loca,
                    cb,
                    MirCodeBlock::from(vec![Mir::Set(alp, 1)]),
                ));
            } else {
                mir.add_mir(Mir::If0(
                    loca,
                    MirCodeBlock::from(vec![Mir::Set(alp, 0)]),
                    cb,
                ));
            }
            Ok(OutputData::new(
                mir,
                span.clone(),
                Some(TypedMemory::new(
                    Type::simple("Bool", span.clone()),
                    vec![alp],
                    span.clone(),
                )),
            ))
        }
        Expr::ArrayDefinition(a, b) => {
            let out1 =
                b.1.iter()
                    .map(|x| compile(x, ls, cm, None))
                    .collect::<Result<Vec<_>, _>>()?;
            let fe = out1.first().expect("Array must have at least one element");
            let rv = fe
                .return_value
                .as_ref()
                .expect("Array must have at least one element")
                .ty
                .clone();
            let mut alloc_block = Vec::new();
            let m = out1.len();
            for i in out1 {
                let mut rv1 = i
                    .return_value
                    .expect("Array must have at least one element");
                if rv1.ty != rv {
                    panic!("Array elements must be the same type");
                }
                mir.add(i.mir);
                alloc_block.append(&mut rv1.locations);
            }
            let az = cm.alloc_block(alloc_block.len());
            mir.copy_bulk(&az, &alloc_block);
            Ok(OutputData::new(
                mir,
                a.clone(),
                Some(TypedMemory::new(
                    Type::new(
                        "Array",
                        Some(SpannedVector(
                            a.clone(),
                            vec![rv, Type::simple(&m.to_string(), a.clone())],
                        )),
                        a.clone(),
                    ),
                    az,
                    a.clone(),
                )),
            ))
        }
    }
}
