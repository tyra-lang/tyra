// DWARF debug info context for Tyra's text LLVM IR backend (ADR-0014 §4a).
//
// Metadata node layout (IDs assigned in DwarfCtx::build):
//   !0       = !DICompileUnit
//   !1..k    = !DIFile per source_files entry
//   !k+1     = !{}  (empty tuple, shared)
//   !k+2     = !DISubroutineType(types: !k+1)
//   !k+3..   = !DISubprogram per Function (program order)
//   next+0   = Dwarf Version module flag
//   next+1   = Debug Info Version module flag
//   next+2   = wchar_size module flag
//   next+3   = ident node
//
// Dynamic nodes (created on demand):
//   !DILocation      — get_or_create_loc
//   !DILocalVariable — emit_local_var
//   type nodes       — type_node

use std::collections::HashMap;
use std::fmt::Write as FmtWrite;

use tyra_mir::Program;
use tyra_types::Ty;

pub struct DwarfCtx {
    defs: Vec<(u32, String)>,
    next_id: u32,
    cu_id: u32,
    /// Metadata IDs for each source file (indexed by SourceLoc::file_id).
    pub file_ids: Vec<u32>,
    /// fn_name → !DISubprogram node id.
    subprogram_ids: HashMap<String, u32>,
    /// (subprogram_id, line) → !DILocation node id.
    loc_cache: HashMap<(u32, u32), u32>,
    /// Cached type node IDs (keyed by monomorphized type name).
    type_cache: HashMap<String, u32>,
    /// Module flag and ident node IDs.
    dw_ver_id: u32,
    dbg_info_id: u32,
    wchar_id: u32,
    ident_id: u32,
}

impl DwarfCtx {
    pub fn build(program: &Program) -> Self {
        let mut defs: Vec<(u32, String)> = Vec::new();
        let mut next_id: u32 = 0;

        // !0 = !DICompileUnit (definition deferred until file/empty IDs are known)
        let cu_id = next_id;
        next_id += 1;

        // !1..k = !DIFile per source file
        let mut file_ids: Vec<u32> = Vec::new();
        for path in &program.source_files {
            let id = next_id;
            next_id += 1;
            file_ids.push(id);
            let (fname, dir) = split_path(path);
            defs.push((id, format!("!DIFile(filename: \"{fname}\", directory: \"{dir}\")")));
        }
        if file_ids.is_empty() {
            let id = next_id;
            next_id += 1;
            file_ids.push(id);
            defs.push((id, "!DIFile(filename: \"<unknown>\", directory: \"\")".into()));
        }
        let primary_file_id = file_ids[0];

        // Shared empty metadata tuple
        let empty_id = next_id;
        next_id += 1;
        defs.push((empty_id, "!{}".into()));

        // Shared subroutine type (parameter-less, for all Tyra functions)
        let subrt_id = next_id;
        next_id += 1;
        defs.push((subrt_id, format!("!DISubroutineType(types: !{empty_id})")));

        // !DISubprogram per function
        let mut subprogram_ids: HashMap<String, u32> = HashMap::new();
        for func in &program.functions {
            let sp_id = next_id;
            next_id += 1;
            subprogram_ids.insert(func.name.clone(), sp_id);

            let first_loc = func.body.iter().find(|s| !s.loc.is_dummy()).map(|s| s.loc);
            let first_line = first_loc.map(|l| l.line).unwrap_or(1);
            let file_node = first_loc
                .and_then(|l| file_ids.get(l.file_id as usize).copied())
                .unwrap_or(primary_file_id);

            let display_name = if func.is_main { "main" } else { func.name.as_str() };
            defs.push((sp_id, format!(
                "distinct !DISubprogram(name: \"{display_name}\", linkageName: \"{ln}\", \
                 scope: !{file_node}, file: !{file_node}, line: {first_line}, \
                 type: !{subrt_id}, isLocal: false, isDefinition: true, \
                 scopeLine: {first_line}, flags: DIFlagPrototyped, isOptimized: false, \
                 unit: !{cu_id}, retainedNodes: !{empty_id})",
                ln = func.name,
            )));
        }

        // Module flags
        let dw_ver_id = next_id;
        next_id += 1;
        defs.push((dw_ver_id, "!{i32 7, !\"Dwarf Version\", i32 4}".into()));
        let dbg_info_id = next_id;
        next_id += 1;
        defs.push((dbg_info_id, "!{i32 2, !\"Debug Info Version\", i32 3}".into()));
        let wchar_id = next_id;
        next_id += 1;
        defs.push((wchar_id, "!{i32 1, !\"wchar_size\", i32 4}".into()));

        // Producer ident
        let ident_id = next_id;
        next_id += 1;
        defs.push((ident_id, "!{!\"Tyra v0.6.0\"}".into()));

        // Define !DICompileUnit now that primary_file_id and empty_id are known
        defs.push((cu_id, format!(
            "distinct !DICompileUnit(language: DW_LANG_C99, file: !{primary_file_id}, \
             producer: \"Tyra v0.6.0\", isOptimized: false, runtimeVersion: 0, \
             emissionKind: FullDebug, splitDebugInlining: false)"
        )));

        DwarfCtx {
            defs,
            next_id,
            cu_id,
            file_ids,
            subprogram_ids,
            loc_cache: HashMap::new(),
            type_cache: HashMap::new(),
            dw_ver_id,
            dbg_info_id,
            wchar_id,
            ident_id,
        }
    }

    /// Returns the !DISubprogram metadata node id for a function.
    pub fn subprogram_id(&self, fn_name: &str) -> Option<u32> {
        self.subprogram_ids.get(fn_name).copied()
    }

    /// Get or create a !DILocation node for (subprogram_id, line).
    pub fn get_or_create_loc(&mut self, sp_id: u32, line: u32) -> u32 {
        let key = (sp_id, line);
        if let Some(&id) = self.loc_cache.get(&key) {
            return id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.loc_cache.insert(key, id);
        self.defs
            .push((id, format!("!DILocation(line: {line}, column: 1, scope: !{sp_id})")));
        id
    }

    /// Get or create a DWARF type node for a Tyra type (cached by monomorphized name).
    pub fn type_node(&mut self, ty: &Ty) -> u32 {
        let key = ty.monomorphized_name();
        if let Some(&id) = self.type_cache.get(&key) {
            return id;
        }
        let def = match ty {
            Ty::Int => "!DIBasicType(name: \"Int\", size: 64, encoding: DW_ATE_signed)".into(),
            Ty::Bool => "!DIBasicType(name: \"Bool\", size: 8, encoding: DW_ATE_boolean)".into(),
            Ty::Float => {
                "!DIBasicType(name: \"Float\", size: 64, encoding: DW_ATE_float)".into()
            }
            Ty::String => {
                "!DIDerivedType(tag: DW_TAG_pointer_type, name: \"String\", size: 64)".into()
            }
            Ty::Unit => "!DIBasicType(name: \"Unit\", size: 0)".into(),
            _ => format!(
                "!DIDerivedType(tag: DW_TAG_pointer_type, name: \"{key}\", size: 64)"
            ),
        };
        let id = self.next_id;
        self.next_id += 1;
        self.type_cache.insert(key, id);
        self.defs.push((id, def));
        id
    }

    /// Emit a !DILocalVariable node; returns its id.
    pub fn emit_local_var(
        &mut self,
        name: &str,
        sp_id: u32,
        file_id: u32,
        line: u32,
        type_id: u32,
    ) -> u32 {
        let file_node = self
            .file_ids
            .get(file_id as usize)
            .copied()
            .unwrap_or(self.file_ids[0]);
        let id = self.next_id;
        self.next_id += 1;
        self.defs.push((id, format!(
            "!DILocalVariable(name: \"{name}\", scope: !{sp_id}, \
             file: !{file_node}, line: {line}, type: !{type_id})"
        )));
        id
    }

    /// Serialize the complete metadata section (named + numbered nodes).
    pub fn emit_metadata(&self) -> String {
        let mut out = String::new();
        writeln!(out, "!llvm.dbg.cu = !{{!{}}}", self.cu_id).unwrap();
        writeln!(
            out,
            "!llvm.module.flags = !{{!{}, !{}, !{}}}",
            self.dw_ver_id, self.dbg_info_id, self.wchar_id
        )
        .unwrap();
        writeln!(out, "!llvm.ident = !{{!{}}}", self.ident_id).unwrap();
        writeln!(out).unwrap();

        // Sort by ID for deterministic output
        let mut sorted = self.defs.clone();
        sorted.sort_by_key(|(id, _)| *id);
        for (id, def) in &sorted {
            writeln!(out, "!{id} = {def}").unwrap();
        }
        out
    }
}

/// Split a source file path into (filename, directory) for !DIFile emission.
fn split_path(path: &str) -> (String, String) {
    let p = std::path::Path::new(path);
    let filename = p
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let directory = p
        .parent()
        .and_then(|d| d.to_str())
        .map(|d| if d.is_empty() { "." } else { d })
        .unwrap_or(".")
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    (filename, directory)
}

/// Patch the last emitted LLVM instruction in `out[start..]` to include `, !dbg !N`.
///
/// For multi-instruction MIR lowerings (StructInit, ListInit, …) this annotates
/// the final result instruction, which is the correct sequence-point for the line.
/// Labels, empty lines, and comments are skipped.
pub fn patch_dbg_on_last_instruction(out: &mut String, start: usize, dbg_id: u32) {
    let added = &out[start..];
    let Some(rel_last_nl) = added.rfind('\n') else {
        return;
    };
    let before_last_nl = &added[..rel_last_nl];
    let last_line_start = before_last_nl.rfind('\n').map(|p| p + 1).unwrap_or(0);
    let last_line = &before_last_nl[last_line_start..];
    let trimmed = last_line.trim();
    if trimmed.is_empty()
        || trimmed.ends_with(':')
        || trimmed.starts_with(';')
        || trimmed.starts_with("declare")
        || trimmed.starts_with('@')
    {
        return;
    }
    // Insert ", !dbg !N" immediately before the final '\n'.
    let abs_insert = start + rel_last_nl;
    let tail: String = out[abs_insert..].to_string();
    out.truncate(abs_insert);
    write!(out, ", !dbg !{dbg_id}").unwrap();
    out.push_str(&tail);
}
