use either::Either;

use crate::compiler::{
    class_loader::ClassLoader,
    mir::{Mir, MirCodeBlock},
    state::{output_data::OutputData, typed_definition::TypedMemory},
};

pub fn implement(cl: &mut ClassLoader) {
    cl.implement_native("System", "debug", |ls, _cm, _mv| {
        let k = ls.get_var_native("a")?;
        println!("----DEBUGING----");
        println!("Type: {:?}", k.ty);
        println!("Locations: {:?}", k.locations);
        println!("----------------");
        Ok(OutputData::native(MirCodeBlock::default(), None))
    });
    cl.implement_native("System", "getRegister", |_ls, cm, mv| {
        let asp = cm.alloc();
        Ok(OutputData::native(
            MirCodeBlock(vec![Mir::ReadRegister(
                asp,
                mv.template.as_ref().unwrap().1[0].name.1.parse().unwrap(),
            )]),
            Some(TypedMemory::native(
                mv.return_type.as_ref().unwrap().clone(),
                vec![asp],
            )),
        ))
    });
    cl.implement_native("System", "setRegister", |ls, _cm, mv| {
        Ok(OutputData::native(
            MirCodeBlock(vec![Mir::WriteRegister(
                mv.template.as_ref().unwrap().1[0].name.1.parse().unwrap(),
                Either::Right(ls.get_var_native("value")?.locations[0]),
            )]),
            None,
        ))
    });
}
