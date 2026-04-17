pub mod build_context;
pub mod new_pipeline;
pub mod run_context;
pub mod test_context;

/// Stack size for compilation threads (1 GiB)
pub const STACK_SIZE: usize = 1024 * 1024 * 1024;
