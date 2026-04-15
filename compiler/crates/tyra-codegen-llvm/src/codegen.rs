// LLVM IR text generation from MIR.
//
// Generates valid LLVM IR text that can be compiled with:
//   clang output.ll -o output
//
// For Milestone 1a, we use C library functions (puts, printf) for I/O.
// The Tyra runtime will replace these in later milestones.

use std::fmt::Write;

use tyra_mir::*;
use tyra_types::Ty;

/// Generate LLVM IR text from a MIR program.
pub fn emit_llvm_ir(program: &Program) -> String {
    let mut out = String::new();

    // Module header
    writeln!(out, "; Tyra compiler output").unwrap();
    writeln!(out, "target triple = \"{}\"", target_triple()).unwrap();
    writeln!(out).unwrap();

    // String constants
    for (idx, s) in program.string_constants.iter().enumerate() {
        let escaped = llvm_escape_string(s);
        // +1 for null terminator
        let len = s.len() + 1;
        writeln!(
            out,
            "@.str.{idx} = private unnamed_addr constant [{len} x i8] c\"{escaped}\\00\""
        )
        .unwrap();
    }
    if !program.string_constants.is_empty() {
        writeln!(out).unwrap();
    }

    // Format string for print (no newline)
    writeln!(
        out,
        "@.fmt.str = private unnamed_addr constant [3 x i8] c\"%s\\00\""
    )
    .unwrap();
    writeln!(out).unwrap();

    // External declarations
    writeln!(out, "; External declarations").unwrap();
    writeln!(out, "declare i32 @puts(ptr)").unwrap();
    writeln!(out, "declare i32 @printf(ptr, ...)").unwrap();
    writeln!(out).unwrap();

    // Functions
    for func in &program.functions {
        emit_function(&mut out, func, &program.string_constants);
        writeln!(out).unwrap();
    }

    out
}

fn emit_function(out: &mut String, func: &Function, strings: &[String]) {
    let ret_ty = llvm_type(&func.return_type, func.is_main);

    // Function signature
    let params: Vec<String> = func
        .params
        .iter()
        .map(|(name, ty)| format!("{} %{name}", llvm_type(ty, false)))
        .collect();

    if func.is_main {
        writeln!(out, "define i32 @main() {{").unwrap();
    } else {
        writeln!(
            out,
            "define {ret_ty} @{}({}) {{",
            func.name,
            params.join(", ")
        )
        .unwrap();
    }

    writeln!(out, "entry:").unwrap();

    // Allocate parameter copies for mutation support
    for (name, ty) in &func.params {
        let lt = llvm_type(ty, false);
        writeln!(out, "  %{name}.addr = alloca {lt}").unwrap();
        writeln!(out, "  store {lt} %{name}, ptr %{name}.addr").unwrap();
    }

    // Emit instructions
    for inst in &func.body {
        emit_instruction(out, inst, func, strings);
    }

    writeln!(out, "}}").unwrap();
}

fn emit_instruction(out: &mut String, inst: &Instruction, func: &Function, strings: &[String]) {
    match inst {
        Instruction::Const { dest, value } => match value {
            Constant::Int(n) => {
                writeln!(out, "  %{dest} = add i64 {n}, 0").unwrap();
            }
            Constant::Float(f) => {
                writeln!(out, "  %{dest} = fadd double {f:e}, 0.0").unwrap();
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
                // Unit has no runtime representation; emit a dummy
                writeln!(out, "  ; {dest} = unit (no-op)").unwrap();
            }
        },

        Instruction::Call {
            dest,
            func: fname,
            args,
        } => {
            let args_str = emit_call_args(args, func, strings);

            // Map Tyra builtins to C functions
            match fname.as_str() {
                "print" | "eprint" => {
                    // print: no trailing newline — use printf("%s", value)
                    if let Some(d) = dest {
                        writeln!(
                            out,
                            "  %{d} = call i32 (ptr, ...) @printf(ptr @.fmt.str, {args_str})"
                        )
                        .unwrap();
                    } else {
                        writeln!(
                            out,
                            "  call i32 (ptr, ...) @printf(ptr @.fmt.str, {args_str})"
                        )
                        .unwrap();
                    }
                }
                "println" | "eprintln" => {
                    // println: adds trailing newline — use puts()
                    if let Some(d) = dest {
                        writeln!(out, "  %{d} = call i32 @puts({args_str})").unwrap();
                    } else {
                        writeln!(out, "  call i32 @puts({args_str})").unwrap();
                    }
                }
                _ => {
                    // User-defined function call
                    if let Some(d) = dest {
                        writeln!(out, "  %{d} = call i64 @{fname}({args_str})").unwrap();
                    } else {
                        writeln!(out, "  call i64 @{fname}({args_str})").unwrap();
                    }
                }
            }
        }

        Instruction::BinOp { dest, op, lhs, rhs } => {
            let l = operand_ref(lhs, func);
            let r = operand_ref(rhs, func);
            let instr = match op {
                MirBinOp::AddInt => format!("add i64 {l}, {r}"),
                MirBinOp::SubInt => format!("sub i64 {l}, {r}"),
                MirBinOp::MulInt => format!("mul i64 {l}, {r}"),
                MirBinOp::DivInt => format!("sdiv i64 {l}, {r}"),
                MirBinOp::AddFloat => format!("fadd double {l}, {r}"),
                MirBinOp::SubFloat => format!("fsub double {l}, {r}"),
                MirBinOp::MulFloat => format!("fmul double {l}, {r}"),
                MirBinOp::DivFloat => format!("fdiv double {l}, {r}"),
                MirBinOp::EqInt => format!("icmp eq i64 {l}, {r}"),
                MirBinOp::NeqInt => format!("icmp ne i64 {l}, {r}"),
                MirBinOp::LtInt => format!("icmp slt i64 {l}, {r}"),
                MirBinOp::LeInt => format!("icmp sle i64 {l}, {r}"),
                MirBinOp::GtInt => format!("icmp sgt i64 {l}, {r}"),
                MirBinOp::GeInt => format!("icmp sge i64 {l}, {r}"),
                MirBinOp::LtFloat => format!("fcmp olt double {l}, {r}"),
                MirBinOp::LeFloat => format!("fcmp ole double {l}, {r}"),
                MirBinOp::GtFloat => format!("fcmp ogt double {l}, {r}"),
                MirBinOp::GeFloat => format!("fcmp oge double {l}, {r}"),
                MirBinOp::And => format!("and i1 {l}, {r}"),
                MirBinOp::Or => format!("or i1 {l}, {r}"),
            };
            writeln!(out, "  %{dest} = {instr}").unwrap();
        }

        Instruction::Neg { dest, operand } => {
            let v = operand_ref(operand, func);
            writeln!(out, "  %{dest} = sub i64 0, {v}").unwrap();
        }

        Instruction::Not { dest, operand } => {
            let v = operand_ref(operand, func);
            writeln!(out, "  %{dest} = xor i1 {v}, 1").unwrap();
        }

        Instruction::Copy { dest, source } => {
            // In LLVM IR, we just alias the value
            if is_param(source, func) {
                let lt = "i64"; // simplified
                writeln!(out, "  %{dest} = load {lt}, ptr %{source}.addr").unwrap();
            } else {
                writeln!(out, "  ; copy {dest} = {source}").unwrap();
            }
        }

        Instruction::Return { value } => {
            if func.is_main {
                writeln!(out, "  ret i32 0").unwrap();
            } else {
                match value {
                    Some(v) => {
                        let ret_ty = llvm_type(&func.return_type, false);
                        let val = operand_ref(v, func);
                        writeln!(out, "  ret {ret_ty} {val}").unwrap();
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
            let entries: Vec<String> = branches
                .iter()
                .map(|(val, label)| format!("[{}, %{label}]", operand_ref(val, func)))
                .collect();
            writeln!(out, "  %{dest} = phi i64 {}", entries.join(", ")).unwrap();
        }
    }
}

fn emit_call_args(args: &[Operand], func: &Function, _strings: &[String]) -> String {
    args.iter()
        .map(|a| {
            let val = operand_ref(a, func);
            format!("ptr {val}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn operand_ref(op: &Operand, func: &Function) -> String {
    match op {
        Operand::Var(name) => {
            if is_param(name, func) {
                // Params are loaded from their alloca
                format!("%{name}")
            } else {
                format!("%{name}")
            }
        }
        Operand::Const(c) => match c {
            Constant::Int(n) => n.to_string(),
            Constant::Float(f) => format!("{f:e}"),
            Constant::Bool(b) => {
                if *b {
                    "1".into()
                } else {
                    "0".into()
                }
            }
            Constant::StringRef(_) => "null".into(),
            Constant::Unit => "void".into(),
        },
    }
}

fn is_param(name: &str, func: &Function) -> bool {
    func.params.iter().any(|(n, _)| n == name)
}

fn llvm_type(ty: &Ty, is_main: bool) -> &'static str {
    if is_main {
        return "i32";
    }
    match ty {
        Ty::Int => "i64",
        Ty::Float => "double",
        Ty::Bool => "i1",
        Ty::String => "ptr",
        Ty::Unit => "void",
        Ty::Never => "void",
        _ => "i64", // fallback for unresolved types
    }
}

fn llvm_escape_string(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'\n' => out.push_str("\\0A"),
            b'\r' => out.push_str("\\0D"),
            b'\t' => out.push_str("\\09"),
            b'\\' => out.push_str("\\5C"),
            b'"' => out.push_str("\\22"),
            0 => out.push_str("\\00"),
            0x20..=0x7e => out.push(b as char),
            _ => write!(out, "\\{b:02X}").unwrap(),
        }
    }
    out
}

fn target_triple() -> &'static str {
    if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "arm64-apple-macosx14.0.0"
        } else {
            "x86_64-apple-macosx14.0.0"
        }
    } else if cfg!(target_os = "linux") {
        "x86_64-unknown-linux-gnu"
    } else {
        "x86_64-unknown-unknown"
    }
}
