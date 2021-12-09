use crate::compiler::class_loader::ClassLoader;

mod array;
mod system;
mod val;

pub fn load_natives(cl: &mut ClassLoader) {
    val::implement(cl);
    system::implement(cl);
    array::implement(cl);
}
