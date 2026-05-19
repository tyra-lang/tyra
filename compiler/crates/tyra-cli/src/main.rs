// tyra CLI: the Tyra language compiler.
//
// Usage:
//   tyra check <file.tyra>               Type-check without codegen
//   tyra run <file.tyra>                 Compile and run a Tyra program
//   tyra build <file.tyra>               Compile to a native binary
//   tyra emit-ir <file.tyra>             Emit LLVM IR to stdout
//   tyra fmt [--check] <file.tyra|dir>   Format source (--check: exit 1 if changed)
//   tyra test [path]                     Run *_test.tyra files (default: .)
//   tyra new [--lib] <name>              Scaffold a new project
//   tyra mod init [--name <name>]        Create Tyra.toml for an existing directory
//   tyra mod add <name> --path <path>    Add a path dependency
//   tyra mod add <name> --git <url> --rev <rev>  Add a git dependency
//   tyra mod tree                        Show the dependency tree
//   tyra mod sync                        Clone git deps into ~/.tyra/cache/git/
//   tyra --version                       Show version info
//
// spec reference: §18 (toolchain)

use std::path::{Path, PathBuf};
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
            println!("tyra {}", env!("CARGO_PKG_VERSION"));
            println!("implementing language spec 0.2");
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
        "fmt" => {
            let rest = &args[2..];
            let check_only = rest.first().map(|s| s.as_str()) == Some("--check");
            let file_arg = if check_only { rest.get(1) } else { rest.first() };
            let path = match file_arg {
                Some(p) => Path::new(p),
                None => {
                    eprintln!("error: `tyra fmt` requires a source file or directory");
                    eprintln!("usage: tyra fmt [--check] <file.tyra|dir>");
                    process::exit(1);
                }
            };
            let files: Vec<std::path::PathBuf> = if path.is_dir() {
                match collect_tyra_files(path) {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("error: cannot walk {}: {e}", path.display());
                        process::exit(1);
                    }
                }
            } else {
                vec![path.to_path_buf()]
            };
            let mut any_would_change = false;
            for file in &files {
                let src = match std::fs::read_to_string(file) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("error: cannot read {}: {e}", file.display());
                        process::exit(1);
                    }
                };
                let formatted = match tyra_fmt::fmt_source(&src) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("error: {}: {e}", file.display());
                        process::exit(1);
                    }
                };
                if check_only {
                    if src != formatted {
                        eprintln!("{}: would reformat", file.display());
                        any_would_change = true;
                    }
                } else if src != formatted {
                    if let Err(e) = std::fs::write(file, &formatted) {
                        eprintln!("error: cannot write {}: {e}", file.display());
                        process::exit(1);
                    }
                }
            }
            if check_only && any_would_change {
                process::exit(1);
            }
        }
        "test" => {
            let path = args
                .get(2)
                .map(|s| PathBuf::from(s))
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let test_files: Vec<PathBuf> = if path.is_file() {
                if is_test_file(&path) {
                    vec![path.clone()]
                } else {
                    eprintln!("error: {} is not a *_test.tyra file", path.display());
                    process::exit(1);
                }
            } else if path.is_dir() {
                match collect_test_files(&path) {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("error: cannot walk {}: {e}", path.display());
                        process::exit(1);
                    }
                }
            } else {
                eprintln!("error: {} not found", path.display());
                process::exit(1);
            };

            if test_files.is_empty() {
                eprintln!("no *_test.tyra files found in {}", path.display());
                process::exit(0);
            }

            let mut total_pass: usize = 0;
            let mut total_fail: usize = 0;
            for test_file in &test_files {
                let (p, f) = run_test_file(test_file);
                total_pass += p;
                total_fail += f;
            }
            eprintln!("\n{} passed, {} failed", total_pass, total_fail);
            if total_fail > 0 {
                process::exit(1);
            }
        }
        "new" => {
            let rest = &args[2..];
            let mut lib_flag = false;
            let mut positional: Vec<&str> = Vec::new();
            for arg in rest {
                if arg.as_str() == "--lib" {
                    lib_flag = true;
                } else if arg.starts_with("--") {
                    eprintln!("error: unknown flag `{arg}`");
                    eprintln!("usage: tyra new [--lib] <name>");
                    process::exit(1);
                } else {
                    positional.push(arg.as_str());
                }
            }
            if positional.len() > 1 {
                eprintln!("error: unexpected argument `{}`", positional[1]);
                eprintln!("usage: tyra new [--lib] <name>");
                process::exit(1);
            }
            let name = match positional.first() {
                Some(n) => *n,
                None => {
                    eprintln!("error: `tyra new` requires a project name");
                    eprintln!("usage: tyra new [--lib] <name>");
                    process::exit(1);
                }
            };
            let kind = if lib_flag {
                tyra_new::ProjectKind::Lib
            } else {
                tyra_new::ProjectKind::Bin
            };
            let dest = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            match tyra_new::create_project(name, kind, &dest) {
                Ok(()) => {
                    let type_label = if lib_flag { "lib" } else { "bin" };
                    println!("created {type_label} project `{name}`");
                    println!("  {name}/Tyra.toml");
                    println!("  {name}/src/{name}.tyra");
                    println!("  {name}/.gitignore");
                    println!("  {name}/README.md");
                }
                Err(tyra_new::NewError::AlreadyExists(p)) => {
                    eprintln!("error: directory already exists: {}", p.display());
                    process::exit(1);
                }
                Err(tyra_new::NewError::InvalidName(n)) => {
                    eprintln!(
                        "error: invalid package name `{n}`: must start with a lowercase \
                         letter, contain only lowercase letters, digits, and underscores, \
                         and must not be a reserved word"
                    );
                    process::exit(1);
                }
                Err(tyra_new::NewError::Io(e)) => {
                    eprintln!("error: {e}");
                    process::exit(1);
                }
            }
        }
        "mod" => {
            let sub = args.get(2).map(|s| s.as_str()).unwrap_or("");
            match sub {
                "init" => {
                    let rest = &args[3..];
                    let mut name_arg: Option<&str> = None;
                    let mut i = 0;
                    while i < rest.len() {
                        if rest[i] == "--name" {
                            i += 1;
                            match rest.get(i) {
                                Some(v) if !v.starts_with("--") => {
                                    name_arg = Some(v.as_str());
                                }
                                Some(v) => {
                                    eprintln!("error: `--name` requires a value, got `{v}`");
                                    eprintln!("usage: tyra mod init [--name <name>]");
                                    process::exit(1);
                                }
                                None => {
                                    eprintln!("error: `--name` requires a value");
                                    eprintln!("usage: tyra mod init [--name <name>]");
                                    process::exit(1);
                                }
                            }
                        } else if rest[i].starts_with("--") {
                            eprintln!("error: unknown flag `{}`", rest[i]);
                            eprintln!("usage: tyra mod init [--name <name>]");
                            process::exit(1);
                        } else {
                            eprintln!("error: unexpected argument `{}`", rest[i]);
                            eprintln!("usage: tyra mod init [--name <name>]");
                            process::exit(1);
                        }
                        i += 1;
                    }
                    let dest = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    match tyra_pkg::run_init(&dest, name_arg) {
                        Ok(()) => {
                            let displayed_name = name_arg.unwrap_or(
                                dest.file_name().and_then(|s| s.to_str()).unwrap_or("unnamed"),
                            );
                            println!("initialized package `{displayed_name}`");
                            println!("  Tyra.toml");
                        }
                        Err(e) => {
                            eprintln!("error: {e}");
                            process::exit(1);
                        }
                    }
                }
                "add" => {
                    let rest = &args[3..];
                    let mut dep_name: Option<&str> = None;
                    let mut path_val: Option<String> = None;
                    let mut git_val: Option<String> = None;
                    let mut rev_val: Option<String> = None;
                    let mut i = 0;
                    while i < rest.len() {
                        match rest[i].as_str() {
                            "--path" => {
                                i += 1;
                                path_val = rest.get(i).map(|s| s.clone());
                            }
                            "--git" => {
                                i += 1;
                                git_val = rest.get(i).map(|s| s.clone());
                            }
                            "--rev" => {
                                i += 1;
                                rev_val = rest.get(i).map(|s| s.clone());
                            }
                            a if a.starts_with("--") => {
                                eprintln!("error: unknown flag `{a}`");
                                eprintln!(
                                    "usage: tyra mod add <name> --path <path>\n\
                                     usage: tyra mod add <name> --git <url> --rev <rev>"
                                );
                                process::exit(1);
                            }
                            a => {
                                if dep_name.is_some() {
                                    eprintln!("error: unexpected argument `{a}`");
                                    process::exit(1);
                                }
                                dep_name = Some(a);
                            }
                        }
                        i += 1;
                    }
                    let dep_name = match dep_name {
                        Some(n) => n,
                        None => {
                            eprintln!("error: `tyra mod add` requires a dependency name");
                            eprintln!(
                                "usage: tyra mod add <name> --path <path>\n\
                                 usage: tyra mod add <name> --git <url> --rev <rev>"
                            );
                            process::exit(1);
                        }
                    };
                    let source = match (path_val, git_val, rev_val) {
                        (Some(p), None, _) => tyra_pkg::DepSource::Path(p),
                        (None, Some(url), Some(rev)) => {
                            tyra_pkg::DepSource::Git { url, rev }
                        }
                        (None, Some(_), None) => {
                            eprintln!("error: `--git` requires `--rev <commit-sha-or-tag>`");
                            process::exit(1);
                        }
                        (Some(_), Some(_), _) => {
                            eprintln!("error: specify either `--path` or `--git`, not both");
                            process::exit(1);
                        }
                        (None, None, _) => {
                            eprintln!(
                                "error: specify `--path <path>` or `--git <url> --rev <rev>`"
                            );
                            process::exit(1);
                        }
                    };
                    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    match tyra_pkg::run_add_from(&cwd, dep_name, source) {
                        Ok(()) => println!("added dependency `{dep_name}`"),
                        Err(e) => {
                            eprintln!("error: {e}");
                            process::exit(1);
                        }
                    }
                }
                "tree" => {
                    if args.len() > 3 {
                        eprintln!("error: `tyra mod tree` takes no arguments");
                        process::exit(1);
                    }
                    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    match tyra_pkg::run_tree_from(&cwd) {
                        Ok(tree) => print!("{tree}"),
                        Err(e) => {
                            eprintln!("error: {e}");
                            process::exit(1);
                        }
                    }
                }
                "sync" => {
                    if args.len() > 3 {
                        eprintln!("error: `tyra mod sync` takes no arguments");
                        process::exit(1);
                    }
                    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    match tyra_pkg::run_sync_from(&cwd) {
                        Ok(report) => {
                            if report.synced.is_empty()
                                && report.cached.is_empty()
                                && report.skipped.is_empty()
                            {
                                println!("nothing to sync (no dependencies declared)");
                            } else {
                                print!("{report}");
                            }
                        }
                        Err(e) => {
                            eprintln!("error: {e}");
                            process::exit(1);
                        }
                    }
                }
                "" => {
                    eprintln!("usage: tyra mod <init|add|tree|sync>");
                    process::exit(1);
                }
                cmd => {
                    eprintln!("error: unknown mod subcommand `{cmd}`");
                    eprintln!("usage: tyra mod <init|add|tree|sync>");
                    process::exit(1);
                }
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
    eprintln!("tyra {} — the Tyra language compiler", env!("CARGO_PKG_VERSION"));
    eprintln!();
    eprintln!("usage: tyra <command> [options]");
    eprintln!();
    eprintln!("commands:");
    eprintln!("  check <file.tyra>                        type-check without codegen (exit 0 = clean)");
    eprintln!("  run <file.tyra>                          compile and run a Tyra program");
    eprintln!("  build <file.tyra>                        compile to a native binary");
    eprintln!("  emit-ir <file.tyra>                      emit LLVM IR to stdout");
    eprintln!("  fmt [--check] <file.tyra|dir>            format source in-place; accepts a directory");
    eprintln!("  test [path]                              run *_test.tyra files (default: current dir)");
    eprintln!("  new [--lib] <name>                       scaffold a new project in the current directory");
    eprintln!("  mod init [--name <name>]                 create Tyra.toml for an existing directory");
    eprintln!("  mod add <name> --path <path>             add a path dependency");
    eprintln!("  mod add <name> --git <url> --rev <rev>   add a git dependency");
    eprintln!("  mod tree                                 show the dependency tree");
    eprintln!("  mod sync                                 clone git deps into ~/.tyra/cache/git/");
    eprintln!("  --version                                show version info");
    eprintln!("  --help                                   show this help");
}

// ─── Test runner helpers ──────────────────────────────────────────────────────

fn is_test_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.ends_with("_test.tyra"))
        .unwrap_or(false)
}

/// Recursively collect all `*_test.tyra` files under `dir`.
fn collect_test_files(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(collect_test_files(&path)?);
        } else if is_test_file(&path) {
            files.push(path);
        }
    }
    Ok(files)
}

/// Scan the AST for functions named `test_*` with no parameters.
fn find_test_fns(ast: &tyra_ast::SourceFile) -> Vec<String> {
    ast.items
        .iter()
        .filter_map(|item| {
            if let tyra_ast::Item::FnDef(f) = item {
                if f.name.starts_with("test_")
                    && f.params.is_empty()
                    && f.self_param.is_none()
                {
                    return Some(f.name.clone());
                }
            }
            None
        })
        .collect()
}

/// Build a complete Tyra source that appends a synthesized `fn main` calling
/// each `test_*` function and printing TAP-compatible output to stdout.
fn synthesize_runner(test_source: &str, test_fns: &[String]) -> String {
    let n = test_fns.len();
    let mut out = String::from(test_source);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("\nfn main() -> Unit\n");
    out.push_str("  println(\"TAP version 14\")\n");
    out.push_str(&format!("  println(\"1..{n}\")\n"));
    for (i, name) in test_fns.iter().enumerate() {
        let seq = i + 1;
        out.push_str(&format!("  match {name}()\n"));
        out.push_str(&format!("  when Ok(_)\n"));
        out.push_str(&format!("    println(\"ok {seq} - {name}\")\n"));
        out.push_str(&format!("  when Err(msg)\n"));
        out.push_str(&format!("    println(\"not ok {seq} - {name}\")\n"));
        out.push_str(&format!("    println(\"# #{{msg}}\")\n"));
        out.push_str("  end\n");
    }
    out.push_str("end\n");
    out
}

/// Parse TAP output, stream it to stdout, and return (pass_count, fail_count).
fn parse_tap_output(output: &str) -> (usize, usize) {
    let mut pass = 0usize;
    let mut fail = 0usize;
    for line in output.lines() {
        println!("{line}");
        if line.starts_with("not ok ") {
            fail += 1;
        } else if line.starts_with("ok ") {
            pass += 1;
        }
    }
    (pass, fail)
}

/// Compile and run a single `*_test.tyra` file; return (pass, fail) counts.
fn run_test_file(test_file: &Path) -> (usize, usize) {
    let source = match std::fs::read_to_string(test_file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", test_file.display());
            return (0, 1);
        }
    };

    let dir = test_file.parent().unwrap_or(Path::new("."));
    let workspace_dir = if dir.as_os_str().is_empty() {
        Some(Path::new("."))
    } else {
        Some(dir)
    };

    // Parse to discover test functions (check-only, no codegen).
    let check = tyra_driver::check_in_memory(
        test_file.to_string_lossy().into_owned(),
        source.clone(),
        workspace_dir,
    );
    if check.report.has_errors() {
        eprint!("{}", check.report.render(&check.sources));
        return (0, 1);
    }

    let test_fns = find_test_fns(&check.ast);
    if test_fns.is_empty() {
        eprintln!(
            "warning: no test_* functions found in {}",
            test_file.display()
        );
        return (0, 0);
    }

    // Reject files that would produce an invalid synthesized runner.
    let has_main = check.ast.items.iter().any(|item| {
        if let tyra_ast::Item::FnDef(f) = item {
            f.name == "main"
        } else {
            false
        }
    });
    let has_top_level_stmts = check
        .ast
        .items
        .iter()
        .any(|item| matches!(item, tyra_ast::Item::Stmt(_)));
    if has_main || has_top_level_stmts {
        eprintln!(
            "error[E0216]: {}: *_test.tyra files must not contain fn main or top-level executable statements",
            test_file.display()
        );
        return (0, 1);
    }

    eprintln!("\n# {}", test_file.display());

    // Write synthesized runner alongside the test file so import resolution works.
    let runner_name = format!("__tyra_test_runner_{}.tyra", std::process::id());
    let runner_path = dir.join(&runner_name);
    let runner_source = synthesize_runner(&source, &test_fns);
    if let Err(e) = std::fs::write(&runner_path, &runner_source) {
        eprintln!("error: cannot write runner: {e}");
        return (0, 1);
    }

    let result = tyra_driver::run_and_capture(&runner_path);
    let _ = std::fs::remove_file(&runner_path);

    if result.report.has_errors() {
        eprint!("{}", result.report.render(&result.sources));
        return (0, test_fns.len());
    }

    // Non-zero exit means the binary crashed (panic, abort, OOM, etc.).
    // Parse whatever TAP lines were emitted before the crash, then ensure at
    // least one failure is recorded regardless of how many TAP lines appeared.
    let stdout = result.stdout.unwrap_or_default();
    let (tap_pass, tap_fail) = parse_tap_output(&stdout);
    if result.exit_code != Some(0) {
        let accounted = tap_pass + tap_fail;
        let unaccounted = test_fns.len().saturating_sub(accounted);
        eprintln!("# binary exited with code {:?}", result.exit_code);
        if unaccounted > 0 {
            eprintln!("# {} test(s) did not run", unaccounted);
        }
        // Even if all TAP lines were emitted, treat the run as failed.
        return (tap_pass, tap_fail + unaccounted.max(1));
    }
    (tap_pass, tap_fail)
}

/// Recursively collect all `.tyra` files under `dir`.
/// Returns an error if any directory entry cannot be read.
fn collect_tyra_files(
    dir: &Path,
) -> Result<Vec<std::path::PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(collect_tyra_files(&path)?);
        } else if path.extension().and_then(|e| e.to_str()) == Some("tyra") {
            files.push(path);
        }
    }
    Ok(files)
}
