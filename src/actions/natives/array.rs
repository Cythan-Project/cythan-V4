use crate::{
    compiler::{
        class_loader::ClassLoader,
        mir::{Mir, MirCodeBlock},
        state::{output_data::OutputData, typed_definition::TypedMemory},
    },
    errors::index_out_of_bounds,
    parser::ty::Type,
};

pub fn implement(cl: &mut ClassLoader) {
    cl.implement_native("Array", "setDyn", |ls, cm, mv| {
        let size: u32 = mv.arguments[0].0.get_template()?.1[1].as_number()?;
        let ty = &mv.arguments[0].0.get_template()?.1[0];
        let unit_size = cm.cl.view(ty)?.size(&cm.cl)?;
        let mpos = cm.alloc();
        let mut mircb = MirCodeBlock::default();
        mircb.copy(mpos, ls.get_var_native("index")?.locations[0]);
        let from = ls.get_var_native("value")?.locations.clone();
        let mut mir = MirCodeBlock::default();
        for position in (0..size).rev() {
            let to = ls
                .get_var_native("self")?
                .locations
                .iter()
                .skip((unit_size * position) as usize)
                .take(unit_size as usize)
                .copied()
                .collect::<Vec<_>>();
            let mut cb = MirCodeBlock::default();
            cb.copy_bulk(&to, &from);
            mir.0.insert(0, Mir::Decrement(mpos));
            mir = MirCodeBlock::from(vec![Mir::If0(mpos, cb, mir)]);
        }
        mircb.add(mir);

        Ok(OutputData::native(mircb, None))
    });
    cl.implement_native("Array", "get", |ls, cm, mv| {
        let position: u32 = mv.get_template()?.1[0].as_number()?;
        let size: u32 = mv.arguments[0].0.get_template()?.1[1].as_number()?;
        if position >= size {
            return Err(index_out_of_bounds(
                position as usize,
                size as usize,
                &mv.get_template()?.1[0].span,
                &mv.arguments[0].0.get_template()?.1[1].span,
            ));
        }

        let ty = &mv.arguments[0].0.get_template()?.1[0];
        let unit_size = cm.cl.view(ty)?.size(&cm.cl)?;

        let data_loc = ls.get_var_native("self")?;

        let ps = data_loc
            .locations
            .iter()
            .skip((unit_size * position) as usize)
            .take(unit_size as usize)
            .copied()
            .collect::<Vec<_>>();

        Ok(OutputData::native(
            MirCodeBlock::default(),
            Some(TypedMemory::native(ty.clone(), ps)),
        ))
    });
    cl.implement_native("Array", "getDyn", |ls, cm, mv| {
        let size: u32 = mv.arguments[0].0.get_template()?.1[1].as_number()?;
        let ty = &mv.arguments[0].0.template.as_ref().unwrap().1[0];
        let unit_size = cm.cl.view(ty)?.size(&cm.cl)?;
        let mpos = cm.alloc();
        let mut mircb = MirCodeBlock::default();
        mircb.copy(mpos, ls.get_var_native("index")?.locations[0]);
        let to = cm.alloc_block(unit_size as usize);
        let mut mir = MirCodeBlock::default();
        for position in (0..size).rev() {
            let from = ls
                .get_var_native("self")?
                .locations
                .iter()
                .skip((unit_size * position) as usize)
                .take(unit_size as usize)
                .copied()
                .collect::<Vec<_>>();
            let mut cb = MirCodeBlock::default();
            cb.copy_bulk(&to, &from);
            mir.0.insert(0, Mir::Decrement(mpos));
            mir = MirCodeBlock::from(vec![Mir::If0(mpos, cb, mir)]);
        }
        mircb.add(mir);

        Ok(OutputData::native(
            mircb,
            Some(TypedMemory::native(ty.clone(), to)),
        ))
    });

    cl.implement_native("Array", "set", |ls, cm, mv| {
        let position: u32 = mv.get_template()?.1[0].as_number()?;
        let size: u32 = mv.arguments[0].0.get_template()?.1[1].as_number()?;
        if position >= size {
            return Err(index_out_of_bounds(
                position as usize,
                size as usize,
                &mv.get_template()?.1[0].span,
                &mv.arguments[0].0.get_template()?.1[1].span,
            ));
        }

        let ty = &mv.arguments[0].0.get_template()?.1[0];
        let unit_size = cm.cl.view(ty)?.size(&cm.cl)?;

        let data_loc = ls.get_var_native("self")?;

        let mut mir = MirCodeBlock::default();

        let to = data_loc
            .locations
            .iter()
            .skip((unit_size * position) as usize)
            .take(unit_size as usize)
            .copied()
            .collect::<Vec<_>>();

        let from = &ls.get_var_native("value")?.locations;

        mir.copy_bulk(&to, from);

        Ok(OutputData::native(mir, None))
    });
    cl.implement_native("Array", "len", |_ls, cm, mv| {
        let len: usize = mv.arguments[0].0.get_template()?.1[1].as_number()? as usize;
        let alloc = cm.alloc();

        Ok(OutputData::native(
            MirCodeBlock(vec![Mir::Set(alloc, len as u8)]),
            Some(TypedMemory::native(Type::native_simple("Val"), vec![alloc])),
        ))
    });
}
