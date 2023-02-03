mod builder;
mod compiler;
mod obj;
pub use builder::builder;
use cranelift_codegen::{binemit, ir};
use wasmtime_environ::{FuncIndex, TrapInformation};

#[derive(Default)]
pub struct CompiledFunction {
    /// The machine code for this function.
    body: Vec<u8>,

    /// Metadata about traps in this module, mapping code offsets to the trap
    /// that they may cause.
    traps: Vec<TrapInformation>,
    // DOIT: In what situations would a function have a different alignment than the ISA?
    relocations: Vec<Relocation>,
}

/// Destination function. Can be either user function or some special one, like `memory.grow`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum RelocationTarget {
    /// The user function index.
    UserFunc(FuncIndex),
    /// A compiler-generated libcall.
    LibCall(ir::LibCall),
}

/// A record of a relocation to perform.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Relocation {
    /// The relocation code.
    reloc: binemit::Reloc,
    /// Relocation target.
    reloc_target: RelocationTarget,
    /// The offset where to apply the relocation.
    offset: binemit::CodeOffset,
    /// The addend to add to the relocation value.
    addend: binemit::Addend,
}
