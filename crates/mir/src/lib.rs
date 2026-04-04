mod block;
mod interpreter;
mod mir;
mod no;
mod optimizer;
mod skip_status;
mod state;

pub use block::MirCodeBlock;
pub use interpreter::*;
pub use mir::Mir;
pub use optimizer::block_inliner::*;
pub use state::MirState;

pub fn opt(input: MirCodeBlock) -> MirCodeBlock {
    let reads = crate::no::remove_no_reads(input);
    reads
}
