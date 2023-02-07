mod builder;
mod compiler;
mod obj;
pub use builder::builder;
use wasmtime_environ::TrapInformation;

#[derive(Default)]
pub struct CompiledFunction {
    /// The machine code for this function.
    body: Vec<u8>,

    /// Metadata about traps in this module, mapping code offsets to the trap
    /// that they may cause.
    traps: Vec<TrapInformation>,
}
