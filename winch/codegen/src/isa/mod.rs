use anyhow::{anyhow, Result};
use core::fmt::Formatter;
use cranelift_codegen::{isa::CallConv, settings, Final, MachBufferFinalized, TextSectionBuilder};
use cranelift_wasm::WasmFuncType;
use std::{
    error,
    fmt::{self, Debug, Display},
};
use target_lexicon::{Architecture, Triple};
use wasmparser::{FuncType, FuncValidator, FunctionBody, ValidatorResources};

#[cfg(feature = "x64")]
pub(crate) mod x64;

#[cfg(feature = "arm64")]
pub(crate) mod aarch64;

pub(crate) mod reg;

macro_rules! isa_builder {
    ($name: ident, $cfg_terms: tt, $triple: ident) => {{
        #[cfg $cfg_terms]
        {
            Ok($name::isa_builder($triple))
        }
        #[cfg(not $cfg_terms)]
        {
            Err(anyhow!(LookupError::SupportDisabled))
        }
    }};
}

/// The target ISA builder.
#[derive(Clone)]
pub struct Builder {
    /// The target triple.
    triple: Triple,
    /// The ISA settings builder.
    settings: settings::Builder,
    /// The Target ISA constructor.
    constructor: fn(Triple, settings::Flags, settings::Builder) -> Result<Box<dyn TargetIsa>>,
}

impl Builder {
    /// Create a TargetIsa by combining ISA-specific settings with the provided
    /// shared flags.
    pub fn build(self, shared_flags: settings::Flags) -> Result<Box<dyn TargetIsa>> {
        (self.constructor)(self.triple, shared_flags, self.settings)
    }

    /// Iterates the available settings in the builder.
    pub fn iter(&self) -> impl Iterator<Item = settings::Setting> {
        self.settings.iter()
    }
}

impl settings::Configurable for Builder {
    fn set(&mut self, name: &str, value: &str) -> Result<(), settings::SetError> {
        self.settings.set(name, value)
    }

    fn enable(&mut self, name: &str) -> Result<(), settings::SetError> {
        self.settings.enable(name)
    }
}

/// Look for an ISA builder for the given target triple.
pub fn lookup(triple: Triple) -> Result<Builder> {
    match triple.architecture {
        Architecture::X86_64 => {
            isa_builder!(x64, (feature = "x64"), triple)
        }
        Architecture::Aarch64 { .. } => {
            isa_builder!(aarch64, (feature = "arm64"), triple)
        }

        _ => Err(anyhow!(LookupError::Unsupported)),
    }
}

impl error::Error for LookupError {}
impl Display for LookupError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            LookupError::Unsupported => write!(f, "This target is not supported yet"),
            LookupError::SupportDisabled => write!(f, "Support for this target was disabled"),
        }
    }
}

#[derive(Debug)]
pub(crate) enum LookupError {
    Unsupported,
    // This directive covers the case in which the consumer
    // enables the `all-arch` feature; in such case, this variant
    // will never be used. This is most likely going to change
    // in the future; this is one of the simplest options for now.
    #[allow(dead_code)]
    SupportDisabled,
}

/// A trait representing commonalities between the supported
/// instruction set architectures.
pub trait TargetIsa: Send + Sync {
    /// Get the name of the ISA.
    fn name(&self) -> &'static str;

    /// Get the target triple of the ISA.
    fn triple(&self) -> &Triple;

    /// Get the ISA-independent flags that were used to make this trait object.
    fn flags(&self) -> &settings::Flags;

    /// Get the ISA-dependent flag values that were used to make this trait object.
    fn isa_flags(&self) -> Vec<settings::Value>;

    /// Get a flag indicating whether branch protection is enabled.
    fn is_branch_protection_enabled(&self) -> bool {
        false
    }

    fn compile_function(
        &self,
        sig: &FuncType,
        body: &FunctionBody,
        validator: FuncValidator<ValidatorResources>,
    ) -> Result<MachBufferFinalized<Final>>;

    fn compile_trampoline(&self, ty: &WasmFuncType) -> Result<MachBufferFinalized<Final>>;

    fn text_section_builder(&self, num_labeled_funcs: usize) -> Box<dyn TextSectionBuilder>;

    /// The function alignment required by this ISA.
    fn function_alignment(&self) -> u32;

    /// Get the default calling convention of the underlying target triple.
    fn call_conv(&self) -> CallConv {
        CallConv::triple_default(&self.triple())
    }

    /// Get the endianess of the underlying target triple.
    fn endianness(&self) -> target_lexicon::Endianness {
        self.triple().endianness().unwrap()
    }

    fn code_section_alignment(&self) -> u64 {
        use target_lexicon::*;
        match (self.triple().operating_system, self.triple().architecture) {
            (
                OperatingSystem::MacOSX { .. }
                | OperatingSystem::Darwin
                | OperatingSystem::Ios
                | OperatingSystem::Tvos,
                Architecture::Aarch64(..),
            ) => 0x4000,
            // 64 KB is the maximal page size (i.e. memory translation granule size)
            // supported by the architecture and is used on some platforms.
            (_, Architecture::Aarch64(..)) => 0x10000,
            _ => 0x1000,
        }
    }
}

impl Debug for &dyn TargetIsa {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Target ISA {{ triple: {:?}, calling convention: {:?} }}",
            self.triple(),
            self.call_conv()
        )
    }
}
