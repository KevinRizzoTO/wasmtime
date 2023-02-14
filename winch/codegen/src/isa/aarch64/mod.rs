use std::mem;

use self::{
    address::Address,
    regs::{scratch, ALL_GPR},
};
use crate::{
    abi::{ABIArg, ABI},
    codegen::{CodeGen, CodeGenContext},
    frame::Frame,
    isa::{Builder, TargetIsa},
    masm::{MacroAssembler, OperandSize, RegImm},
    reg,
    regalloc::RegAlloc,
    regset::RegSet,
    stack::Stack,
};
use anyhow::Result;
use cranelift_codegen::{
    isa::aarch64::{inst, settings as aarch64_settings},
    settings::Flags,
    Final, MachBufferFinalized, MachTextSectionBuilder,
};
use cranelift_wasm::WasmFuncType;
use masm::MacroAssembler as Aarch64Masm;
use target_lexicon::Triple;
use wasmparser::{FuncType, FuncValidator, FunctionBody, ValidatorResources};

mod abi;
mod address;
mod asm;
mod masm;
mod regs;

/// Create an ISA from the given triple.
pub(crate) fn isa_builder(triple: Triple) -> Builder {
    Builder {
        triple,
        settings: aarch64_settings::builder(),
        constructor: |triple, shared_flags, settings| {
            let isa_flags = aarch64_settings::Flags::new(&shared_flags, settings);
            let isa = Aarch64::new(triple, shared_flags, isa_flags);
            Ok(Box::new(isa))
        },
    }
}

/// Aarch64 ISA.
// Until Aarch64 emission is supported.
#[allow(dead_code)]
pub(crate) struct Aarch64 {
    /// The target triple.
    triple: Triple,
    /// ISA specific flags.
    isa_flags: aarch64_settings::Flags,
    /// Shared flags.
    shared_flags: Flags,
}

impl Aarch64 {
    /// Create an Aarch64 ISA.
    pub fn new(triple: Triple, shared_flags: Flags, isa_flags: aarch64_settings::Flags) -> Self {
        Self {
            isa_flags,
            shared_flags,
            triple,
        }
    }
}

impl TargetIsa for Aarch64 {
    fn name(&self) -> &'static str {
        "aarch64"
    }

    fn triple(&self) -> &Triple {
        &self.triple
    }

    fn compile_function(
        &self,
        sig: &FuncType,
        body: &FunctionBody,
        mut validator: FuncValidator<ValidatorResources>,
    ) -> Result<MachBufferFinalized<Final>> {
        let mut body = body.get_binary_reader();
        let mut masm = Aarch64Masm::new(self.shared_flags.clone());
        let stack = Stack::new();
        let abi = abi::Aarch64ABI::default();
        let abi_sig = abi.sig(sig);
        let frame = Frame::new(&abi_sig, &mut body, &mut validator, &abi)?;
        // TODO: Add floating point bitmask
        let regalloc = RegAlloc::new(RegSet::new(ALL_GPR, 0), scratch());
        let codegen_context = CodeGenContext::new(&mut masm, stack, &frame);
        let mut codegen = CodeGen::new::<abi::Aarch64ABI>(codegen_context, abi_sig, regalloc);

        codegen.emit(&mut body, validator)?;
        Ok(masm.finalize())
    }

    fn compile_trampoline(&self, ty: &WasmFuncType) -> Result<MachBufferFinalized<Final>> {
        // DOIT: Can this whole thing be abstracted into a different struct that ISA's can share?
        let mut masm = Aarch64Masm::new(self.shared_flags.clone());

        let abi = abi::Aarch64ABI::default();
        // DOIT: find a way to convert these types without a clone
        // WIP implementation for From<FuncType> is written but might not be needed
        let abi_sig = abi.sig(&ty.clone().into());

        // DOIT: update trampoline to use n arguments instead of one
        masm.prologue();
        // load up the registers with the arguments
        // x3 contains the values we need to place

        // DOIT: create params based on a fake wasm signature from what we know the trampoline will
        // look like

        // DOIT: Should we use X13 to store arguments temporarily?
        // Also, what operand size should we use in this case?
        masm.mov(RegImm::reg(regs::xreg(3)), RegImm::reg(regs::xreg(13)), OperandSize::S64);

        // The max size a value can be when reading from the params memory location
        let value_size = mem::size_of::<u128>();

        for (i, arg) in abi_sig.params.into_iter().enumerate() {
            match arg {
                ABIArg::Reg { ty, reg } => {
                    // load the value from [x3] into the register

                    // params are separated by the largest size of the params
                    masm.load(Address::offset(regs::xreg(13), (i * value_size) as i64), reg, ty.into());
                }
                ABIArg::Stack { ty, offset } => {
                    todo!()
                }
            }
        }

        masm.call(regs::xreg(2));
        // only doing one return value for now
        // aarch64 calling convention is to return in x0
        // read from w0 (for 32 bit) and x0 (for 64 bit)
        // store in [x3] so it's updated for the caller
        masm.mov(RegImm::reg(regs::xreg(13)), RegImm::reg(regs::xreg(3)), OperandSize::S64);

        masm.store(
            RegImm::reg(regs::xreg(0)),
            Address::offset(regs::xreg(3), 0),
            OperandSize::S64,
        );
        masm.epilogue(0);

        Ok(masm.finalize())
    }

    fn text_section_builder(
        &self,
        num_labeled_funcs: usize,
    ) -> Box<dyn cranelift_codegen::TextSectionBuilder> {
        Box::new(MachTextSectionBuilder::<inst::Inst>::new(num_labeled_funcs))
    }

    fn function_alignment(&self) -> u32 {
        // We use 32-byte alignment for performance reasons, but for correctness we would only need
        // 4-byte alignment.
        32
    }

    fn flags(&self) -> &cranelift_codegen::settings::Flags {
        &self.shared_flags
    }

    fn isa_flags(&self) -> Vec<cranelift_codegen::settings::Value> {
        self.isa_flags.iter().collect()
    }
}
