use crate::compiler::{
    class_loader::ClassLoader,
    mir::{Mir, MirCodeBlock},
    state::output_data::OutputData,
};

pub fn implement(cl: &mut ClassLoader) {
    cl.implement_native("Val", "dec", |ls, _cm, _mv| {
        let mut mir = MirCodeBlock::default();
        let loc = ls.get_var_native("self")?.locations[0];
        mir.add_mir(Mir::Decrement(loc));
        Ok(OutputData::native(mir, None))
    });
    cl.implement_native("Val", "inc", |ls, _cm, _mv| {
        let mut mir = MirCodeBlock::default();
        let loc = ls.get_var_native("self")?.locations[0];
        mir.add_mir(Mir::Increment(loc));
        Ok(OutputData::native(mir, None))
    });
}
