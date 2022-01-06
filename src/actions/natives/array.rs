use errors::{index_out_of_bounds, Span, SpannedObject};
use mir::{Mir, MirCodeBlock};

use crate::compiler::{
    class_loader::ClassLoader,
    state::{local_state::LocalState, output_data::OutputData, typed_definition::TypedMemory},
};

pub fn implement(cl: &mut ClassLoader) {
    cl.implement_native("Array", "setDyn", |ls, cm, mv| {
        let size: u32 = mv.arguments[0].0.get_template()?.1[1].as_number()?;
        let ty = &mv.arguments[0].0.get_template()?.1[0];
        let index_ty = &mv.arguments[0].0.get_template()?.1[2];

        // Size of an item of the list
        let unit_size = cm.cl.view(ty)?.size(&cm.cl)?;

        // Size of the list indexer
        let mpos = cm.alloc_block(cm.cl.view(index_ty)?.size(&cm.cl)? as usize);
        let mtypedmemory = TypedMemory::new(index_ty.clone(), mpos.clone(), Span::default());

        let mut mircb = MirCodeBlock::default();

        mircb.copy_bulk(&mpos, &ls.get_var_native("index")?.locations);

        let mut ifcontainer = MirCodeBlock::default();

        let from = ls.get_var_native("value")?.locations.clone();
        for position in 0..size {
            // Get the position to copy to
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
            cb.add_mir(Mir::Skip);
            ifcontainer.add(mpos.iter().fold(cb, |a, b| {
                MirCodeBlock::from(Mir::If0(*b, a, MirCodeBlock::default()))
            }));
            let output_data = cm
                .cl
                .view(index_ty)?
                .method_view(&SpannedObject::native("dec".to_string()), &None)?
                .execute(&mut LocalState::new(), cm, vec![mtypedmemory.clone()])?;
            ifcontainer.add(output_data.mir);
        }
        mircb.add_mir(Mir::Block(ifcontainer));

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
        let index_ty = &mv.arguments[0].0.get_template()?.1[2];

        // Size of an item of the list
        let unit_size = cm.cl.view(ty)?.size(&cm.cl)?;

        // Size of the list indexer
        let mpos = cm.alloc_block(cm.cl.view(index_ty)?.size(&cm.cl)? as usize);
        let mtypedmemory = TypedMemory::new(index_ty.clone(), mpos.clone(), Span::default());

        let mut mircb = MirCodeBlock::default();
        mircb.copy_bulk(&mpos, &ls.get_var_native("index")?.locations);

        let to = cm.alloc_block(unit_size as usize);
        let mut ifcontainer = MirCodeBlock::default();
        for position in 0..size {
            let from = ls
                .get_var_native("self")?
                .locations
                .iter()
                .skip((unit_size * position) as usize)
                .take(unit_size as usize)
                .copied()
                .collect::<Vec<_>>();
            let mut cb = MirCodeBlock::default();
            cb.copy_bulk(&to, &from).add_mir(Mir::Skip);
            ifcontainer.add(mpos.iter().fold(cb, |a, b| {
                MirCodeBlock::from(Mir::If0(*b, a, MirCodeBlock::default()))
            }));
            let output_data = cm
                .cl
                .view(index_ty)?
                .method_view(&SpannedObject::native("dec".to_string()), &None)?
                .execute(&mut LocalState::new(), cm, vec![mtypedmemory.clone()])?;
            ifcontainer.add(output_data.mir);
        }
        mircb.add_mir(Mir::Block(ifcontainer));

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
        let mut len: usize = mv.arguments[0].0.get_template()?.1[1].as_number()? as usize;
        let ltylen = &mv.arguments[0].0.get_template()?.1[2];
        let mpos = cm.alloc_block(cm.cl.view(&ltylen)?.size(&cm.cl)? as usize);
        let mut k = MirCodeBlock::default();
        mpos.iter().for_each(|v| {
            k.set(*v, (len % 16) as u8);
            len /= 16;
        });
        Ok(OutputData::native(
            k,
            Some(TypedMemory::native(ltylen.clone(), mpos)),
        ))
    });
}
