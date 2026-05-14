// Instruction-level LLVM IR emission.
//
// Extracted from codegen.rs. The main `emit_instruction` function is a big
// match on MIR instructions that writes the corresponding LLVM IR. Keeping
// this in its own module keeps codegen.rs focused on orchestration.

use std::fmt::Write;

use tyra_mir::*;

use crate::builtins::emit_builtin_call;
use crate::codegen::EmitCtx;
use crate::helpers::{
    infer_operand_type, is_bool_operand, is_param, llvm_float_literal, llvm_type_str, operand_ref,
    param_llvm_type,
};
use crate::list_codegen::emit_list_instruction;

pub(crate) fn emit_instruction(
    out: &mut String,
    inst: &Instruction,
    func: &Function,
    strings: &[String],
    ctx: &EmitCtx,
) {
    match inst {
        Instruction::Const { dest, value } => match value {
            Constant::Int(n) => {
                writeln!(out, "  %{dest} = add i64 {n}, 0").unwrap();
            }
            Constant::Float(f) => {
                let lit = llvm_float_literal(*f);
                writeln!(out, "  %{dest} = fadd double {lit}, 0.0").unwrap();
            }
            Constant::Bool(b) => {
                let val = if *b { 1 } else { 0 };
                writeln!(out, "  %{dest} = add i1 {val}, 0").unwrap();
            }
            Constant::StringRef(idx) => {
                let len = strings[*idx].len() + 1;
                writeln!(
                    out,
                    "  %{dest} = getelementptr [{len} x i8], ptr @.str.{idx}, i64 0, i64 0"
                )
                .unwrap();
            }
            Constant::Unit => {
                // Unit is represented as i64 0 at runtime so it can be used in
                // Store/Load (e.g., match arm results). Cost-free in practice.
                writeln!(out, "  %{dest} = add i64 0, 0").unwrap();
            }
        },

        Instruction::Call {
            dest,
            func: fname,
            args,
        } => {
            // Try builtin dispatch first; fall through to user-defined call
            if !emit_builtin_call(out, dest, fname, args, func, ctx) {
                // User-defined function call — look up signature for types
                let ret_ty = if let Some(sig) = ctx.fn_sigs.get(fname.as_str()) {
                    llvm_type_str(&sig.return_type, ctx.struct_map)
                } else {
                    "i64".into()
                };
                let user_args = emit_call_args_typed(args, fname, func, ctx);
                if ret_ty == "void" {
                    writeln!(out, "  call void @{fname}({user_args})").unwrap();
                } else if let Some(d) = dest {
                    writeln!(out, "  %{d} = call {ret_ty} @{fname}({user_args})")
                        .unwrap();
                } else {
                    writeln!(out, "  call {ret_ty} @{fname}({user_args})").unwrap();
                }
            }
        }

        Instruction::BinOp { dest, op, lhs, rhs } => {
            let l = operand_ref(lhs, func);
            let r = operand_ref(rhs, func);

            // If either operand is an Option/Result struct, emit a structural
            // comparison (tag + all payload fields).
            //
            // When all fields are scalar (i8/i1/i64/double) we emit a full
            // field-by-field compare AND'd together. AdtInit deterministically
            // zero-fills inactive variant fields, so comparing every field is
            // correct for structural equality of scalar-payload prelude ADTs
            // (Option<Int>, Option<Bool>, Option<Float>, Result<Int,Int>, …).
            //
            // When any field is a ptr or nested struct (Option<String>, data
            // payloads, recursive fields) we fall back to tag-only comparison
            // — identical to the pre-existing behaviour — since deep equality
            // requires string/recursive comparison that is out of scope here.
            if matches!(op, MirBinOp::EqInt | MirBinOp::NeqInt) {
                let lhs_stype = if let Operand::Var(n) = lhs { ctx.struct_temps.get(n.as_str()) } else { None };
                let rhs_stype = if let Operand::Var(n) = rhs { ctx.struct_temps.get(n.as_str()) } else { None };
                if let Some(stype) = lhs_stype.or(rhs_stype) {
                    let info = &ctx.struct_map[stype.as_str()];
                    let llvm_ty = &info.llvm_name;
                    let num_fields = info.field_types.len();
                    let is_eq = matches!(op, MirBinOp::EqInt);

                    // Compute effective LLVM type per field.
                    let field_llvm: Vec<String> = (0..num_fields)
                        .map(|fi| {
                            if fi == 0 {
                                "i8".to_string()
                            } else if info.recursive_fields.get(fi).copied().unwrap_or(false) {
                                "ptr".to_string()
                            } else {
                                let t = llvm_type_str(&info.field_types[fi], ctx.struct_map);
                                if t == "void" { "i64".to_string() } else { t }
                            }
                        })
                        .collect();

                    let all_scalar = field_llvm.iter().all(|t| {
                        matches!(t.as_str(), "i8" | "i1" | "i64" | "double")
                    });

                    if all_scalar {
                        // Extract and compare every field; AND the per-field results.
                        let mut prev_acc: Option<String> = None;
                        for (fi, fty) in field_llvm.iter().enumerate() {
                            writeln!(out, "  %{dest}.l{fi} = extractvalue {llvm_ty} {l}, {fi}").unwrap();
                            writeln!(out, "  %{dest}.r{fi} = extractvalue {llvm_ty} {r}, {fi}").unwrap();
                            if fty == "double" {
                                writeln!(out, "  %{dest}.e{fi} = fcmp oeq double %{dest}.l{fi}, %{dest}.r{fi}").unwrap();
                            } else {
                                writeln!(out, "  %{dest}.e{fi} = icmp eq {fty} %{dest}.l{fi}, %{dest}.r{fi}").unwrap();
                            }
                            prev_acc = Some(match prev_acc {
                                None => format!("%{dest}.e{fi}"),
                                Some(prev) => {
                                    writeln!(out, "  %{dest}.a{fi} = and i1 {prev}, %{dest}.e{fi}").unwrap();
                                    format!("%{dest}.a{fi}")
                                }
                            });
                        }
                        let alleq = prev_acc.unwrap_or_else(|| "true".to_string());
                        if is_eq {
                            writeln!(out, "  %{dest} = or i1 {alleq}, false").unwrap();
                        } else {
                            writeln!(out, "  %{dest} = xor i1 {alleq}, true").unwrap();
                        }
                        return;
                    }

                    // Fallback: tag-only comparison (preserves pre-existing behaviour
                    // for ptr/struct payload ADTs).
                    let cmp = if is_eq { "eq" } else { "ne" };
                    writeln!(out, "  %{dest}.l_tag = extractvalue {llvm_ty} {l}, 0").unwrap();
                    writeln!(out, "  %{dest}.r_tag = extractvalue {llvm_ty} {r}, 0").unwrap();
                    writeln!(out, "  %{dest} = icmp {cmp} i8 %{dest}.l_tag, %{dest}.r_tag").unwrap();
                    return;
                }
            }

            let instr = match op {
                MirBinOp::AddInt => format!("add i64 {l}, {r}"),
                MirBinOp::SubInt => format!("sub i64 {l}, {r}"),
                MirBinOp::MulInt => format!("mul i64 {l}, {r}"),
                MirBinOp::DivInt => format!("sdiv i64 {l}, {r}"),
                MirBinOp::RemInt => format!("srem i64 {l}, {r}"),
                MirBinOp::AddFloat => format!("fadd double {l}, {r}"),
                MirBinOp::SubFloat => format!("fsub double {l}, {r}"),
                MirBinOp::MulFloat => format!("fmul double {l}, {r}"),
                MirBinOp::DivFloat => format!("fdiv double {l}, {r}"),
                MirBinOp::EqInt => {
                    let is_bool = is_bool_operand(lhs, ctx) || is_bool_operand(rhs, ctx);
                    if is_bool {
                        format!("icmp eq i1 {l}, {r}")
                    } else {
                        format!("icmp eq i64 {l}, {r}")
                    }
                }
                MirBinOp::NeqInt => {
                    let is_bool = is_bool_operand(lhs, ctx) || is_bool_operand(rhs, ctx);
                    if is_bool {
                        format!("icmp ne i1 {l}, {r}")
                    } else {
                        format!("icmp ne i64 {l}, {r}")
                    }
                }
                MirBinOp::LtInt => format!("icmp slt i64 {l}, {r}"),
                MirBinOp::LeInt => format!("icmp sle i64 {l}, {r}"),
                MirBinOp::GtInt => format!("icmp sgt i64 {l}, {r}"),
                MirBinOp::GeInt => format!("icmp sge i64 {l}, {r}"),
                MirBinOp::LtFloat => format!("fcmp olt double {l}, {r}"),
                MirBinOp::LeFloat => format!("fcmp ole double {l}, {r}"),
                MirBinOp::GtFloat => format!("fcmp ogt double {l}, {r}"),
                MirBinOp::GeFloat => format!("fcmp oge double {l}, {r}"),
                MirBinOp::EqString | MirBinOp::NeqString => {
                    // String comparison via strcmp: returns 0 if equal
                    let cmp_op = if matches!(op, MirBinOp::EqString) {
                        "eq"
                    } else {
                        "ne"
                    };
                    writeln!(
                        out,
                        "  %{dest}.cmp = call i32 @strcmp(ptr {l}, ptr {r})"
                    )
                    .unwrap();
                    writeln!(
                        out,
                        "  %{dest} = icmp {cmp_op} i32 %{dest}.cmp, 0"
                    )
                    .unwrap();
                    return; // Already wrote %dest, skip the generic writeln below
                }
                MirBinOp::And => format!("and i1 {l}, {r}"),
                MirBinOp::Or => format!("or i1 {l}, {r}"),
            };
            writeln!(out, "  %{dest} = {instr}").unwrap();
        }

        Instruction::Neg { dest, operand } => {
            let v = operand_ref(operand, func);
            let is_float = match operand {
                Operand::Const(Constant::Float(_)) => true,
                Operand::Var(name) => ctx.float_temps.contains(name),
                _ => false,
            };
            if is_float {
                writeln!(out, "  %{dest} = fneg double {v}").unwrap();
            } else {
                writeln!(out, "  %{dest} = sub i64 0, {v}").unwrap();
            }
        }

        Instruction::Not { dest, operand } => {
            let v = operand_ref(operand, func);
            writeln!(out, "  %{dest} = xor i1 {v}, 1").unwrap();
        }

        Instruction::Copy { dest, source } => {
            if is_param(source, func) {
                let lt = param_llvm_type(source, func, ctx.struct_map);
                writeln!(out, "  %{dest} = load {lt}, ptr %{source}.addr").unwrap();
            } else if ctx.struct_temps.contains_key(source.as_str()) {
                // Struct SSA copy: use alloca+store+load to create a new SSA name
                let stype = &ctx.struct_temps[source.as_str()];
                let llvm_ty = &ctx.struct_map[stype.as_str()].llvm_name;
                writeln!(out, "  %{dest}.copy.addr = alloca {llvm_ty}").unwrap();
                writeln!(out, "  store {llvm_ty} %{source}, ptr %{dest}.copy.addr")
                    .unwrap();
                writeln!(out, "  %{dest} = load {llvm_ty}, ptr %{dest}.copy.addr")
                    .unwrap();
            } else if ctx.string_temps.contains(source.as_str()) {
                // String (ptr) SSA copy via inttoptr/ptrtoint round-trip
                writeln!(out, "  %{dest}.ptr.int = ptrtoint ptr %{source} to i64")
                    .unwrap();
                writeln!(out, "  %{dest} = inttoptr i64 %{dest}.ptr.int to ptr")
                    .unwrap();
            } else if ctx.float_temps.contains(source.as_str()) {
                writeln!(out, "  %{dest} = fadd double %{source}, 0.0").unwrap();
            } else if ctx.bool_temps.contains(source.as_str()) {
                writeln!(out, "  %{dest} = xor i1 %{source}, 0").unwrap();
            } else {
                // SSA alias: create a new SSA value identical to the source
                writeln!(out, "  %{dest} = add i64 %{source}, 0").unwrap();
            }
        }

        Instruction::Return { value } => {
            if func.is_main {
                writeln!(out, "  ret i32 0").unwrap();
            } else {
                match value {
                    Some(v) => {
                        let ret_ty = llvm_type_str(&func.return_type, ctx.struct_map);
                        if ret_ty == "void" {
                            // Unit return: value is a dummy placeholder, ignore it
                            writeln!(out, "  ret void").unwrap();
                        } else {
                            let val = operand_ref(v, func);
                            writeln!(out, "  ret {ret_ty} {val}").unwrap();
                        }
                    }
                    None => {
                        writeln!(out, "  ret void").unwrap();
                    }
                }
            }
        }

        Instruction::Label(name) => {
            writeln!(out, "{name}:").unwrap();
        }

        Instruction::BranchIf {
            cond,
            true_label,
            false_label,
        } => {
            let c = operand_ref(cond, func);
            writeln!(
                out,
                "  br i1 {c}, label %{true_label}, label %{false_label}"
            )
            .unwrap();
        }

        Instruction::Jump { label } => {
            writeln!(out, "  br label %{label}").unwrap();
        }

        Instruction::Phi { dest, branches } => {
            let phi_ty = if let Some((first_val, _)) = branches.first() {
                infer_operand_type(first_val, ctx)
            } else {
                "i64".into()
            };
            let entries: Vec<String> = branches
                .iter()
                .map(|(val, label)| format!("[{}, %{label}]", operand_ref(val, func)))
                .collect();
            writeln!(out, "  %{dest} = phi {phi_ty} {}", entries.join(", ")).unwrap();
        }

        // Alloca slots are hoisted to the entry block in emit_function so they
        // are allocated once instead of per loop iteration (099 diagnosis).
        Instruction::Alloca { .. } => {}

        Instruction::Store { dest, value } => {
            let val = operand_ref(value, func);
            // Use the VALUE's LLVM type for the store annotation, not the
            // destination alloca's declared type.  They differ on dead code
            // paths (e.g. the Ok-path of `Err("msg")?` extracts i64/Unit but
            // the match result alloca is ptr/String from another arm).  LLVM
            // requires the store annotation to match the value's type; it does
            // not require it to match the alloca's declared type.
            let llvm_ty = infer_operand_type(value, ctx);
            writeln!(out, "  store {llvm_ty} {val}, ptr %{dest}").unwrap();
        }

        Instruction::Load { dest, source } => {
            let llvm_ty = ctx
                .alloca_llvm_types
                .get(source.as_str())
                .map(String::as_str)
                .unwrap_or("i64");
            writeln!(out, "  %{dest} = load {llvm_ty}, ptr %{source}").unwrap();
        }

        Instruction::StructInit {
            dest,
            type_name,
            fields,
        } => {
            let info = &ctx.struct_map[type_name.as_str()];
            let llvm_ty = &info.llvm_name;
            if info.is_data {
                // Data type: heap-allocate via GC_malloc, then GEP+store each field (§8.6).
                // Follows the same GC_malloc+null-check pattern as StringFormat.
                // Note: Boehm GC_malloc never returns NULL (calls GC_oom_func on OOM,
                // default aborts). The null check is defensive/documentary only.
                writeln!(out, "  %{dest}.size.gep = getelementptr {llvm_ty}, ptr null, i32 1").unwrap();
                writeln!(out, "  %{dest}.size = ptrtoint ptr %{dest}.size.gep to i64").unwrap();
                writeln!(out, "  %{dest} = call ptr @GC_malloc(i64 %{dest}.size)").unwrap();
                // Abort on OOM (consistent with StringFormat)
                writeln!(out, "  %{dest}.null = icmp eq ptr %{dest}, null").unwrap();
                writeln!(out, "  br i1 %{dest}.null, label %{dest}.oom, label %{dest}.ok").unwrap();
                writeln!(out, "{dest}.oom:").unwrap();
                writeln!(out, "  call void @abort()").unwrap();
                writeln!(out, "  unreachable").unwrap();
                writeln!(out, "{dest}.ok:").unwrap();
                for (i, field_op) in fields.iter().enumerate() {
                    let val = operand_ref(field_op, func);
                    let field_ty = llvm_type_str(&info.field_types[i], ctx.struct_map);
                    writeln!(out, "  %{dest}.f{i}.gep = getelementptr {llvm_ty}, ptr %{dest}, i32 0, i32 {i}").unwrap();
                    writeln!(out, "  store {field_ty} {val}, ptr %{dest}.f{i}.gep").unwrap();
                }
            } else if fields.is_empty() {
                // Zero-field struct: just produce undef
                writeln!(out, "  ; %{dest} = {llvm_ty} undef (zero-field struct)")
                    .unwrap();
            } else {
                // Value type: build struct value via insertvalue chain starting from undef
                let mut current = "undef".to_string();
                for (i, field_op) in fields.iter().enumerate() {
                    let val = operand_ref(field_op, func);
                    let field_ty = llvm_type_str(&info.field_types[i], ctx.struct_map);
                    let step_dest = if i + 1 == fields.len() {
                        dest.clone()
                    } else {
                        format!("{dest}.s{i}")
                    };
                    writeln!(
                        out,
                        "  %{step_dest} = insertvalue {llvm_ty} {current}, {field_ty} {val}, {i}"
                    )
                    .unwrap();
                    current = format!("%{step_dest}");
                }
            }
        }

        Instruction::FieldGet {
            dest,
            obj,
            type_name,
            field_index,
        } => {
            let info = &ctx.struct_map[type_name.as_str()];
            let val = operand_ref(obj, func);
            if info.is_data {
                // Data type: GEP + load the field from the heap struct
                let llvm_ty = &info.llvm_name;
                let field_llvm_ty = llvm_type_str(&info.field_types[*field_index as usize], ctx.struct_map);
                writeln!(out, "  %{dest}.gep = getelementptr {llvm_ty}, ptr {val}, i32 0, i32 {field_index}").unwrap();
                writeln!(out, "  %{dest} = load {field_llvm_ty}, ptr %{dest}.gep").unwrap();
            } else {
                // Value type: extractvalue from struct
                let llvm_ty = &info.llvm_name;
                writeln!(
                    out,
                    "  %{dest} = extractvalue {llvm_ty} {val}, {field_index}"
                )
                .unwrap();
            }
        }

        Instruction::FieldSet {
            obj,
            type_name,
            field_index,
            value,
        } => {
            let info = &ctx.struct_map[type_name.as_str()];
            let llvm_ty = &info.llvm_name;
            let field_llvm_ty = llvm_type_str(&info.field_types[*field_index as usize], ctx.struct_map);
            let ptr_val = operand_ref(obj, func);
            let store_val = operand_ref(value, func);
            let obj_name = match obj {
                Operand::Var(n) => n.as_str(),
                _ => "fset",
            };
            writeln!(out, "  %{obj_name}.f{field_index}.gep = getelementptr {llvm_ty}, ptr {ptr_val}, i32 0, i32 {field_index}").unwrap();
            writeln!(out, "  store {field_llvm_ty} {store_val}, ptr %{obj_name}.f{field_index}.gep").unwrap();
        }

        Instruction::StringFormat {
            dest,
            format_ref,
            args,
        } => {
            // Heap-allocate a 1024-byte buffer for the formatted string via GC.
            // Strings longer than 1024 bytes are truncated by snprintf.
            // TODO(M8+): use GC_malloc_atomic once atomic/non-atomic classification
            // is implemented — string buffers contain no pointers.
            writeln!(
                out,
                "  %{dest} = call ptr @GC_malloc(i64 1024)"
            )
            .unwrap();
            // Abort if malloc returns null (out of memory)
            writeln!(
                out,
                "  %{dest}.null = icmp eq ptr %{dest}, null"
            )
            .unwrap();
            writeln!(
                out,
                "  br i1 %{dest}.null, label %{dest}.oom, label %{dest}.ok"
            )
            .unwrap();
            writeln!(out, "{dest}.oom:").unwrap();
            writeln!(out, "  call void @abort()").unwrap();
            writeln!(out, "  unreachable").unwrap();
            writeln!(out, "{dest}.ok:").unwrap();

            // Build format string reference
            let fmt_len = strings[*format_ref].len() + 1;
            let fmt_ref = format!(
                "getelementptr ([{fmt_len} x i8], ptr @.str.{format_ref}, i64 0, i64 0)"
            );

            // Build snprintf argument list
            let mut snprintf_args = vec![
                format!("ptr %{dest}"),
                "i64 1024".to_string(),
                format!("ptr {fmt_ref}"),
            ];

            for (j, arg) in args.iter().enumerate() {
                let val = operand_ref(arg, func);
                let ty = infer_operand_type(arg, ctx);
                if ty == "i1" {
                    // Bool needs widening for printf varargs
                    writeln!(out, "  %{dest}.zext.{j} = zext i1 {val} to i64").unwrap();
                    snprintf_args.push(format!("i64 %{dest}.zext.{j}"));
                } else {
                    snprintf_args.push(format!("{ty} {val}"));
                }
            }

            writeln!(
                out,
                "  %{dest}.len = call i32 (ptr, i64, ptr, ...) @snprintf({})",
                snprintf_args.join(", ")
            )
            .unwrap();
        }

        Instruction::AdtInit {
            dest,
            type_name,
            tag,
            fields,
        } => {
            let info = &ctx.struct_map[type_name.as_str()];
            let llvm_ty = &info.llvm_name;
            let num_fields = info.field_types.len();

            // Field 0: tag (i8)
            let mut current = if num_fields <= 1 && fields.is_empty() {
                // Tag-only struct (unit variant with no payload fields in struct)
                format!("%{dest}")
            } else {
                format!("%{dest}.s0")
            };
            writeln!(
                out,
                "  {current} = insertvalue {llvm_ty} undef, i8 {tag}, 0"
            )
            .unwrap();

            // Fill remaining fields from the fields vector, zero-filling extras
            for fi in 1..num_fields {
                let raw_ty = llvm_type_str(&info.field_types[fi], ctx.struct_map);
                let is_recursive = info
                    .recursive_fields
                    .get(fi)
                    .copied()
                    .unwrap_or(false);
                // Unit (void) is stored as i64 in struct fields.
                // Recursive self-reference is boxed as GC-heap ptr.
                let field_ty_str = if is_recursive {
                    "ptr".to_string()
                } else if raw_ty == "void" {
                    "i64".into()
                } else {
                    raw_ty
                };
                let is_last = fi + 1 == num_fields;
                let step_dest = if is_last {
                    format!("%{dest}")
                } else {
                    format!("%{dest}.s{fi}")
                };

                let field_idx = fi - 1; // fields[0] → struct field 1, etc.
                if let Some(field_op) = fields.get(field_idx) {
                    if is_recursive {
                        // Box the incoming self-typed value onto the GC
                        // heap and insert the resulting `ptr`. The
                        // recursive field's MIR operand is a fully-formed
                        // ADT struct SSA value; we allocate sizeof(struct)
                        // bytes and store it, then pass the ptr to
                        // insertvalue. Using a zero-literal (placeholder
                        // for an absent field in a different variant)
                        // yields a null ptr instead.
                        if matches!(field_op, Operand::Const(Constant::Int(0))) {
                            writeln!(
                                out,
                                "  {step_dest} = insertvalue {llvm_ty} {current}, ptr null, {fi}"
                            )
                            .unwrap();
                        } else {
                            let v = operand_ref(field_op, func);
                            let size_gep = format!("%{dest}.r{fi}.sg");
                            let size_int = format!("%{dest}.r{fi}.sz");
                            let box_ptr = format!("%{dest}.r{fi}.box");
                            writeln!(
                                out,
                                "  {size_gep} = getelementptr {llvm_ty}, ptr null, i32 1"
                            )
                            .unwrap();
                            writeln!(
                                out,
                                "  {size_int} = ptrtoint ptr {size_gep} to i64"
                            )
                            .unwrap();
                            writeln!(
                                out,
                                "  {box_ptr} = call ptr @GC_malloc(i64 {size_int})"
                            )
                            .unwrap();
                            writeln!(
                                out,
                                "  store {llvm_ty} {v}, ptr {box_ptr}"
                            )
                            .unwrap();
                            writeln!(
                                out,
                                "  {step_dest} = insertvalue {llvm_ty} {current}, ptr {box_ptr}, {fi}"
                            )
                            .unwrap();
                        }
                        current = step_dest;
                        continue;
                    }
                    // Check for zero placeholder on non-integer fields
                    let val = match field_op {
                        Operand::Const(Constant::Int(0)) if field_ty_str == "ptr" => {
                            "null".to_string()
                        }
                        Operand::Const(Constant::Int(0)) if field_ty_str == "double" => {
                            "0.0".to_string()
                        }
                        Operand::Const(Constant::Int(0))
                            if field_ty_str.starts_with("%struct.") =>
                        {
                            "zeroinitializer".to_string()
                        }
                        _ => operand_ref(field_op, func),
                    };
                    writeln!(
                        out,
                        "  {step_dest} = insertvalue {llvm_ty} {current}, {field_ty_str} {val}, {fi}"
                    )
                    .unwrap();
                } else {
                    // No field provided: insert zero
                    let zero = if field_ty_str == "ptr" {
                        "null"
                    } else if field_ty_str == "double" {
                        "0.0"
                    } else if field_ty_str.starts_with("%struct.") {
                        "zeroinitializer"
                    } else {
                        "0"
                    };
                    writeln!(
                        out,
                        "  {step_dest} = insertvalue {llvm_ty} {current}, {field_ty_str} {zero}, {fi}"
                    )
                    .unwrap();
                }
                current = step_dest;
            }
        }

        Instruction::AdtTag {
            dest,
            obj,
            type_name,
        } => {
            let info = ctx
                .struct_map
                .get(type_name.as_str())
                .unwrap_or_else(|| panic!("AdtTag: unknown struct {type_name}"));
            let llvm_ty = &info.llvm_name;
            let val = operand_ref(obj, func);
            // Extract tag (field 0, i8) and extend to i64 for comparison
            writeln!(
                out,
                "  %{dest}.i8 = extractvalue {llvm_ty} {val}, 0"
            )
            .unwrap();
            writeln!(
                out,
                "  %{dest} = zext i8 %{dest}.i8 to i64"
            )
            .unwrap();
        }

        Instruction::AdtPayload {
            dest,
            obj,
            type_name,
            field_index,
        } => {
            let info = &ctx.struct_map[type_name.as_str()];
            let llvm_ty = &info.llvm_name;
            let val = operand_ref(obj, func);
            let idx = *field_index as usize;
            let is_recursive = info
                .recursive_fields
                .get(idx)
                .copied()
                .unwrap_or(false);
            if is_recursive {
                // Extract the boxed `ptr` and load the referenced ADT
                // struct back into an SSA value the rest of codegen treats
                // uniformly with inline variants.
                writeln!(
                    out,
                    "  %{dest}.box = extractvalue {llvm_ty} {val}, {field_index}"
                )
                .unwrap();
                writeln!(
                    out,
                    "  %{dest} = load {llvm_ty}, ptr %{dest}.box"
                )
                .unwrap();
            } else {
                writeln!(
                    out,
                    "  %{dest} = extractvalue {llvm_ty} {val}, {field_index}"
                )
                .unwrap();
            }
        }

        // List instructions are handled by list_codegen delegation above
        Instruction::ListInit { .. }
        | Instruction::ListLen { .. }
        | Instruction::ListGet { .. }
        | Instruction::ListGetSafe { .. }
        | Instruction::ListPush { .. } => {
            emit_list_instruction(out, inst, func, ctx);
        }

        Instruction::MapGetOption { dest, handle, key } => {
            let h = operand_ref(handle, func);
            let k = operand_ref(key, func);
            let opt_ty = "Option__Int";
            let opt_llvm = if let Some(info) = ctx.struct_map.get(opt_ty) {
                info.llvm_name.clone()
            } else {
                "%struct.Option__Int".into()
            };
            // raw = call __map_get_string_int(handle, key)
            writeln!(
                out,
                "  %{dest}.raw = call i64 @tyra_map_get_string_int(ptr {h}, ptr {k})"
            )
            .unwrap();
            // present = call __map_get_present()  (i32 → i1)
            writeln!(
                out,
                "  %{dest}.present.i32 = call i32 @tyra_map_get_present()"
            )
            .unwrap();
            writeln!(
                out,
                "  %{dest}.present = icmp ne i32 %{dest}.present.i32, 0"
            )
            .unwrap();
            // tag = present ? 0 (Some) : 1 (None) — Option layout is {i8, i64}.
            writeln!(
                out,
                "  %{dest}.tag = select i1 %{dest}.present, i8 0, i8 1"
            )
            .unwrap();
            // value = present ? raw : 0  (zero out the unused payload for None
            // so Hash/Eq derivations behave deterministically)
            writeln!(
                out,
                "  %{dest}.val = select i1 %{dest}.present, i64 %{dest}.raw, i64 0"
            )
            .unwrap();
            // Build Option<Int>.
            writeln!(
                out,
                "  %{dest}.s0 = insertvalue {opt_llvm} undef, i8 %{dest}.tag, 0"
            )
            .unwrap();
            writeln!(
                out,
                "  %{dest} = insertvalue {opt_llvm} %{dest}.s0, i64 %{dest}.val, 1"
            )
            .unwrap();
        }

        Instruction::Spawn {
            dest,
            func: target_fn,
            args,
            arg_types,
            result_type,
        } => {
            emit_spawn(out, dest, target_fn, args, arg_types, result_type, func, ctx);
        }

        Instruction::Await {
            dest,
            task,
            result_type,
        } => {
            emit_await(out, dest, task, result_type, func, ctx);
        }

        Instruction::JoinAll {
            dest,
            list,
            elem_type,
        } => {
            emit_join_all(out, dest, list, elem_type, func, ctx);
        }

        Instruction::Select {
            dest,
            list,
            // elem_type is captured on the MIR instruction for symmetry
            // with JoinAll and to keep lowering/codegen round-trip faithful.
            // Codegen does not need it: the runtime returns a new Task
            // handle and the downstream `.await` consults `task_result_types`
            // (populated in call.rs) to drive the unbox. If a future
            // codegen change demands verification of T against the MIR
            // record, plumb it through here.
            elem_type: _,
        } => {
            emit_select(out, dest, list, func, ctx);
        }
    }
}

/// Emit the runtime call `tyra_task_select(handles, n)` → new Task handle.
/// The incoming list carries task handles as i64; we hand its raw `ptr`
/// straight to the runtime (the layout `{ ptr data, i64 len }` matches
/// `*const *const Task + i64` on LP64).
fn emit_select(
    out: &mut String,
    dest: &str,
    list: &Operand,
    func: &Function,
    ctx: &EmitCtx,
) {
    let list_ref = operand_ref(list, func);
    let in_list_ty = if let Operand::Var(name) = list {
        ctx.struct_temps
            .get(name)
            .map(|s| format!("%struct.{s}"))
            .unwrap_or_else(|| "%struct.List__Int".to_string())
    } else {
        "%struct.List__Int".to_string()
    };
    writeln!(
        out,
        "  %{dest}.in_data = extractvalue {in_list_ty} {list_ref}, 0"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest}.n = extractvalue {in_list_ty} {list_ref}, 1"
    )
    .unwrap();
    // tyra_task_select returns *const Task; cast to i64 so it flows as a
    // plain task handle through the MIR (same convention as Spawn).
    writeln!(
        out,
        "  %{dest}.tptr = call ptr @tyra_task_select(ptr %{dest}.in_data, i64 %{dest}.n)"
    )
    .unwrap();
    writeln!(out, "  %{dest} = ptrtoint ptr %{dest}.tptr to i64").unwrap();
}

/// Emit inline loop: await every i64 task handle in `list`, load the unboxed
/// T from each result box, and build a fresh `%struct.List__T` with the
/// unboxed values. `list` operand type: `{ ptr data, i64 len }` of i64
/// handles (task handles travel as i64 through the MIR).
fn emit_join_all(
    out: &mut String,
    dest: &str,
    list: &Operand,
    elem_type: &tyra_types::Ty,
    func: &Function,
    ctx: &EmitCtx,
) {
    let list_ref = operand_ref(list, func);
    let elem_llvm = llvm_type_str(elem_type, ctx.struct_map);
    // Look up the incoming (handle) list's monomorphized struct name from
    // the type scan. Falls back to List__Int for bare spawn temps which the
    // scan does not tag (handles travel as i64). All Tyra lists share the
    // `{ ptr, i64 }` shape regardless, so the handle-list may legitimately
    // use a different monomorphization than the result list_ty below.
    let in_list_ty = if let Operand::Var(name) = list {
        ctx.struct_temps
            .get(name)
            .map(|s| format!("%struct.{s}"))
            .unwrap_or_else(|| "%struct.List__Int".to_string())
    } else {
        "%struct.List__Int".to_string()
    };
    writeln!(
        out,
        "  %{dest}.in_data = extractvalue {in_list_ty} {list_ref}, 0"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest}.n = extractvalue {in_list_ty} {list_ref}, 1"
    )
    .unwrap();
    let list_ty = format!("%struct.List__{}", elem_type.monomorphized_name());

    // Allocate result array.
    writeln!(
        out,
        "  %{dest}.esz_ptr = getelementptr {elem_llvm}, ptr null, i32 1"
    )
    .unwrap();
    writeln!(out, "  %{dest}.esz = ptrtoint ptr %{dest}.esz_ptr to i64").unwrap();
    writeln!(
        out,
        "  %{dest}.tsz = mul i64 %{dest}.n, %{dest}.esz"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest}.out_data = call ptr @GC_malloc(i64 %{dest}.tsz)"
    )
    .unwrap();

    // Loop counter.
    writeln!(out, "  %{dest}.ctr = alloca i64").unwrap();
    writeln!(out, "  store i64 0, ptr %{dest}.ctr").unwrap();
    writeln!(out, "  br label %{dest}.loop").unwrap();
    writeln!(out, "{dest}.loop:").unwrap();
    writeln!(out, "  %{dest}.i = load i64, ptr %{dest}.ctr").unwrap();
    writeln!(
        out,
        "  %{dest}.done = icmp sge i64 %{dest}.i, %{dest}.n"
    )
    .unwrap();
    writeln!(
        out,
        "  br i1 %{dest}.done, label %{dest}.end, label %{dest}.body"
    )
    .unwrap();
    writeln!(out, "{dest}.body:").unwrap();
    writeln!(
        out,
        "  %{dest}.hgep = getelementptr i64, ptr %{dest}.in_data, i64 %{dest}.i"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest}.handle = load i64, ptr %{dest}.hgep"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest}.tptr = inttoptr i64 %{dest}.handle to ptr"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest}.box = call ptr @tyra_task_await(ptr %{dest}.tptr)"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest}.val = load {elem_llvm}, ptr %{dest}.box"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest}.ogep = getelementptr {elem_llvm}, ptr %{dest}.out_data, i64 %{dest}.i"
    )
    .unwrap();
    writeln!(
        out,
        "  store {elem_llvm} %{dest}.val, ptr %{dest}.ogep"
    )
    .unwrap();
    writeln!(out, "  %{dest}.next = add i64 %{dest}.i, 1").unwrap();
    writeln!(out, "  store i64 %{dest}.next, ptr %{dest}.ctr").unwrap();
    writeln!(out, "  br label %{dest}.loop").unwrap();
    writeln!(out, "{dest}.end:").unwrap();

    // Build the output list struct.
    writeln!(
        out,
        "  %{dest}.s0 = insertvalue {list_ty} undef, ptr %{dest}.out_data, 0"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest} = insertvalue {list_ty} %{dest}.s0, i64 %{dest}.n, 1"
    )
    .unwrap();
}

/// Emit `spawn func(args)` — box args into a GC-managed struct, register
/// a per-site thunk, call `tyra_task_spawn`, and bind the handle to `dest`.
fn emit_spawn(
    out: &mut String,
    dest: &str,
    target_fn: &str,
    args: &[Operand],
    arg_types: &[tyra_types::Ty],
    result_type: &tyra_types::Ty,
    func: &Function,
    ctx: &EmitCtx,
) {
    // Reserve a thunk id and register metadata. The thunk body is emitted
    // after all user functions by emit_spawn_thunk.
    let id = {
        let mut thunks = ctx.spawn_thunks.borrow_mut();
        let id = thunks.len();
        thunks.push(crate::codegen::SpawnThunk {
            id,
            func: target_fn.to_string(),
            arg_types: arg_types.to_vec(),
            result_type: result_type.clone(),
        });
        id
    };

    // Pack args into a GC_malloc'd struct if there are any; otherwise pass null.
    let args_box = if args.is_empty() {
        "null".into()
    } else {
        let args_ty = format!("%struct.__tyra_spawn_args_{id}");
        // sizeof via GEP(null, 1) trick.
        writeln!(
            out,
            "  %{dest}.asz_ptr = getelementptr {args_ty}, ptr null, i32 1"
        )
        .unwrap();
        writeln!(
            out,
            "  %{dest}.asz = ptrtoint ptr %{dest}.asz_ptr to i64"
        )
        .unwrap();
        writeln!(
            out,
            "  %{dest}.args = call ptr @GC_malloc(i64 %{dest}.asz)"
        )
        .unwrap();

        // Store each arg into its slot via GEP.
        for (i, (arg, ty)) in args.iter().zip(arg_types.iter()).enumerate() {
            let val = operand_ref(arg, func);
            let llvm_ty = llvm_type_str(ty, ctx.struct_map);
            writeln!(
                out,
                "  %{dest}.ap{i} = getelementptr {args_ty}, ptr %{dest}.args, i32 0, i32 {i}"
            )
            .unwrap();
            writeln!(out, "  store {llvm_ty} {val}, ptr %{dest}.ap{i}").unwrap();
        }
        format!("%{dest}.args")
    };

    // Task handle is carried as i64 through the MIR so it can flow through
    // lists and mixed-type expressions. Codegen round-trips through ptr at
    // Spawn (out) and Await (in).
    // SAFETY: ptrtoint/inttoptr assumes 64-bit flat pointers. v0.1 targets
    // macOS/Linux x86_64 and aarch64; CHERI and 32-bit platforms are out of
    // scope and will need explicit ptr-valued handles instead of i64.
    writeln!(
        out,
        "  %{dest}.h = call ptr @tyra_task_spawn(ptr @__tyra_spawn_thunk_{id}, ptr {args_box})"
    )
    .unwrap();
    writeln!(out, "  %{dest} = ptrtoint ptr %{dest}.h to i64").unwrap();
}

/// Emit `dest = task.await` — call runtime, load boxed result.
fn emit_await(
    out: &mut String,
    dest: &str,
    task: &Operand,
    result_type: &tyra_types::Ty,
    func: &Function,
    ctx: &EmitCtx,
) {
    let task_ref = operand_ref(task, func);
    // Task handle travels as i64 through the MIR; convert back to ptr.
    writeln!(
        out,
        "  %{dest}.tptr = inttoptr i64 {task_ref} to ptr"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest}.box = call ptr @tyra_task_await(ptr %{dest}.tptr)"
    )
    .unwrap();

    if matches!(result_type, tyra_types::Ty::Unit) {
        // Unit has no runtime value, but SSA requires `dest` be assigned
        // somewhere so downstream references type-check. `add i64 0, 0` is
        // a self-documenting no-op LLVM folds to a constant.
        writeln!(out, "  %{dest} = add i64 0, 0").unwrap();
        return;
    }

    let llvm_ty = llvm_type_str(result_type, ctx.struct_map);
    writeln!(out, "  %{dest} = load {llvm_ty}, ptr %{dest}.box").unwrap();
}

/// Emit call arguments using the callee's function signature for type info.
pub(crate) fn emit_call_args_typed(
    args: &[Operand],
    callee_name: &str,
    func: &Function,
    ctx: &EmitCtx,
) -> String {
    let sig = ctx.fn_sigs.get(callee_name);
    args.iter()
        .enumerate()
        .map(|(i, a)| {
            // Function references (e.g. passing `greet_handler` as a callback)
            // must use @name (global) instead of %name (local variable).
            let val = if let Operand::Var(name) = a {
                if ctx.fn_sigs.contains_key(name.as_str()) {
                    format!("@{name}")
                } else {
                    operand_ref(a, func)
                }
            } else {
                operand_ref(a, func)
            };
            // Use callee's param type if available, otherwise infer
            let ty = if let Some(sig) = sig {
                if let Some(param_ty) = sig.param_types.get(i) {
                    llvm_type_str(param_ty, ctx.struct_map)
                } else {
                    infer_operand_type(a, ctx)
                }
            } else {
                infer_operand_type(a, ctx)
            };
            format!("{ty} {val}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}
