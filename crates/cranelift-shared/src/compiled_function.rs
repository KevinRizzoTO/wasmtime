use cranelift_codegen::{MachBufferFinalized, Final, MachStackMap, MachTrap, ir, MachReloc};
use wasmtime_environ::{StackMapInformation, TrapInformation, Trap};

fn mach_stack_maps_to_stack_maps(mach_stack_maps: &[MachStackMap]) -> Vec<StackMapInformation> {
    // This is converting from Cranelift's representation of a stack map to
    // Wasmtime's representation. They happen to align today but that may
    // not always be true in the future.
    let mut stack_maps = Vec::new();
    for &MachStackMap {
        offset_end,
        ref stack_map,
        ..
    } in mach_stack_maps
    {
        let stack_map = wasmtime_environ::StackMap::new(
            stack_map.mapped_words(),
            stack_map.as_slice().iter().map(|a| a.0),
        );
        stack_maps.push(StackMapInformation {
            code_offset: offset_end,
            stack_map,
        });
    }
    stack_maps.sort_unstable_by_key(|info| info.code_offset);
    stack_maps
}

const ALWAYS_TRAP_CODE: u16 = 100;

fn mach_trap_to_trap(trap: &MachTrap) -> TrapInformation {
    let &MachTrap { offset, code } = trap;
    TrapInformation {
        code_offset: offset,
        trap_code: match code {
            ir::TrapCode::StackOverflow => Trap::StackOverflow,
            ir::TrapCode::HeapOutOfBounds => Trap::MemoryOutOfBounds,
            ir::TrapCode::HeapMisaligned => Trap::HeapMisaligned,
            ir::TrapCode::TableOutOfBounds => Trap::TableOutOfBounds,
            ir::TrapCode::IndirectCallToNull => Trap::IndirectCallToNull,
            ir::TrapCode::BadSignature => Trap::BadSignature,
            ir::TrapCode::IntegerOverflow => Trap::IntegerOverflow,
            ir::TrapCode::IntegerDivisionByZero => Trap::IntegerDivisionByZero,
            ir::TrapCode::BadConversionToInteger => Trap::BadConversionToInteger,
            ir::TrapCode::UnreachableCodeReached => Trap::UnreachableCodeReached,
            ir::TrapCode::Interrupt => Trap::Interrupt,
            ir::TrapCode::User(ALWAYS_TRAP_CODE) => Trap::AlwaysTrapAdapter,

            // these should never be emitted by wasmtime-cranelift
            ir::TrapCode::User(_) => unreachable!(),
        },
    }
}

fn mach_reloc_to_reloc(func: &Function, reloc: &MachReloc) -> Relocation {
    let &MachReloc {
        offset,
        kind,
        ref name,
        addend,
    } = reloc;
    let reloc_target = if let ExternalName::User(user_func_ref) = *name {
        let UserExternalName { namespace, index } = func.params.user_named_funcs()[user_func_ref];
        debug_assert_eq!(namespace, 0);
        RelocationTarget::UserFunc(FuncIndex::from_u32(index))
    } else if let ExternalName::LibCall(libcall) = *name {
        RelocationTarget::LibCall(libcall)
    } else {
        panic!("unrecognized external name")
    };
    Relocation {
        reloc: kind,
        reloc_target,
        offset,
        addend,
    }
}

#[derive(Default)]
pub struct CompiledFunction {
    /// The machine code for this function.
    body: Vec<u8>,

    /// The unwind information.
    unwind_info: Option<UnwindInfo>,

    /// Information used to translate from binary offsets back to the original
    /// location found in the wasm input.
    address_map: FunctionAddressMap,

    /// Metadata about traps in this module, mapping code offsets to the trap
    /// that they may cause.
    traps: Vec<TrapInformation>,

    relocations: Vec<Relocation>,
    value_labels_ranges: cranelift_codegen::ValueLabelsRanges,
    alignment: u32,
}

impl From<MachBufferFinalized<Final>> for CompiledFunction {
    fn from(value: MachBufferFinalized<Final>) -> Self {
        todo!()
    }
    // add code here
}
