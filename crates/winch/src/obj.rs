use std::ops::Range;

use cranelift_codegen::TextSectionBuilder;
use object::{
    write::{Object, SectionId, StandardSegment, Symbol, SymbolId, SymbolSection},
    SectionKind, SymbolFlags, SymbolKind, SymbolScope,
};
use wasmtime_environ::FuncIndex;
use winch_codegen::TargetIsa;

use crate::CompiledFunction;

const TEXT_SECTION_NAME: &[u8] = b".text";

pub struct ModuleTextBuilder<'a> {
    /// The target that we're compiling for, used to query target-specific
    /// information as necessary.
    isa: &'a dyn TargetIsa,

    /// The object file that we're generating code into.
    obj: &'a mut Object<'static>,

    /// The WebAssembly module we're generating code for.
    text_section: SectionId,

    /// In-progress text section that we're using cranelift's `MachBuffer` to
    /// build to resolve relocations (calls) between functions.
    text: Box<dyn TextSectionBuilder>,
}

impl<'a> ModuleTextBuilder<'a> {
    /// Creates a new builder for the text section of an executable.
    ///
    /// The `.text` section will be appended to the specified `obj` along with
    /// any unwinding or such information as necessary. The `num_funcs`
    /// parameter indicates the number of times the `append_func` function will
    /// be called. The `finish` function will panic if this contract is not met.
    pub fn new(obj: &'a mut Object<'static>, isa: &'a dyn TargetIsa, num_funcs: usize) -> Self {
        // Entire code (functions and trampolines) will be placed
        // in the ".text" section.
        let text_section = obj.add_section(
            obj.segment_name(StandardSegment::Text).to_vec(),
            TEXT_SECTION_NAME.to_vec(),
            SectionKind::Text,
        );

        Self {
            isa,
            obj,
            text_section,
            text: isa.text_section_builder(num_funcs),
        }
    }

    /// Appends the `func` specified named `name` to this object.
    ///
    /// The `resolve_reloc_target` closure is used to resolve a relocation
    /// target to an adjacent function which has already been added or will be
    /// added to this object. The argument is the relocation target specified
    /// within `CompiledFunction` and the return value must be an index where
    /// the target will be defined by the `n`th call to `append_func`.
    ///
    /// Returns the symbol associated with the function as well as the range
    /// that the function resides within the text section.
    pub fn append_func(
        &mut self,
        name: &str,
        func: &'a CompiledFunction,
        _resolve_reloc_target: impl Fn(FuncIndex) -> usize,
    ) -> (SymbolId, Range<u64>) {
        let body_len = func.body.len() as u64;

        let off = self.text.append(
            true,
            &func.body,
            self.isa.function_alignment(),
        );

        let symbol_id = self.obj.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value: off,
            size: body_len,
            kind: SymbolKind::Text,
            scope: SymbolScope::Compilation,
            weak: false,
            section: SymbolSection::Section(self.text_section),
            flags: SymbolFlags::None,
        });

        // TODO: Add relocation resolution when calls are supported in each Winch ISA.

        (symbol_id, off..off + body_len)
    }

    /// Indicates that the text section has been written completely and this
    /// will finish appending it to the original object.
    ///
    /// Note that this will also write out the unwind information sections if
    /// necessary.
    pub fn finish(mut self) {
        // Finish up the text section now that we're done adding functions.
        let text = self.text.finish();
        self.obj
            .section_mut(self.text_section)
            .set_data(text, self.isa.code_section_alignment());
    }
}
