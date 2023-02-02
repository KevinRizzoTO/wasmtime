use crate::compiler::Compiler;
use anyhow::Result;
use cranelift_codegen::settings::{self, Configurable, SetError};
use std::sync::Arc;
use target_lexicon::Triple;
use wasmtime_environ::{CompilerBuilder, Setting, SettingKind};
use winch_codegen::isa;

/// Compiler builder.
struct Builder {
    /// Target triple.
    triple: Triple,
    /// Shared flags builder.
    shared_flags: settings::Builder,
    /// ISA builder.
    isa_builder: isa::Builder,
}

pub fn builder() -> Box<dyn CompilerBuilder> {
    let triple = Triple::host();

    Box::new(Builder {
        triple: triple.clone(),
        shared_flags: settings::builder(),
        // TODO:
        // Either refactor and re-use `cranelift-native::builder()` or come up with a similar
        // mechanism to lookup the host's architecture ISA and infer native flags.
        isa_builder: isa::lookup(triple).expect("host architecture is not supported"),
    })
}

// DOIT: consider if the copy-paste for most of these methods from `cranelift`
// is the right approach
impl CompilerBuilder for Builder {
    fn triple(&self) -> &target_lexicon::Triple {
        // DOIT: see if isa_flags.triple() is the same as self.triple
        &self.triple
    }

    fn target(&mut self, target: target_lexicon::Triple) -> Result<()> {
        // DOIT: consider if we should use `isa::lookup()` here instead.
        self.triple = target;
        Ok(())
    }

    fn set(&mut self, name: &str, value: &str) -> Result<()> {
        if let Err(err) = self.shared_flags.set(name, value) {
            match err {
                SetError::BadName(_) => {
                    // Try the target-specific flags.
                    self.isa_builder.set(name, value)?;
                }
                _ => return Err(err.into()),
            }
        }
        Ok(())
    }

    fn enable(&mut self, name: &str) -> Result<()> {
        if let Err(err) = self.shared_flags.enable(name) {
            match err {
                SetError::BadName(_) => {
                    // Try the target-specific flags.
                    self.isa_builder.enable(name)?;
                }
                _ => return Err(err.into()),
            }
        }
        Ok(())
    }

    fn settings(&self) -> Vec<Setting> {
        // DOIT: consider abstracting this into a function in
        // `cranelift-codegen` or somewhere shared.
        self.isa_builder
            .iter()
            .map(|s| Setting {
                description: s.description,
                name: s.name,
                values: s.values,
                kind: match s.kind {
                    settings::SettingKind::Preset => SettingKind::Preset,
                    settings::SettingKind::Enum => SettingKind::Enum,
                    settings::SettingKind::Num => SettingKind::Num,
                    settings::SettingKind::Bool => SettingKind::Bool,
                },
            })
            .collect()
    }

    fn build(&self) -> Result<Box<dyn wasmtime_environ::Compiler>> {
        let flags = settings::Flags::new(self.shared_flags.clone());
        Ok(Box::new(Compiler::new(
            self.isa_builder.clone().build(flags)?,
        )))
    }

    fn enable_incremental_compilation(
        &mut self,
        _cache_store: Arc<dyn wasmtime_environ::CacheStore>,
    ) {
        // DOIT: we won't support incremental compilation
        // does this need to be told to the user?
        todo!()
    }
}

impl std::fmt::Debug for Builder {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Builder: {{ triple: {:?} }}", self.triple())
    }
}
