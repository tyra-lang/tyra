// tyra CLI: the Tyra language compiler.
//
// Usage:
//   tyra check <file.tyra>     Type-check without codegen
//   tyra run <file.tyra>       Compile and run a Tyra program
//   tyra build <file.tyra>     Compile to a native binary
//   tyra emit-ir <file.tyra>   Emit LLVM IR to stdout
//   tyra --version             Show version info
//
// spec reference: §18 (toolchain)

use std::path::Path;
use std::process;

// Forces cargo to build the tyra-runtime staticlib (libtyra_runtime.a)
// alongside this binary so the driver can link it into Tyra programs.
// The rlib is not used from Rust-side code.
use tyra_runtime as _;

fn main() {
    // Catch MIR panics (e.g. E0204 unknown module function) and present them
    // as a clean diagnostic rather than the default "thread 'main' panicked"
    // backtrace dump. Strips the location-prefix that `panic!` prepends so a
    // user sees the canonical "error[E0xxx]: ..." line.
    std::panic::set_hook(Box::new(|info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "internal compiler error".to_string());
        if let Some(rest) = msg.strip_prefix("[E") {
            if let Some(close) = rest.find(']') {
                let code = &rest[..close];
                let body = rest[close + 1..].trim_start_matches([':', ' ']);
                eprintln!("error[E{code}]: {body}");
                return;
            }
        }
        eprintln!("error: {msg}");
    }));

    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        print_usage();
        process::exit(1);
    }

    match args[1].as_str() {
        "--version" | "-V" => {
            println!("tyra 0.1.0");
            println!("implementing language spec 0.1");
        }
        "--help" | "-h" => {
            print_usage();
        }
        "run" => {
            if args.len() < 3 {
                eprintln!("error: `tyra run` requires a source file");
                eprintln!("usage: tyra run <file.tyra>");
                process::exit(1);
            }
            let path = Path::new(&args[2]);
            let result = tyra_driver::run(path);
            if result.report.has_errors() {
                eprint!("{}", result.report.render(&result.sources));
                process::exit(1);
            }
        }
        "build" => {
            if args.len() < 3 {
                eprintln!("error: `tyra build` requires a source file");
                eprintln!("usage: tyra build <file.tyra> [-o output]");
                process::exit(1);
            }
            let source_path = Path::new(&args[2]);
            let output_path = if args.len() >= 5 && args[3] == "-o" {
                Path::new(&args[4]).to_path_buf()
            } else {
                source_path.with_extension("")
            };

            let result = tyra_driver::compile_to_binary(source_path, &output_path);
            if result.report.has_errors() {
                eprint!("{}", result.report.render(&result.sources));
                process::exit(1);
            }
            println!("compiled to {}", output_path.display());
        }
        "check" => {
            if args.len() < 3 {
                eprintln!("error: `tyra check` requires a source file");
                eprintln!("usage: tyra check <file.tyra>");
                process::exit(1);
            }
            let path = Path::new(&args[2]);
            let source = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: cannot read {}: {e}", path.display());
                    process::exit(1);
                }
            };
            // Full display path (not bare basename) is intentional: diagnostics
            // printed by `tyra check` appear in CI/script output where the path
            // context is useful. Other subcommands (run/build) use the basename
            // internally but those paths are not surfaced to stdout.
            let file_name = path.display().to_string();
            // path.parent() returns Some("") for bare filenames; treat that as
            // the current working directory so import resolution works correctly.
            let parent = path.parent().unwrap_or(Path::new("."));
            let workspace_dir = if parent.as_os_str().is_empty() {
                Some(Path::new("."))
            } else {
                Some(parent)
            };
            let tyra_driver::CheckResult { report, sources, .. } =
                tyra_driver::check_in_memory(file_name, source, workspace_dir);
            if report.has_errors() {
                eprint!("{}", report.render(&sources));
                process::exit(1);
            }
        }
        "emit-ir" => {
            if args.len() < 3 {
                eprintln!("error: `tyra emit-ir` requires a source file");
                process::exit(1);
            }
            let path = Path::new(&args[2]);
            let result = tyra_driver::compile_to_ir(path);
            if result.report.has_errors() {
                eprint!("{}", result.report.render(&result.sources));
                process::exit(1);
            }
            if let Some(ir) = &result.llvm_ir {
                print!("{ir}");
            }
        }
        cmd => {
            eprintln!("error: unknown command `{cmd}`");
            print_usage();
            process::exit(1);
        }
    }
}

fn print_usage() {
    eprintln!("tyra 0.1.0 — the Tyra language compiler");
    eprintln!();
    eprintln!("usage: tyra <command> [options]");
    eprintln!();
    eprintln!("commands:");
    eprintln!("  check <file.tyra>        type-check without codegen (exit 0 = clean)");
    eprintln!("  run <file.tyra>          compile and run a Tyra program");
    eprintln!("  build <file.tyra>        compile to a native binary");
    eprintln!("  emit-ir <file.tyra>      emit LLVM IR to stdout");
    eprintln!("  --version                show version info");
    eprintln!("  --help                   show this help");
}
