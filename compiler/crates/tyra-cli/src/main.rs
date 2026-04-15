// tyra CLI: the Tyra language compiler.
//
// Usage:
//   tyra run <file.tyra>       Compile and run a Tyra program
//   tyra build <file.tyra>     Compile to a native binary
//   tyra emit-ir <file.tyra>   Emit LLVM IR to stdout
//   tyra --version             Show version info
//
// spec reference: §18 (toolchain)

use std::path::Path;
use std::process;

fn main() {
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
    eprintln!("  run <file.tyra>          compile and run a Tyra program");
    eprintln!("  build <file.tyra>        compile to a native binary");
    eprintln!("  emit-ir <file.tyra>      emit LLVM IR to stdout");
    eprintln!("  --version                show version info");
    eprintln!("  --help                   show this help");
}
