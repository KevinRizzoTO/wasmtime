use std::mem;

use crate::abi::{align_to, ABIArg, ABI, ABIResult};
use crate::codegen::{CodeGen, CodeGenContext, FnCall, FuncEnv};
use crate::frame::{DefinedLocals, Frame};
use crate::isa::x64::masm::MacroAssembler as X64Masm;
use crate::masm::{MacroAssembler, RegImm, OperandSize};
use crate::reg::Reg;
use crate::regalloc::RegAlloc;
use crate::stack::{Stack, Val};
use crate::{
    isa::{Builder, TargetIsa},
    regset::RegSet,
};
use anyhow::{bail, Result};
use cranelift_codegen::{
    isa::x64::settings as x64_settings, settings::Flags, Final, MachBufferFinalized,
};
use target_lexicon::Triple;
use wasmparser::{FuncType, FuncValidator, FunctionBody, ValType, ValidatorResources};

use self::address::Address;
use self::regs::ALL_GPR;

mod abi;
mod address;
mod asm;
mod masm;
// Not all the fpr and gpr constructors are used at the moment;
// in that sense, this directive is a temporary measure to avoid
// dead code warnings.
#[allow(dead_code)]
mod regs;

/// Create an ISA builder.
pub(crate) fn isa_builder(triple: Triple) -> Builder {
    Builder {
        triple,
        settings: x64_settings::builder(),
        constructor: |triple, shared_flags, settings| {
            // TODO: Once enabling/disabling flags is allowed, and once features like SIMD are supported
            // ensure compatibility between shared flags and ISA flags.
            let isa_flags = x64_settings::Flags::new(&shared_flags, settings);
            let isa = X64::new(triple, shared_flags, isa_flags);
            Ok(Box::new(isa))
        },
    }
}

/// x64 ISA.
pub(crate) struct X64 {
    /// The target triple.
    triple: Triple,
    /// ISA specific flags.
    isa_flags: x64_settings::Flags,
    /// Shared flags.
    shared_flags: Flags,
}

impl X64 {
    /// Create a x64 ISA.
    pub fn new(triple: Triple, shared_flags: Flags, isa_flags: x64_settings::Flags) -> Self {
        Self {
            isa_flags,
            shared_flags,
            triple,
        }
    }
}

impl TargetIsa for X64 {
    fn name(&self) -> &'static str {
        "x64"
    }

    fn triple(&self) -> &Triple {
        &self.triple
    }

    fn compile_function(
        &self,
        sig: &FuncType,
        body: &FunctionBody,
        env: &dyn FuncEnv,
        mut validator: FuncValidator<ValidatorResources>,
    ) -> Result<MachBufferFinalized<Final>> {
        let mut body = body.get_binary_reader();
        let mut masm = X64Masm::new(self.shared_flags.clone(), self.isa_flags.clone());
        let stack = Stack::new();
        let abi = abi::X64ABI::default();
        let abi_sig = abi.sig(self.winch_call_conv(), sig);

        let defined_locals = DefinedLocals::new(&mut body, &mut validator)?;
        let frame = Frame::new(&abi_sig, &defined_locals, &abi)?;

        // TODO Add in floating point bitmask
        let regalloc = RegAlloc::new(RegSet::new(ALL_GPR, 0), regs::scratch());
        let codegen_context = CodeGenContext::new(regalloc, stack, &frame);
        let mut codegen = CodeGen::new(&mut masm, &abi, codegen_context, env, abi_sig);

        codegen.emit(&mut body, validator)?;

        Ok(masm.finalize())
    }

    fn compile_trampoline(&self, ty: &FuncType) -> Result<MachBufferFinalized<Final>> {
        let abi = abi::X64ABI::default();
        let mut masm = X64Masm::new(self.shared_flags.clone(), self.isa_flags.clone());
        let stack = Stack::new();
        let regalloc = RegAlloc::new(RegSet::new(ALL_GPR, 0), regs::scratch());

        let trampoline_ty = FuncType::new(
            vec![ValType::I32, ValType::I32, ValType::I32, ValType::I32],
            vec![],
        );
        let trampoline_sig = abi.sig(self.wasmtime_call_conv(), &trampoline_ty);

        let frame = Frame::new(&trampoline_sig, &DefinedLocals::default(), &abi)?;
        let mut codegen_context = CodeGenContext::new(regalloc, stack, &frame);

        let callee_sig = abi.sig(self.winch_call_conv(), ty);

        // This pointer needs to move onto the stack as well prior to assiging arguments to
        // registers
        let val_ptr = if let ABIArg::Reg { reg, ty: _ty } = &trampoline_sig.params[3] {
            Ok(RegImm::reg(*reg))
        } else {
            Err(anyhow::anyhow!(""))
        }
        .unwrap();

        let scratch = regs::scratch();

        // The max size a value can be when reading from the params memory location
        let value_size = mem::size_of::<u128>();

        masm.prologue();

        // Reserve stack space so we can spill the trampoline arguments
        // Do we need to align the stack here? Or can we rely on the FnCall to do that?
        // We also need to combine the fncall total arg stack size, so we don't do multiple
        // instructions
        masm.reserve_stack(trampoline_sig.stack_bytes);

        // Does this really make sense to do here? The value stack gets picked up by the FnCall
        // It assumes that all arguments will be on the value stack, but for a trampoline that
        // isn't true (but should it be?)
        let mut offsets: [u32; 4] = [0; 4];

        trampoline_sig.params.iter().enumerate().for_each(|(i, param)| {
            if let ABIArg::Reg { reg, ty } = param {
                let offset = masm.push(*reg);
                offsets[i] = offset;
            }
        });

        // How big of an operand do we need here? My stub signature has an I32 but is that right?
        masm.mov(val_ptr, RegImm::reg(scratch), crate::masm::OperandSize::S32);

        let call = FnCall::new(&abi, &callee_sig, &mut codegen_context, &mut masm);

        // Keep a second scratch if we need to load a value from the stack
        // It doesn't need to be saved and can be clobbered by the callee
        let argv = regs::argv();

        masm.reserve_stack(call.total_arg_stack_space);

        callee_sig.params.iter().enumerate().for_each(|(i, param)| {
            let value_offset = (i * value_size) as u32;

            match param {
                ABIArg::Reg { reg, ty } => {
                    masm.load(Address::offset(scratch, value_offset), *reg, (*ty).into())
                }
                ABIArg::Stack { offset, ty } => {
                    masm.load(Address::offset(scratch, value_offset), argv, (*ty).into());
                    masm.store(
                        RegImm::reg(argv),
                        masm.address_from_sp(*offset),
                        (*ty).into(),
                    );
                }
            }
        });

        // Move the function pointer from it's stack location into a scratch register
        masm.load(masm.address_from_sp(offsets[2]), scratch, OperandSize::S32);

        // Call the function that was passed into the trampoline
        masm.call(crate::masm::Call::Indirect(scratch));

        // Move the val ptr back into the scratch register so we can load the return values
        masm.load(masm.address_from_sp(offsets[3]), scratch, OperandSize::S32);

        // Move the return values into the value ptr
        // Only doing a single return value for now
        if let ABIResult::Reg { reg, ty } = &callee_sig.result {
            masm.store(RegImm::reg(*reg), Address::offset(scratch, 0), (*ty).unwrap().into());
        }

        // How many locals are actually on the stack at this point?
        // Does the callee clean up it's own stack?
        masm.epilogue(32);

        Ok(masm.finalize())
    }
}
