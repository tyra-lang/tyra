//! Inkwell I6 (slice a): DWARF debug info via `DebugInfoBuilder` (ADR-0014 §4a).
//!
//! Replaces the legacy text-metadata backend (`dwarf.rs`) for the inkwell path.
//! The DIBuilder model is a strict improvement over emitting `!DILocation`
//! strings and string-patching `, !dbg !N` onto the last instruction: setting
//! the builder's current debug location auto-attaches `!dbg` to every
//! instruction it creates, so there is no fragile post-hoc patching.
//!
//! This slice covers the line table — the compile unit, one `DIFile` per source
//! file, a `DISubprogram` per function (attached via `FunctionValue::
//! set_subprogram`), and a per-statement `DILocation`. Local-variable debug info
//! (`llvm.dbg.declare` + `DIType`) lands in a follow-up slice (I6b).
//!
//! `LLVM` requires that, in a function carrying debug info, every inlinable
//! `call` has an in-scope `!dbg`. To satisfy this without annotating each
//! prologue instruction individually, the emitter sets an entry-line location
//! covering the prologue and updates it per source statement; synthetic
//! functions (spawn thunks, eq/hash helpers) carry no subprogram, so their
//! instructions need — and get — no location (the builder location is cleared
//! before they are emitted).

use std::collections::HashMap;

use inkwell::debug_info::{
    AsDIScope, DIFile, DIFlags, DIFlagsConstants, DISubprogram, DIType, DebugInfoBuilder,
    DWARFEmissionKind, DWARFSourceLanguage,
};
use inkwell::module::FlagBehavior;

use tyra_mir::Program;

use crate::inkwell_codegen::CodeGen;

const PRODUCER: &str = "Tyra";

/// Debug-info state for a `--debug` build, owned by `CodeGen.di`.
pub(crate) struct DebugInfo<'ctx> {
    pub(crate) builder: DebugInfoBuilder<'ctx>,
    /// One `DIFile` per `Program::source_files` entry (index = file_id). Always
    /// non-empty (a synthetic `<unknown>` file backs programs with no sources).
    /// Read by I6b (`create_auto_variable` needs the variable's file).
    #[allow(dead_code)]
    pub(crate) files: Vec<DIFile<'ctx>>,
    /// fn name → its `DISubprogram` (also attached to the `FunctionValue`).
    subprograms: HashMap<String, DISubprogram<'ctx>>,
    /// Tyra type (monomorphized name) → `DIType`, for I6b local variables.
    #[allow(dead_code)]
    pub(crate) type_cache: HashMap<String, DIType<'ctx>>,
}

impl<'ctx> CodeGen<'ctx> {
    /// Build the compile unit, per-file `DIFile`s, the shared parameter-less
    /// subroutine type, and a `DISubprogram` per function (attached to its
    /// `FunctionValue`). Called after `declare_functions`, only for `--debug`.
    pub(crate) fn init_debug_info(&mut self, program: &Program) {
        let (pfile, pdir) = program
            .source_files
            .first()
            .map(|p| split_path(p))
            .unwrap_or_else(|| ("<unknown>".into(), String::new()));

        // The compile unit + DIBuilder. `allow_unresolved` lets temporary nodes
        // exist until `finalize`. The trailing "", "" are sysroot/sdk (LLVM 11+).
        let (builder, _cu) = self.module.create_debug_info_builder(
            true,
            DWARFSourceLanguage::C99,
            &pfile,
            &pdir,
            PRODUCER,
            false,
            "",
            0,
            "",
            DWARFEmissionKind::Full,
            0,
            false,
            false,
            "",
            "",
        );

        // inkwell's DIBuilder does not add the module flags itself (clang/LLVM's
        // IRBuilder normally would). Without "Debug Info Version" the verifier
        // silently strips all debug metadata, so add it (and the DWARF version,
        // matching the legacy text backend) explicitly.
        let i32t = self.ctx.i32_type();
        self.module.add_basic_value_flag(
            "Debug Info Version",
            FlagBehavior::Warning,
            i32t.const_int(3, false),
        );
        self.module.add_basic_value_flag(
            "Dwarf Version",
            FlagBehavior::Warning,
            i32t.const_int(4, false),
        );

        let mut files: Vec<DIFile<'ctx>> = program
            .source_files
            .iter()
            .map(|p| {
                let (f, d) = split_path(p);
                builder.create_file(&f, &d)
            })
            .collect();
        if files.is_empty() {
            files.push(builder.create_file("<unknown>", ""));
        }
        let primary = files[0];

        // All Tyra functions share one parameter-less subroutine type (matches
        // the legacy `!DISubroutineType(types: !{})`).
        let subroutine_ty = builder.create_subroutine_type(primary, None, &[], DIFlags::ZERO);

        let mut subprograms: HashMap<String, DISubprogram<'ctx>> = HashMap::new();
        for func in &program.functions {
            let first_loc = func.body.iter().find(|s| !s.loc.is_dummy()).map(|s| s.loc);
            let line = first_loc.map(|l| l.line).unwrap_or(1);
            let file = first_loc
                .and_then(|l| files.get(l.file_id as usize).copied())
                .unwrap_or(primary);
            let display = if func.is_main { "main" } else { func.name.as_str() };
            let sp = builder.create_function(
                file.as_debug_info_scope(),
                display,
                Some(&func.name),
                file,
                line,
                subroutine_ty,
                false, // is_local_to_unit
                true,  // is_definition
                line,  // scope_line
                DIFlags::PROTOTYPED,
                false, // is_optimized
            );
            if let Some(fv) = self.fn_values.get(&func.name) {
                fv.set_subprogram(sp);
            }
            subprograms.insert(func.name.clone(), sp);
        }

        self.di = Some(DebugInfo { builder, files, subprograms, type_cache: HashMap::new() });
    }

    /// The `DISubprogram` for `name`, if debug info is enabled.
    pub(crate) fn di_subprogram(&self, name: &str) -> Option<DISubprogram<'ctx>> {
        self.di.as_ref().and_then(|d| d.subprograms.get(name).copied())
    }

    /// Set the builder's current debug location to `(sp, line)` at column 1
    /// (matching the legacy `column: 1`). Subsequent instructions get this
    /// `!dbg`. No-op without debug info.
    pub(crate) fn set_debug_line(&self, sp: DISubprogram<'ctx>, line: u32) {
        if let Some(d) = &self.di {
            let loc = d.builder.create_debug_location(self.ctx, line, 1, sp.as_debug_info_scope(), None);
            self.builder.set_current_debug_location(loc);
        }
    }

    /// Clear the current debug location before emitting a synthetic function
    /// (spawn thunk / eq-hash helper) that has no subprogram, so its
    /// instructions don't inherit the previous function's scope (which would be
    /// an out-of-scope `!dbg` the verifier rejects). No-op without debug info.
    pub(crate) fn clear_debug_line(&self) {
        if self.di.is_some() {
            self.builder.unset_current_debug_location();
        }
    }

    /// Resolve all temporary debug nodes. Must run once after every body is
    /// emitted (before the module is finalized / verified). No-op without debug.
    pub(crate) fn finalize_debug_info(&self) {
        if let Some(d) = &self.di {
            d.builder.finalize();
        }
    }
}

/// Split a source path into `(filename, directory)` for `DIFile`. An empty or
/// missing directory becomes `"."` (matching the legacy `split_path`).
fn split_path(path: &str) -> (String, String) {
    let p = std::path::Path::new(path);
    let filename = p.file_name().and_then(|n| n.to_str()).unwrap_or(path).to_string();
    let directory = p
        .parent()
        .and_then(|d| d.to_str())
        .map(|d| if d.is_empty() { "." } else { d })
        .unwrap_or(".")
        .to_string();
    (filename, directory)
}
