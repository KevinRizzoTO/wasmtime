use std::mem;

use self::{regs::{scratch, ALL_GPR}, address::Address};
use crate::{
    abi::{ABI, ABIArg},
    codegen::{CodeGen, CodeGenContext},
    frame::Frame,
    isa::{Builder, TargetIsa},
    masm::{MacroAssembler, RegImm, OperandSize},
    regalloc::RegAlloc,
    regset::RegSet,
    stack::Stack,
};
use anyhow::Result;
use cranelift_wasm::WasmFuncType;
use cranelift_codegen::settings::{self, Flags};
use cranelift_codegen::{isa::aarch64::settings as aarch64_settings, Final, MachBufferFinalized};
use cranelift_codegen::{MachTextSectionBuilder, TextSectionBuilder};
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
    Builder::new(
        triple,
        aarch64_settings::builder(),
        |triple, shared_flags, settings| {
            let isa_flags = aarch64_settings::Flags::new(&shared_flags, settings);
            let isa = Aarch64::new(triple, shared_flags, isa_flags);
            Ok(Box::new(isa))
        },
    )
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

    fn flags(&self) -> &settings::Flags {
        &self.shared_flags
    }

    fn isa_flags(&self) -> Vec<settings::Value> {
        self.isa_flags.iter().collect()
    }

    fn is_branch_protection_enabled(&self) -> bool {
        self.isa_flags.use_bti()
    }

    fn compile_function(
        &self,
        sig: &FuncType,
        body: &FunctionBody,
        validator: &mut FuncValidator<ValidatorResources>,
    ) -> Result<MachBufferFinalized<Final>> {
        let mut body = body.get_binary_reader();
        let mut masm = Aarch64Masm::new(self.shared_flags.clone());
        let stack = Stack::new();
        let abi = abi::Aarch64ABI::default();
        let abi_sig = abi.sig(sig);
        let frame = Frame::new(&abi_sig, &mut body, validator, &abi)?;
        // TODO: Add floating point bitmask
        let regalloc = RegAlloc::new(RegSet::new(ALL_GPR, 0), scratch());
        let codegen_context = CodeGenContext::new(regalloc, stack, &frame);
        let mut codegen = CodeGen::new::<abi::Aarch64ABI>(&mut masm, codegen_context, abi_sig);

        codegen.emit(&mut body, validator)?;
        Ok(masm.finalize())
    }

    fn text_section_builder(&self, num_funcs: usize) -> Box<dyn TextSectionBuilder> {
        Box::new(MachTextSectionBuilder::<
            cranelift_codegen::isa::aarch64::inst::Inst,
        >::new(num_funcs))
    }

    fn function_alignment(&self) -> u32 {
        // See `cranelift_codegen`'s value of this for more information
        32
    }

    fn compile_trampoline(&self, ty: &WasmFuncType) -> Result<MachBufferFinalized<Final>> {
        let mut masm = Aarch64Masm::new(self.shared_flags.clone());

        let abi = abi::Aarch64ABI::default();
        // WIP implementation for From<FuncType> is written but might not be needed
        let abi_sig = abi.sig(&ty.clone().into());

        masm.prologue();

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
                    masm.load(Address::offset(regs::xreg(13), (i * value_size) as i64), regs::xreg(19), ty.into());
                    masm.store(RegImm::reg(regs::xreg(19)), Address::from_shadow_sp(offset as i64), ty.into());
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
}
