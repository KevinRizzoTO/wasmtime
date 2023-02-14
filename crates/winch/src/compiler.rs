use anyhow::{Context, Result};
use cranelift_codegen::{ir, settings, MachStackMap, MachTrap};
use object::write::{Object, SymbolId};
use std::any::Any;
use wasmtime_environ::{
    CompileError, DefinedFuncIndex, FilePos, FlagValue, FuncIndex, FunctionBodyData, FunctionLoc,
    ModuleTranslation, ModuleTypes, PrimaryMap, StackMapInformation, Trap, TrapEncodingBuilder,
    TrapInformation, Tunables, WasmFunctionInfo,
};
use winch_codegen::TargetIsa;

use crate::{obj::ModuleTextBuilder, CompiledFunction};
pub(crate) struct Compiler {
    isa: Box<dyn TargetIsa>,
}

impl Compiler {
    pub fn new(isa: Box<dyn TargetIsa>) -> Self {
        Self { isa }
    }
}

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

            // these should never be emitted by wasmtime-winch
            ir::TrapCode::User(_) => unreachable!(),
        },
    }
}

fn to_flag_value(v: &settings::Value) -> FlagValue {
    match v.kind() {
        settings::SettingKind::Enum => FlagValue::Enum(v.as_enum().unwrap().into()),
        settings::SettingKind::Num => FlagValue::Num(v.as_num().unwrap()),
        settings::SettingKind::Bool => FlagValue::Bool(v.as_bool().unwrap()),
        settings::SettingKind::Preset => unreachable!(),
    }
}

impl wasmtime_environ::Compiler for Compiler {
    fn compile_function(
        &self,
        translation: &ModuleTranslation<'_>,
        index: DefinedFuncIndex,
        data: FunctionBodyData<'_>,
        _tunables: &Tunables,
        _types: &ModuleTypes,
    ) -> Result<(WasmFunctionInfo, Box<dyn Any + Send>), CompileError> {
        let isa = &*self.isa;
        let module = &translation.module;
        let index = module.func_index(index);
        let sig = translation
            .get_types()
            .func_type_at(index.as_u32())
            .context(format!(
                "function type at index {:?} not found",
                index.as_u32()
            ))
            .map_err(|e| CompileError::Codegen(format!("{:?}", e)))?;
        let FunctionBodyData { body, validator } = data;
        // TODO: Need to introduce the concept of a validation context so we can
        // share allocations. Look at the wasmtime_cranelift::Compiler to see
        // how we can re-use existing context objects.
        let validator = validator.into_validator(Default::default());

        let buffer = isa
            .compile_function(&sig, &body, validator)
            .map_err(|e| CompileError::Codegen(format!("{:?}", e)))?;

        let info = WasmFunctionInfo {
            start_srcloc: FilePos::new(body.get_binary_reader().original_position() as u32),
            stack_maps: mach_stack_maps_to_stack_maps(buffer.stack_maps()).into(),
        };

        let traps = buffer.traps().into_iter().map(mach_trap_to_trap).collect();

        Ok((
            info,
            Box::new(CompiledFunction {
                traps,
                body: buffer.data().to_vec(),
            }),
        ))
    }

    fn compile_host_to_wasm_trampoline(
        &self,
        ty: &wasmtime_environ::WasmFuncType,
    ) -> Result<Box<dyn Any + Send>, CompileError> {
        let buffer = self
            .isa
            .compile_trampoline(ty)
            .map_err(|e| CompileError::Codegen(format!("{:?}", e)))?;

        Ok(Box::new(CompiledFunction {
            traps: Vec::new(),
            body: buffer.data().to_vec(),
        }))
    }

    fn append_code(
        &self,
        obj: &mut Object<'static>,
        funcs: &[(String, Box<dyn Any + Send>)],
        _tunables: &Tunables,
        resolve_reloc: &dyn Fn(usize, FuncIndex) -> usize,
    ) -> Result<Vec<(SymbolId, FunctionLoc)>> {
        let mut builder = ModuleTextBuilder::new(obj, &*self.isa, funcs.len());
        let mut traps = TrapEncodingBuilder::default();

        // High level overview:
        // Take the object that is being created. Append all compiled functions
        // in the .text section for executable code and do a final check to make
        // sure all the right data is in the object file. Take the traps within
        // a function and append it to the .wasmtime.traps section.

        let mut ret = Vec::with_capacity(funcs.len());
        for (i, (sym, func)) in funcs.iter().enumerate() {
            let func = func.downcast_ref::<CompiledFunction>().unwrap();

            let (sym, range) = builder.append_func(&sym, func, |idx| resolve_reloc(i, idx));
            traps.push(range.clone(), &func.traps);
            let info = FunctionLoc {
                start: u32::try_from(range.start).unwrap(),
                length: u32::try_from(range.end - range.start).unwrap(),
            };
            ret.push((sym, info));
        }

        builder.finish();

        traps.append_to(obj);

        Ok(ret)
    }

    fn emit_trampoline_obj(
        &self,
        _ty: &wasmtime_environ::WasmFuncType,
        _host_fn: usize,
        _obj: &mut wasmtime_environ::object::write::Object<'static>,
    ) -> Result<(FunctionLoc, FunctionLoc)> {
        // TODO: This is used to create a trampline for host functions.
        // We don't need this for now, but we will need to implement this
        // when we support imports through Winch.
        todo!()
    }

    fn triple(&self) -> &target_lexicon::Triple {
        self.isa.triple()
    }

    fn page_size_align(&self) -> u64 {
        self.isa.code_section_alignment()
    }

    fn flags(&self) -> std::collections::BTreeMap<String, wasmtime_environ::FlagValue> {
        self.isa
            .flags()
            .iter()
            .map(|val| (val.name.to_string(), to_flag_value(&val)))
            .collect()
    }

    fn isa_flags(&self) -> std::collections::BTreeMap<String, wasmtime_environ::FlagValue> {
        self.isa
            .isa_flags()
            .iter()
            .map(|val| (val.name.to_string(), to_flag_value(&val)))
            .collect()
    }

    #[cfg(feature = "component-model")]
    fn component_compiler(&self) -> &dyn wasmtime_environ::component::ComponentCompiler {
        todo!()
    }

    fn append_dwarf(
        &self,
        _obj: &mut Object<'_>,
        _translation: &ModuleTranslation<'_>,
        _funcs: &PrimaryMap<DefinedFuncIndex, (SymbolId, &(dyn Any + Send))>,
    ) -> Result<()> {
        todo!()
    }

    fn is_branch_protection_enabled(&self) -> bool {
        self.isa.is_branch_protection_enabled()
    }
}
