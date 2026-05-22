// tyra CLI: the Tyra language compiler.
//
// Usage:
//   tyra check [<file.tyra>]                      Type-check without codegen
//   tyra run   [--release] [<file.tyra>]         Compile and run a Tyra program
//   tyra build [--release] [<file.tyra>] [-o <out>]  Compile to a native binary
//   tyra emit-ir <file.tyra>                     Emit LLVM IR to stdout
//   tyra fmt [--check] <file.tyra|dir>           Format source (--check: exit 1 if changed)
//   tyra test [--filter <pat>] [--list] [--format tap|junit] [path]
//   tyra new [--lib] [--vcs none] <name>         Scaffold a new project
//   tyra mod init [--name <name>]                Create Tyra.toml for an existing directory
//   tyra mod add <name> --path <path>            Add a path dependency
//   tyra mod add <name> --git <url> --rev <rev>  Add a git dependency
//   tyra mod remove <name>                       Remove a dependency
//   tyra mod show <name>                         Show details of a dependency
//   tyra mod tree [--json]                       Show the dependency tree
//   tyra mod sync [--check]                      Clone git deps; --check validates without mutating
//   tyra mod clean                               Remove ~/.tyra/cache/
//   tyra bench ai-gen [options]                  Run the AI-generation benchmark
//   tyra --version                               Show version info
//
// spec reference: §18 (toolchain)

use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicUsize, Ordering};

static RUN_COUNTER: AtomicUsize = AtomicUsize::new(0);

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
        if let Some(rest) = msg.strip_prefix("[E")
            && let Some(close) = rest.find(']')
        {
            let code = &rest[..close];
            let body = rest[close + 1..].trim_start_matches([':', ' ']);
            eprintln!("error[E{code}]: {body}");
            return;
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
            println!("implementing language spec 0.4");
        }
        "--help" | "-h" => {
            print_usage();
        }
        "run" => {
            let mut release = false;
            let mut file_arg: Option<&str> = None;
            for arg in &args[2..] {
                match arg.as_str() {
                    "--release" => release = true,
                    a if a.starts_with("--") => {
                        eprintln!("error: unknown flag `{a}`");
                        eprintln!("usage: tyra run [--release] [<file.tyra>]");
                        process::exit(1);
                    }
                    a => {
                        if file_arg.is_some() {
                            eprintln!("error: unexpected argument `{a}`");
                            process::exit(1);
                        }
                        file_arg = Some(a);
                    }
                }
            }
            let path = match file_arg {
                Some(f) => PathBuf::from(f),
                None => match project_entry_point() {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("error: {e}");
                        process::exit(1);
                    }
                },
            };
            let result = if release {
                tyra_driver::run_release(&path)
            } else {
                tyra_driver::run(&path)
            };
            if result.report.has_errors() {
                eprint!("{}", result.report.render(&result.sources));
                process::exit(1);
            }
        }
        "build" => {
            let mut release = false;
            let mut file_arg: Option<String> = None;
            let mut output_arg: Option<String> = None;
            let mut rest_iter = args[2..].iter().peekable();
            while let Some(arg) = rest_iter.next() {
                match arg.as_str() {
                    "--release" => release = true,
                    "-o" => {
                        let val = rest_iter.next().cloned().unwrap_or_else(|| {
                            eprintln!("error: `-o` requires an output path");
                            process::exit(1);
                        });
                        if val.starts_with("--") {
                            eprintln!("error: `-o` requires an output path, got flag `{val}`");
                            eprintln!("usage: tyra build [--release] [<file.tyra>] [-o <out>]");
                            process::exit(1);
                        }
                        output_arg = Some(val);
                    }
                    a if a.starts_with("--") => {
                        eprintln!("error: unknown flag `{a}`");
                        eprintln!("usage: tyra build [--release] [<file.tyra>] [-o <out>]");
                        process::exit(1);
                    }
                    a => {
                        if file_arg.is_some() {
                            eprintln!("error: unexpected argument `{a}`");
                            process::exit(1);
                        }
                        file_arg = Some(a.to_string());
                    }
                }
            }
            // When auto-resolving from project root, default output goes to
            // <project_root>/<name>, not <src_dir>/<name>.
            let (source_path, auto_output): (PathBuf, Option<PathBuf>) = match file_arg {
                Some(ref f) => (PathBuf::from(f), None),
                None => match project_root_and_entry() {
                    Ok((root, entry)) => {
                        let name = entry.file_stem().unwrap_or_default().to_os_string();
                        (entry, Some(root.join(name)))
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                        process::exit(1);
                    }
                },
            };
            let output_path = match output_arg {
                Some(ref o) => PathBuf::from(o),
                None => auto_output.unwrap_or_else(|| source_path.with_extension("")),
            };
            let result = if release {
                tyra_driver::compile_to_binary_release(&source_path, &output_path)
            } else {
                tyra_driver::compile_to_binary(&source_path, &output_path)
            };
            if result.report.has_errors() {
                eprint!("{}", result.report.render(&result.sources));
                process::exit(1);
            }
            let mode = if release { " (release)" } else { "" };
            println!("compiled to {}{mode}", output_path.display());
        }
        "check" => {
            let mut file_arg: Option<&str> = None;
            for arg in &args[2..] {
                match arg.as_str() {
                    a if a.starts_with("--") => {
                        eprintln!("error: unknown flag `{a}`");
                        eprintln!("usage: tyra check [<file.tyra>]");
                        process::exit(1);
                    }
                    a => {
                        if file_arg.is_some() {
                            eprintln!("error: unexpected argument `{a}`");
                            process::exit(1);
                        }
                        file_arg = Some(a);
                    }
                }
            }
            let path_buf = match file_arg {
                Some(f) => PathBuf::from(f),
                None => match project_entry_point() {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("error: {e}");
                        process::exit(1);
                    }
                },
            };
            let path = path_buf.as_path();
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
            let tyra_driver::CheckResult {
                report, sources, ..
            } = tyra_driver::check_in_memory(file_name, source, workspace_dir);
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
            let mut check_only = false;
            let mut stdin_mode = false;
            let mut file_arg: Option<&str> = None;
            for a in args[2..].iter().map(String::as_str) {
                match a {
                    "--check" => check_only = true,
                    "--stdin" => stdin_mode = true,
                    a if a.starts_with("--") => {
                        eprintln!("error: unknown flag `{a}`");
                        eprintln!("usage: tyra fmt [--check] [--stdin] <file.tyra|dir>");
                        process::exit(1);
                    }
                    a => {
                        if file_arg.is_some() {
                            eprintln!("error: unexpected argument `{a}`");
                            process::exit(1);
                        }
                        file_arg = Some(a);
                    }
                }
            }
            if stdin_mode {
                if file_arg.is_some() {
                    eprintln!("error: cannot specify both --stdin and a file path");
                    process::exit(1);
                }
                use std::io::Read;
                let mut src = String::new();
                if let Err(e) = std::io::stdin().read_to_string(&mut src) {
                    eprintln!("error: cannot read stdin: {e}");
                    process::exit(1);
                }
                let formatted = match tyra_fmt::fmt_source(&src) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("error: {e}");
                        process::exit(1);
                    }
                };
                if check_only {
                    if src != formatted {
                        eprintln!("stdin: would reformat");
                        process::exit(1);
                    }
                } else {
                    print!("{formatted}");
                }
            } else {
                let path = match file_arg {
                    Some(p) => Path::new(p),
                    None => {
                        eprintln!(
                            "error: `tyra fmt` requires a source file, directory, or --stdin"
                        );
                        eprintln!("usage: tyra fmt [--check] [--stdin] <file.tyra|dir>");
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
                    } else if src != formatted
                        && let Err(e) = std::fs::write(file, &formatted)
                    {
                        eprintln!("error: cannot write {}: {e}", file.display());
                        process::exit(1);
                    }
                }
                if check_only && any_would_change {
                    process::exit(1);
                }
            }
        }
        "test" => {
            // Parse: tyra test [--filter <pat>] [--list] [--format tap|junit]
            //                  [--timeout <secs>] [--jobs <n>] [path]
            let mut filter: Option<String> = None;
            let mut list_mode = false;
            let mut junit = false;
            let mut timeout_secs: Option<u64> = None;
            let mut jobs: usize = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1);
            let mut path_arg: Option<&str> = None;
            let mut rest = args[2..].iter().peekable();
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--filter" => {
                        filter = Some(rest.next().cloned().unwrap_or_else(|| {
                            eprintln!("error: --filter requires a pattern");
                            process::exit(1);
                        }));
                    }
                    "--list" => list_mode = true,
                    "--format" => match rest.next().map(String::as_str) {
                        Some("tap") => {}
                        Some("junit") => junit = true,
                        Some(v) => {
                            eprintln!(
                                "error: unknown --format value `{v}` (expected `tap` or `junit`)"
                            );
                            process::exit(1);
                        }
                        None => {
                            eprintln!("error: --format requires a value (tap or junit)");
                            process::exit(1);
                        }
                    },
                    "--timeout" => {
                        let v = rest.next().cloned().unwrap_or_else(|| {
                            eprintln!("error: --timeout requires a value in seconds");
                            process::exit(1);
                        });
                        timeout_secs = Some(v.parse::<u64>().unwrap_or_else(|_| {
                            eprintln!("error: --timeout value must be a positive integer");
                            process::exit(1);
                        }));
                    }
                    "--jobs" => {
                        let v = rest.next().cloned().unwrap_or_else(|| {
                            eprintln!("error: --jobs requires a value");
                            process::exit(1);
                        });
                        jobs = v.parse::<usize>().unwrap_or_else(|_| {
                            eprintln!("error: --jobs value must be a positive integer");
                            process::exit(1);
                        });
                        if jobs == 0 {
                            eprintln!("error: --jobs must be >= 1");
                            process::exit(1);
                        }
                    }
                    other if other.starts_with("--") => {
                        eprintln!("error: unknown flag `{other}` for `tyra test`");
                        eprintln!(
                            "usage: tyra test [--filter <pattern>] [--list] [--format tap|junit] [--timeout <secs>] [--jobs <n>] [path]"
                        );
                        process::exit(1);
                    }
                    other => {
                        if path_arg.is_some() {
                            eprintln!("error: unexpected argument `{other}`");
                            process::exit(1);
                        }
                        path_arg = Some(other);
                    }
                }
            }
            let path = path_arg
                .map(PathBuf::from)
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

            if list_mode {
                for test_file in &test_files {
                    list_test_fns(test_file, filter.as_deref());
                }
            } else {
                // Run test files in parallel chunks of `jobs` (deterministic output order).
                let results: Vec<FileTestOut> =
                    run_test_files_parallel(&test_files, filter.as_deref(), timeout_secs, jobs);

                if junit {
                    let mut suites: Vec<(String, Vec<TestRecord>, f64)> = Vec::new();
                    let mut total_fail: usize = 0;
                    for out in &results {
                        total_fail += out.fail;
                        let mut records = parse_tap_to_records(&out.tap);
                        // Compile/infra failures produce no TAP lines but do set fail > 0.
                        // Synthesize a failure record so the XML reflects the real outcome.
                        if records.is_empty() && out.fail > 0 {
                            records.push(TestRecord {
                                name: "infrastructure failure".to_string(),
                                passed: false,
                                failure_msg: out.diag.trim().to_string(),
                            });
                        }
                        suites.push((out.path.clone(), records, out.elapsed));
                    }
                    print!("{}", render_junit_xml(&suites));
                    if total_fail > 0 {
                        process::exit(1);
                    }
                } else {
                    let mut total_pass: usize = 0;
                    let mut total_fail: usize = 0;
                    for out in &results {
                        // Print buffered output in deterministic order.
                        eprint!("{}", out.diag);
                        if !out.header.is_empty() {
                            eprintln!("{}", out.header);
                        }
                        print!("{}", out.tap);
                        if !out.timing.is_empty() {
                            println!("{}", out.timing);
                        }
                        total_pass += out.pass;
                        total_fail += out.fail;
                    }
                    eprintln!("\n{} passed, {} failed", total_pass, total_fail);
                    if total_fail > 0 {
                        process::exit(1);
                    }
                }
            }
        }
        "new" => {
            let rest = &args[2..];
            let mut lib_flag = false;
            let mut vcs_none = false;
            let mut positional: Vec<&str> = Vec::new();
            let mut rest_iter = rest.iter().peekable();
            while let Some(arg) = rest_iter.next() {
                match arg.as_str() {
                    "--lib" => lib_flag = true,
                    "--vcs" => {
                        let val = rest_iter.next().map(String::as_str).unwrap_or("");
                        if val == "none" {
                            vcs_none = true;
                        } else {
                            eprintln!("error: unknown --vcs value `{val}` (expected `none`)");
                            process::exit(1);
                        }
                    }
                    other if other.starts_with("--") => {
                        eprintln!("error: unknown flag `{other}`");
                        eprintln!("usage: tyra new [--lib] [--vcs none] <name>");
                        process::exit(1);
                    }
                    other => positional.push(other),
                }
            }
            if positional.len() > 1 {
                eprintln!("error: unexpected argument `{}`", positional[1]);
                eprintln!("usage: tyra new [--lib] [--vcs none] <name>");
                process::exit(1);
            }
            let name = match positional.first() {
                Some(n) => *n,
                None => {
                    eprintln!("error: `tyra new` requires a project name");
                    eprintln!("usage: tyra new [--lib] [--vcs none] <name>");
                    process::exit(1);
                }
            };
            let kind = if lib_flag {
                tyra_new::ProjectKind::Lib
            } else {
                tyra_new::ProjectKind::Bin
            };
            let vcs = if vcs_none {
                tyra_new::VcsMode::None
            } else {
                tyra_new::VcsMode::Git
            };
            let dest = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            match tyra_new::create_project(name, kind, vcs, &dest) {
                Ok(()) => {
                    let type_label = if lib_flag { "lib" } else { "bin" };
                    println!("created {type_label} project `{name}`");
                    println!("  {name}/Tyra.toml");
                    println!("  {name}/src/{name}.tyra");
                    if !vcs_none {
                        println!("  {name}/.gitignore");
                    }
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
                                dest.file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("unnamed"),
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
                                path_val = rest.get(i).cloned();
                            }
                            "--git" => {
                                i += 1;
                                git_val = rest.get(i).cloned();
                            }
                            "--rev" => {
                                i += 1;
                                rev_val = rest.get(i).cloned();
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
                        (None, Some(url), Some(rev)) => tyra_pkg::DepSource::Git { url, rev },
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
                    let json_flag = args.get(3).map(String::as_str) == Some("--json");
                    if args.len() > 3 && !json_flag {
                        eprintln!("error: unknown argument `{}`", args[3]);
                        eprintln!("usage: tyra mod tree [--json]");
                        process::exit(1);
                    }
                    if args.len() > 4 {
                        eprintln!("error: unexpected argument `{}`", args[4]);
                        eprintln!("usage: tyra mod tree [--json]");
                        process::exit(1);
                    }
                    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    if json_flag {
                        match tyra_pkg::run_tree_json_from(&cwd) {
                            Ok(json) => print!("{json}"),
                            Err(e) => {
                                eprintln!("error: {e}");
                                process::exit(1);
                            }
                        }
                    } else {
                        match tyra_pkg::run_tree_from(&cwd) {
                            Ok(tree) => print!("{tree}"),
                            Err(e) => {
                                eprintln!("error: {e}");
                                process::exit(1);
                            }
                        }
                    }
                }
                "sync" => {
                    let mut check_flag = false;
                    let mut locked_flag = false;
                    let mut json_flag = false;
                    let mut quiet_flag = false;
                    for a in args[3..].iter().map(String::as_str) {
                        match a {
                            "--check" => check_flag = true,
                            "--locked" => locked_flag = true,
                            "--json" => json_flag = true,
                            "--quiet" => quiet_flag = true,
                            a => {
                                eprintln!("error: unknown argument `{a}`");
                                eprintln!(
                                    "usage: tyra mod sync [--check] [--locked] [--json] [--quiet]"
                                );
                                process::exit(1);
                            }
                        }
                    }
                    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    if check_flag {
                        match tyra_pkg::run_sync_check_from(&cwd) {
                            Ok(issues) => {
                                if issues.is_empty() {
                                    if json_flag {
                                        println!("{{\"ok\": true, \"issues\": []}}");
                                    } else if !quiet_flag {
                                        println!("all dependencies ok");
                                    }
                                } else {
                                    if json_flag {
                                        let items: Vec<String> = issues
                                            .iter()
                                            .map(|s| format!("\"{}\"", s.replace('"', "\\\"")))
                                            .collect();
                                        eprintln!(
                                            "{{\"ok\": false, \"issues\": [{}]}}",
                                            items.join(", ")
                                        );
                                    } else {
                                        for issue in &issues {
                                            eprintln!("error: {issue}");
                                        }
                                    }
                                    process::exit(1);
                                }
                            }
                            Err(e) => {
                                eprintln!("error: {e}");
                                process::exit(1);
                            }
                        }
                    } else if locked_flag {
                        match tyra_pkg::run_sync_locked_from(&cwd) {
                            Ok(report) => {
                                if json_flag {
                                    print!("{}", report.to_json());
                                } else if !quiet_flag {
                                    print!("{report}");
                                }
                            }
                            Err(e) => {
                                eprintln!("error: {e}");
                                process::exit(1);
                            }
                        }
                    } else {
                        match tyra_pkg::run_sync_from(&cwd) {
                            Ok(report) => {
                                if json_flag {
                                    print!("{}", report.to_json());
                                } else if !quiet_flag {
                                    if report.synced.is_empty()
                                        && report.cached.is_empty()
                                        && report.skipped.is_empty()
                                    {
                                        println!("nothing to sync (no dependencies declared)");
                                    } else {
                                        print!("{report}");
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("error: {e}");
                                process::exit(1);
                            }
                        }
                    }
                }
                "remove" => {
                    let dep_name = match args.get(3).map(String::as_str) {
                        Some(n) if !n.starts_with("--") => n,
                        _ => {
                            eprintln!("error: `tyra mod remove` requires a dependency name");
                            eprintln!("usage: tyra mod remove <name>");
                            process::exit(1);
                        }
                    };
                    if args.len() > 4 {
                        eprintln!("error: unexpected argument `{}`", args[4]);
                        eprintln!("usage: tyra mod remove <name>");
                        process::exit(1);
                    }
                    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    match tyra_pkg::run_remove_from(&cwd, dep_name) {
                        Ok(()) => println!("removed dependency `{dep_name}`"),
                        Err(e) => {
                            eprintln!("error: {e}");
                            process::exit(1);
                        }
                    }
                }
                "update" => {
                    let mut dep_name: Option<&str> = None;
                    let mut path_val: Option<String> = None;
                    let mut git_val: Option<String> = None;
                    let mut rev_val: Option<String> = None;
                    let rest: Vec<String> = args[3..].to_vec();
                    let mut i = 0;
                    while i < rest.len() {
                        match rest[i].as_str() {
                            "--path" => {
                                i += 1;
                                path_val = rest.get(i).cloned();
                            }
                            "--git" => {
                                i += 1;
                                git_val = rest.get(i).cloned();
                            }
                            "--rev" => {
                                i += 1;
                                rev_val = rest.get(i).cloned();
                            }
                            a if a.starts_with("--") => {
                                eprintln!("error: unknown flag `{a}`");
                                eprintln!(
                                    "usage: tyra mod update <name> --path <path>\n\
                                     usage: tyra mod update <name> --git <url> --rev <rev>"
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
                            eprintln!("error: `tyra mod update` requires a dependency name");
                            eprintln!(
                                "usage: tyra mod update <name> --path <path>\n\
                                 usage: tyra mod update <name> --git <url> --rev <rev>"
                            );
                            process::exit(1);
                        }
                    };
                    let source = match (path_val, git_val, rev_val) {
                        (Some(p), None, _) => tyra_pkg::DepSource::Path(p),
                        (None, Some(url), Some(rev)) => tyra_pkg::DepSource::Git { url, rev },
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
                    match tyra_pkg::run_update_from(&cwd, dep_name, source) {
                        Ok(()) => println!("updated dependency `{dep_name}`"),
                        Err(e) => {
                            eprintln!("error: {e}");
                            process::exit(1);
                        }
                    }
                }
                "show" => {
                    let mut dep_name: Option<&str> = None;
                    let mut json_flag = false;
                    for a in args[3..].iter().map(String::as_str) {
                        match a {
                            "--json" => json_flag = true,
                            a if a.starts_with("--") => {
                                eprintln!("error: unknown flag `{a}`");
                                eprintln!("usage: tyra mod show <name> [--json]");
                                process::exit(1);
                            }
                            name => {
                                if dep_name.is_some() {
                                    eprintln!("error: unexpected argument `{name}`");
                                    eprintln!("usage: tyra mod show <name> [--json]");
                                    process::exit(1);
                                }
                                dep_name = Some(name);
                            }
                        }
                    }
                    let dep_name = match dep_name {
                        Some(n) => n,
                        None => {
                            eprintln!("error: `tyra mod show` requires a dependency name");
                            eprintln!("usage: tyra mod show <name> [--json]");
                            process::exit(1);
                        }
                    };
                    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    if json_flag {
                        match tyra_pkg::run_show_json_from(&cwd, dep_name) {
                            Ok(json) => print!("{json}"),
                            Err(e) => {
                                eprintln!("error: {e}");
                                process::exit(1);
                            }
                        }
                    } else {
                        match tyra_pkg::run_show_from(&cwd, dep_name) {
                            Ok(info) => print!("{info}"),
                            Err(e) => {
                                eprintln!("error: {e}");
                                process::exit(1);
                            }
                        }
                    }
                }
                "clean" => {
                    if args.len() > 3 {
                        eprintln!("error: unexpected argument `{}`", args[3]);
                        eprintln!("usage: tyra mod clean");
                        process::exit(1);
                    }
                    let cache_root = tyra_pkg::tyra_cache_root();
                    match tyra_pkg::run_clean() {
                        Ok(true) => println!("cleaned cache ({})", cache_root.display()),
                        Ok(false) => println!("cache already empty"),
                        Err(e) => {
                            eprintln!("error: {e}");
                            process::exit(1);
                        }
                    }
                }
                "" => {
                    eprintln!("usage: tyra mod <init|add|remove|show|tree|sync|clean>");
                    process::exit(1);
                }
                cmd => {
                    eprintln!("error: unknown mod subcommand `{cmd}`");
                    eprintln!("usage: tyra mod <init|add|remove|show|tree|sync|clean>");
                    process::exit(1);
                }
            }
        }
        "bench" => {
            // tyra bench [<path>]               — run *_bench.tyra files (§18.8)
            // tyra bench ai-gen [<harness-args>] — AI-generation benchmark (legacy)
            let sub = args.get(2).map(String::as_str);
            if sub != Some("ai-gen") {
                // New: tyra bench [path]
                let path = sub.map(PathBuf::from).unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                });

                let bench_files: Vec<PathBuf> = if path.is_file() {
                    if is_bench_file(&path) {
                        vec![path.clone()]
                    } else {
                        eprintln!("error: {} is not a *_bench.tyra file", path.display());
                        process::exit(1);
                    }
                } else if path.is_dir() {
                    match collect_bench_files(&path) {
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

                if bench_files.is_empty() {
                    eprintln!("no *_bench.tyra files found in {}", path.display());
                    process::exit(0);
                }

                let mut any_fail = false;
                for bench_file in &bench_files {
                    if !run_bench_file(bench_file) {
                        any_fail = true;
                    }
                }
                process::exit(if any_fail { 1 } else { 0 });
            }

            // Locate bench/ai-gen/harness.py by walking up from the executable's
            // directory (installed binary) or from cwd (dev/source checkout).
            let harness = find_bench_harness();
            let harness = match harness {
                Some(p) => p,
                None => {
                    eprintln!(
                        "error: could not find bench/ai-gen/harness.py; \
                         run from the tyra repository root or install the full toolchain"
                    );
                    process::exit(1);
                }
            };

            // Forward all args after "ai-gen" to harness.py verbatim.
            let forward: Vec<&str> = args[3..].iter().map(String::as_str).collect();
            let status = std::process::Command::new("python3")
                .arg(&harness)
                .args(&forward)
                .status()
                .unwrap_or_else(|e| {
                    eprintln!("error: failed to launch python3: {e}");
                    process::exit(1);
                });
            process::exit(status.code().unwrap_or(1));
        }
        cmd => {
            eprintln!("error: unknown command `{cmd}`");
            print_usage();
            process::exit(1);
        }
    }
}

fn print_usage() {
    eprintln!(
        "tyra {} — the Tyra language compiler",
        env!("CARGO_PKG_VERSION")
    );
    eprintln!();
    eprintln!("usage: tyra <command> [options]");
    eprintln!();
    eprintln!("commands:");
    eprintln!(
        "  check [<file.tyra>]                      type-check (defaults to project entry point)"
    );
    eprintln!(
        "  run   [--release] [<file.tyra>]          compile and run (defaults to project entry point)"
    );
    eprintln!(
        "  build [--release] [<file.tyra>] [-o out] compile to binary (defaults to project entry point)"
    );
    eprintln!("  emit-ir <file.tyra>                      emit LLVM IR to stdout");
    eprintln!(
        "  fmt [--check] [--stdin] <file.tyra|dir>  format source in-place; --stdin reads stdin"
    );
    eprintln!(
        "  test [--filter <pat>] [--list]           run *_test.tyra files (default: current dir)"
    );
    eprintln!("       [--format tap|junit] [--timeout <s>] [--jobs <n>] [path]");
    eprintln!(
        "  new [--lib] [--vcs none] <name>          scaffold a new project in the current directory"
    );
    eprintln!(
        "  mod init [--name <name>]                 create Tyra.toml for an existing directory"
    );
    eprintln!("  mod add <name> --path <path>             add a path dependency");
    eprintln!("  mod add <name> --git <url> --rev <rev>   add a git dependency");
    eprintln!("  mod update <name> --path <path>          update an existing path dependency");
    eprintln!("  mod update <name> --git <url> --rev <r>  update an existing git dependency");
    eprintln!("  mod remove <name>                        remove a dependency");
    eprintln!("  mod show <name> [--json]                 show details of a dependency");
    eprintln!("  mod tree [--json]                        show the dependency tree");
    eprintln!(
        "  mod sync [--check] [--json] [--quiet]    clone git deps; --check validates without mutating"
    );
    eprintln!("  mod clean                                remove ~/.tyra/cache/");
    eprintln!("  bench [path]                             run *_bench.tyra micro-benchmarks");
    eprintln!("  bench ai-gen [options]                   run the AI-generation benchmark");
    eprintln!("  --version                                show version info");
    eprintln!("  --help                                   show this help");
}

/// Walk up from cwd (and from the executable's dir) to find bench/ai-gen/harness.py.
fn find_bench_harness() -> Option<std::path::PathBuf> {
    let relative = std::path::Path::new("bench")
        .join("ai-gen")
        .join("harness.py");

    // Try cwd walk-up first (source checkout / dev use).
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join(&relative);
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }

    // Try walk-up from the directory containing the tyra binary (installed).
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent()?.to_path_buf();
        loop {
            let candidate = dir.join(&relative);
            if candidate.is_file() {
                return Some(candidate);
            }
            if !dir.pop() {
                break;
            }
        }
    }

    None
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
            if let tyra_ast::Item::FnDef(f) = item
                && f.name.starts_with("test_")
                && f.params.is_empty()
                && f.self_param.is_none()
            {
                return Some(f.name.clone());
            }
            None
        })
        .collect()
}

/// Print test function names found in `test_file` (one per line, tab-separated
/// as `<file>\t<fn_name>`), applying an optional substring filter.
fn list_test_fns(test_file: &Path, filter: Option<&str>) {
    let source = match std::fs::read_to_string(test_file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", test_file.display());
            return;
        }
    };
    let dir = test_file.parent().unwrap_or(Path::new("."));
    let workspace_dir = if dir.as_os_str().is_empty() {
        Some(Path::new("."))
    } else {
        Some(dir)
    };
    let check = tyra_driver::check_in_memory(
        test_file.to_string_lossy().into_owned(),
        source,
        workspace_dir,
    );
    if check.report.has_errors() {
        eprint!("{}", check.report.render(&check.sources));
        return;
    }
    let fns = find_test_fns(&check.ast);
    for name in fns {
        if filter.map(|p| name.contains(p)).unwrap_or(true) {
            println!("{}\t{name}", test_file.display());
        }
    }
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
        out.push_str("  when Ok(_)\n");
        out.push_str(&format!("    println(\"ok {seq} - {name}\")\n"));
        out.push_str("  when Err(msg)\n");
        out.push_str(&format!("    println(\"not ok {seq} - {name}\")\n"));
        out.push_str("    println(\"# #{msg}\")\n");
        out.push_str("  end\n");
    }
    out.push_str("end\n");
    out
}

// ─── Parallel / buffered test runner ─────────────────────────────────────────

/// All outputs produced by running one `*_test.tyra` file, buffered for
/// deterministic ordered printing in parallel mode.
struct FileTestOut {
    path: String,
    pass: usize,
    fail: usize,
    /// `# path/to/file` header line (empty when no tests ran).
    header: String,
    /// TAP lines (stdout of the test binary).
    tap: String,
    /// Timing line (`# time: X.XXXs`).
    timing: String,
    /// Compile/infra diagnostics (stderr).
    diag: String,
    /// Wall-clock seconds spent running this file (0.0 on compile failure).
    elapsed: f64,
}

impl FileTestOut {
    fn error(path: &Path, diag: String) -> Self {
        Self {
            path: path.display().to_string(),
            pass: 0,
            fail: 1,
            header: String::new(),
            tap: String::new(),
            timing: String::new(),
            diag,
            elapsed: 0.0,
        }
    }
}

/// Run one `*_test.tyra` file, buffering all output.
/// `timeout` is the per-binary execution timeout in seconds (None = unlimited).
fn run_test_file_core(test_file: &Path, filter: Option<&str>, timeout: Option<u64>) -> FileTestOut {
    let mut diag = String::new();

    let source = match std::fs::read_to_string(test_file) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("error: cannot read {}: {e}\n", test_file.display());
            return FileTestOut::error(test_file, msg);
        }
    };

    let dir = test_file.parent().unwrap_or(Path::new("."));
    let workspace_dir = if dir.as_os_str().is_empty() {
        Some(Path::new("."))
    } else {
        Some(dir)
    };

    let check = tyra_driver::check_in_memory(
        test_file.to_string_lossy().into_owned(),
        source.clone(),
        workspace_dir,
    );
    if check.report.has_errors() {
        let rendered = check.report.render(&check.sources);
        diag.push_str(&rendered);
        return FileTestOut::error(test_file, diag);
    }

    let all_fns = find_test_fns(&check.ast);
    let test_fns: Vec<String> = if let Some(pat) = filter {
        all_fns.into_iter().filter(|n| n.contains(pat)).collect()
    } else {
        all_fns
    };

    if test_fns.is_empty() {
        let warn = if filter.is_some() {
            format!(
                "warning: no test_* functions match filter in {}\n",
                test_file.display()
            )
        } else {
            format!(
                "warning: no test_* functions found in {}\n",
                test_file.display()
            )
        };
        diag.push_str(&warn);
        return FileTestOut {
            path: test_file.display().to_string(),
            pass: 0,
            fail: 0,
            header: String::new(),
            tap: String::new(),
            timing: String::new(),
            diag,
            elapsed: 0.0,
        };
    }

    // Validate: test files must not define fn main or top-level statements.
    let has_main = check.ast.items.iter().any(|item| {
        if let tyra_ast::Item::FnDef(f) = item {
            f.name == "main"
        } else {
            false
        }
    });
    let has_top_stmts = check
        .ast
        .items
        .iter()
        .any(|item| matches!(item, tyra_ast::Item::Stmt(_)));
    if has_main || has_top_stmts {
        let msg = format!(
            "error[E0216]: {}: *_test.tyra files must not contain fn main or top-level executable statements\n",
            test_file.display()
        );
        diag.push_str(&msg);
        return FileTestOut::error(test_file, diag);
    }

    let header = format!("\n# {}", test_file.display());

    let run_id = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let runner_name = format!("__tyra_test_runner_{}_{}.tyra", std::process::id(), run_id);
    let runner_path = dir.join(&runner_name);
    let runner_source = synthesize_runner(&source, &test_fns);
    if let Err(e) = std::fs::write(&runner_path, &runner_source) {
        diag.push_str(&format!("error: cannot write runner: {e}\n"));
        return FileTestOut::error(test_file, diag);
    }

    let t0 = std::time::Instant::now();
    let result = match timeout {
        Some(secs) => tyra_driver::run_and_capture_with_timeout(&runner_path, secs),
        None => tyra_driver::run_and_capture(&runner_path),
    };
    let elapsed = t0.elapsed().as_secs_f64();
    let _ = std::fs::remove_file(&runner_path);

    if result.report.has_errors() {
        diag.push_str(&result.report.render(&result.sources));
        return FileTestOut {
            path: test_file.display().to_string(),
            pass: 0,
            fail: test_fns.len(),
            header,
            tap: String::new(),
            timing: String::new(),
            diag,
            elapsed,
        };
    }

    // Buffer stderr from the test binary into diag so parallel jobs stay ordered.
    if let Some(se) = result.stderr.as_deref()
        && !se.is_empty()
    {
        diag.push_str(se);
        if !se.ends_with('\n') {
            diag.push('\n');
        }
    }

    if result.timed_out {
        // Synthesize TAP failure lines for every test function.
        let mut tap = format!("TAP version 14\n1..{}\n", test_fns.len());
        for (i, name) in test_fns.iter().enumerate() {
            tap.push_str(&format!(
                "not ok {} - {} (timeout after {}s)\n",
                i + 1,
                name,
                timeout.unwrap_or(0)
            ));
        }
        diag.push_str(&format!(
            "# binary timed out after {}s\n",
            timeout.unwrap_or(0)
        ));
        return FileTestOut {
            path: test_file.display().to_string(),
            pass: 0,
            fail: test_fns.len(),
            header,
            tap,
            timing: format!("# time: {elapsed:.3}s"),
            diag,
            elapsed,
        };
    }

    let stdout = result.stdout.unwrap_or_default();
    let (pass, fail) = count_tap_lines(&stdout);
    let timing = format!("# time: {elapsed:.3}s");

    // Handle binary crash: account for unrun tests.
    let (tap_pass, tap_fail) = if result.exit_code != Some(0) {
        let accounted = pass + fail;
        let unaccounted = test_fns.len().saturating_sub(accounted);
        if unaccounted > 0 {
            diag.push_str(&format!("# {} test(s) did not run\n", unaccounted));
        }
        diag.push_str(&format!(
            "# binary exited with code {:?}\n",
            result.exit_code
        ));
        (pass, fail + unaccounted.max(1))
    } else {
        (pass, fail)
    };

    FileTestOut {
        path: test_file.display().to_string(),
        pass: tap_pass,
        fail: tap_fail,
        header,
        tap: stdout,
        timing,
        diag,
        elapsed,
    }
}

/// Run `test_files` in parallel chunks of `jobs`, returning results in the
/// same order as the input slice (deterministic output).
fn run_test_files_parallel(
    test_files: &[PathBuf],
    filter: Option<&str>,
    timeout: Option<u64>,
    jobs: usize,
) -> Vec<FileTestOut> {
    let jobs = jobs.max(1);
    let mut results = Vec::with_capacity(test_files.len());
    for chunk in test_files.chunks(jobs) {
        let handles: Vec<_> = chunk
            .iter()
            .map(|f| {
                let f = f.clone();
                let filter = filter.map(str::to_owned);
                std::thread::spawn(move || run_test_file_core(&f, filter.as_deref(), timeout))
            })
            .collect();
        for handle in handles {
            results.push(handle.join().unwrap_or_else(|_| FileTestOut {
                path: String::new(),
                pass: 0,
                fail: 1,
                header: String::new(),
                tap: String::new(),
                timing: String::new(),
                diag: "error: test thread panicked\n".into(),
                elapsed: 0.0,
            }));
        }
    }
    results
}

/// Recursively collect all `.tyra` files under `dir`.
/// Returns an error if any directory entry cannot be read.
fn collect_tyra_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, std::io::Error> {
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

// ─── Project root helpers ─────────────────────────────────────────────────────

/// Resolve the entry-point source file and project root from the nearest `Tyra.toml`.
/// Returns `(project_root, entry_path)`.
fn project_root_and_entry() -> Result<(PathBuf, PathBuf), String> {
    let cwd =
        std::env::current_dir().map_err(|e| format!("cannot determine working directory: {e}"))?;
    let root = tyra_manifest::find_project_root(&cwd).ok_or_else(|| {
        "no Tyra.toml found; specify a source file or run `tyra new <name>` to create a project"
            .to_string()
    })?;
    let manifest =
        tyra_manifest::load_manifest(&root).map_err(|e| format!("cannot load Tyra.toml: {e}"))?;
    let entry = root
        .join("src")
        .join(format!("{}.tyra", manifest.package.name));
    if !entry.is_file() {
        return Err(format!(
            "entry point `{}` not found; expected `src/{}.tyra`",
            entry.display(),
            manifest.package.name
        ));
    }
    Ok((root, entry))
}

/// Resolve the entry-point source file from the nearest `Tyra.toml`.
/// Used by `run`/`check` when no source file is specified.
fn project_entry_point() -> Result<PathBuf, String> {
    project_root_and_entry().map(|(_, entry)| entry)
}

// ─── JUnit output helpers ─────────────────────────────────────────────────────

struct TestRecord {
    name: String,
    passed: bool,
    failure_msg: String,
}

fn count_tap_lines(output: &str) -> (usize, usize) {
    let mut pass = 0usize;
    let mut fail = 0usize;
    for line in output.lines() {
        if line.starts_with("not ok ") {
            fail += 1;
        } else if line.starts_with("ok ") {
            pass += 1;
        }
    }
    (pass, fail)
}

fn parse_tap_to_records(tap: &str) -> Vec<TestRecord> {
    let mut records: Vec<TestRecord> = Vec::new();
    let mut last_failed: Option<usize> = None;
    for line in tap.lines() {
        if let Some(rest) = line.strip_prefix("not ok ") {
            let name = rest
                .split_once(" - ")
                .map(|(_, b)| b)
                .unwrap_or(rest)
                .to_string();
            records.push(TestRecord {
                name,
                passed: false,
                failure_msg: String::new(),
            });
            last_failed = Some(records.len() - 1);
        } else if let Some(rest) = line.strip_prefix("ok ") {
            let name = rest
                .split_once(" - ")
                .map(|(_, b)| b)
                .unwrap_or(rest)
                .to_string();
            records.push(TestRecord {
                name,
                passed: true,
                failure_msg: String::new(),
            });
            last_failed = None;
        } else if let Some(msg) = line.strip_prefix("# ")
            && let Some(idx) = last_failed
        {
            if !records[idx].failure_msg.is_empty() {
                records[idx].failure_msg.push('\n');
            }
            records[idx].failure_msg.push_str(msg);
        }
    }
    records
}

fn render_junit_xml(suites: &[(String, Vec<TestRecord>, f64)]) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites>\n");
    for (file, records, elapsed) in suites {
        let tests = records.len();
        let failures = records.iter().filter(|r| !r.passed).count();
        let classname = std::path::Path::new(file.as_str())
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(file.as_str());
        xml.push_str(&format!(
            "  <testsuite name=\"{}\" tests=\"{tests}\" failures=\"{failures}\" time=\"{elapsed:.3}\">\n",
            xml_escape(file)
        ));
        for r in records {
            let name = xml_escape(&r.name);
            let cls = xml_escape(classname);
            if r.passed {
                xml.push_str(&format!(
                    "    <testcase name=\"{name}\" classname=\"{cls}\"/>\n"
                ));
            } else {
                let msg = xml_escape(&r.failure_msg);
                xml.push_str(&format!(
                    "    <testcase name=\"{name}\" classname=\"{cls}\">\n      <failure message=\"{msg}\"/>\n    </testcase>\n"
                ));
            }
        }
        xml.push_str("  </testsuite>\n");
    }
    xml.push_str("</testsuites>\n");
    xml
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---------------------------------------------------------------------------
// tyra bench <dir> — micro-benchmark runner (§18.8, v0.4.0)
// ---------------------------------------------------------------------------

fn is_bench_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.ends_with("_bench.tyra"))
        .unwrap_or(false)
}

fn collect_bench_files(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_bench_files(&path)?);
        } else if is_bench_file(&path) {
            files.push(path);
        }
    }
    Ok(files)
}

/// Scan the AST for `bench_*` functions with no parameters.
fn find_bench_fns(ast: &tyra_ast::SourceFile) -> Vec<String> {
    ast.items
        .iter()
        .filter_map(|item| {
            if let tyra_ast::Item::FnDef(f) = item
                && f.name.starts_with("bench_")
                && f.params.is_empty()
                && f.self_param.is_none()
            {
                return Some(f.name.clone());
            }
            None
        })
        .collect()
}

/// Synthesize a `fn main()` that times each `bench_*` function with
/// `__bench_clock_ns()` and prints `BENCH <name> <ns> ns` lines.
fn synthesize_bench_runner(bench_source: &str, bench_fns: &[String]) -> String {
    let mut out = String::from(bench_source);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("\nfn main() -> Unit\n");
    for name in bench_fns {
        out.push_str(&format!("  let __t0_{name} = __bench_clock_ns()\n"));
        out.push_str(&format!("  {name}()\n"));
        out.push_str(&format!("  let __t1_{name} = __bench_clock_ns()\n"));
        out.push_str(&format!(
            "  println(\"BENCH {name} #{{__t1_{name} - __t0_{name}}} ns\")\n"
        ));
    }
    out.push_str("end\n");
    out
}

/// Compile and run a single `*_bench.tyra` file; print results and return
/// true on success.  Mirrors run_test_file_inner: synthesizes a runner, writes
/// it to a temp file alongside the bench file, compiles + runs, then cleans up.
fn run_bench_file(bench_file: &Path) -> bool {
    let source = match std::fs::read_to_string(bench_file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", bench_file.display());
            return false;
        }
    };

    let dir = bench_file.parent().unwrap_or(Path::new("."));
    let workspace_dir = if dir.as_os_str().is_empty() {
        Some(Path::new("."))
    } else {
        Some(dir)
    };

    let check = tyra_driver::check_in_memory(
        bench_file.to_string_lossy().into_owned(),
        source.clone(),
        workspace_dir,
    );
    if check.report.has_errors() {
        eprint!("{}", check.report.render(&check.sources));
        return false;
    }

    let bench_fns = find_bench_fns(&check.ast);
    if bench_fns.is_empty() {
        eprintln!("warning: no bench_* functions in {}", bench_file.display());
        return true;
    }

    let runner_source = synthesize_bench_runner(&source, &bench_fns);

    // Write synthesized runner alongside the bench file so import resolution works.
    let runner_name = format!("__tyra_bench_runner_{}.tyra", std::process::id());
    let runner_path = dir.join(&runner_name);
    if let Err(e) = std::fs::write(&runner_path, &runner_source) {
        eprintln!("error: cannot write bench runner: {e}");
        return false;
    }

    let result = tyra_driver::run_and_capture(&runner_path);
    let _ = std::fs::remove_file(&runner_path);

    if result.report.has_errors() {
        eprint!("{}", result.report.render(&result.sources));
        return false;
    }

    let stdout = result.stdout.unwrap_or_default();
    let file_stem = bench_file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("?");
    println!("running {} bench(es) in {file_stem}", bench_fns.len());
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("BENCH ") {
            // "BENCH <name> <ns> ns"
            let parts: Vec<&str> = rest.splitn(3, ' ').collect();
            if parts.len() >= 2 {
                let name = parts[0];
                let ns_str = parts[1];
                if let Ok(ns) = ns_str.parse::<u64>() {
                    let ms = ns as f64 / 1_000_000.0;
                    println!("  {name:<40} {ns:>12} ns  ({ms:.3} ms)");
                } else {
                    println!("  {line}");
                }
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Write a minimal `.tyra` file with no `test_*` functions.
    /// `run_test_file_core` returns immediately after `check_in_memory` without
    /// invoking LLVM, so these helpers are safe to use in fast unit tests.
    fn write_no_test_file(dir: &std::path::Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, "fn helper() -> Int\n  42\nend\n").unwrap();
        path
    }

    // --- run_test_files_parallel: ordering invariant ---

    #[test]
    fn parallel_results_maintain_input_order() {
        // 4 files that finish instantly (no test_* fns → no LLVM needed).
        // jobs=4 runs all in one chunk; threads may complete in any order.
        // Results must come back in the same order as the input slice.
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<PathBuf> = ["a_test.tyra", "b_test.tyra", "c_test.tyra", "d_test.tyra"]
            .iter()
            .map(|n| write_no_test_file(dir.path(), n))
            .collect();

        let results = run_test_files_parallel(&files, None, None, 4);

        assert_eq!(results.len(), files.len(), "result count must equal input count");
        for (i, (res, file)) in results.iter().zip(files.iter()).enumerate() {
            let got = std::path::Path::new(&res.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            let want = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
            assert_eq!(got, want, "result[{i}].path must match input[{i}]");
        }
    }

    #[test]
    fn parallel_jobs_1_and_jobs_n_produce_same_order() {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<PathBuf> = ["x_test.tyra", "y_test.tyra", "z_test.tyra"]
            .iter()
            .map(|n| write_no_test_file(dir.path(), n))
            .collect();

        let seq = run_test_files_parallel(&files, None, None, 1);
        let par = run_test_files_parallel(&files, None, None, 3);

        assert_eq!(seq.len(), par.len());
        for (s, p) in seq.iter().zip(par.iter()) {
            assert_eq!(s.path, p.path, "path order must be identical for jobs=1 vs jobs=N");
        }
    }

    #[test]
    fn parallel_empty_input_returns_empty() {
        let results = run_test_files_parallel(&[], None, None, 4);
        assert!(results.is_empty());
    }

    #[test]
    fn parallel_filter_no_match_yields_zero_tests() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![write_no_test_file(dir.path(), "q_test.tyra")];

        // "nomatch" won't match anything; file also has no test_ fns anyway.
        let results = run_test_files_parallel(&files, Some("nomatch"), None, 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].pass, 0);
        assert_eq!(results[0].fail, 0);
    }

    #[test]
    fn parallel_single_chunk_larger_than_jobs() {
        // 5 files, jobs=2 → ceil(5/2)=3 chunks: [2,2,1].
        // Output must still be in original order.
        let dir = tempfile::tempdir().unwrap();
        let names = ["f1_test.tyra", "f2_test.tyra", "f3_test.tyra", "f4_test.tyra", "f5_test.tyra"];
        let files: Vec<PathBuf> = names.iter().map(|n| write_no_test_file(dir.path(), n)).collect();

        let results = run_test_files_parallel(&files, None, None, 2);

        assert_eq!(results.len(), 5);
        for (i, (res, file)) in results.iter().zip(files.iter()).enumerate() {
            let got = std::path::Path::new(&res.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            let want = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
            assert_eq!(got, want, "result[{i}] must match input[{i}]");
        }
    }

    // --- Timeout tests (require a pre-built tyra binary) ---
    //
    // Run with: cargo build && cargo test -p tyra-cli -- --ignored
    //
    // These tests invoke the external `tyra` binary via Command so that the
    // runtime staticlib lookup (current_exe().parent()) resolves correctly.
    // Calling run_test_file_core() from a test binary would point at
    // target/debug/deps/ instead of target/debug/ and always fail with E0502.

    /// Locate the built `tyra` binary for end-to-end invocation.
    /// The test binary lives at target/{debug,release}/deps/; the tyra binary
    /// is one level up at target/{debug,release}/.
    fn find_tyra_binary() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let profile_dir = exe.parent()?.parent()?;
        let tyra = profile_dir.join("tyra");
        if tyra.exists() { Some(tyra) } else { None }
    }

    #[test]
    #[ignore = "requires pre-built tyra binary — run with: cargo build && cargo test -p tyra-cli -- --ignored"]
    fn timeout_kills_hanging_test_and_reports_failure() {
        let Some(tyra) = find_tyra_binary() else {
            eprintln!("SKIP: tyra binary not found at expected path — run `cargo build` first");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hang_test.tyra");
        fs::write(
            &path,
            "import assert\n\
             fn test_infinite() -> Result<Unit, String>\n\
             \x20 while true\n\
             \x20 end\n\
             \x20 Ok(())\n\
             end\n",
        )
        .unwrap();

        let output = std::process::Command::new(&tyra)
            .args(["test", "--timeout", "1", path.to_str().unwrap()])
            .output()
            .expect("failed to invoke tyra binary");

        assert_ne!(
            output.status.code(),
            Some(0),
            "timed-out test must exit non-zero"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stdout.contains("timeout") || stderr.contains("timeout"),
            "output must mention timeout:\nstdout={stdout:?}\nstderr={stderr:?}"
        );
    }

    #[test]
    #[ignore = "requires pre-built tyra binary — run with: cargo build && cargo test -p tyra-cli -- --ignored"]
    fn timeout_does_not_affect_fast_passing_test() {
        let Some(tyra) = find_tyra_binary() else {
            eprintln!("SKIP: tyra binary not found at expected path — run `cargo build` first");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fast_test.tyra");
        fs::write(
            &path,
            "import assert\n\
             fn test_fast() -> Result<Unit, String>\n\
             \x20 assert.eq(1, 1)?\n\
             \x20 Ok(())\n\
             end\n",
        )
        .unwrap();

        // 10-second budget is ample for a trivial test.
        let output = std::process::Command::new(&tyra)
            .args(["test", "--timeout", "10", path.to_str().unwrap()])
            .output()
            .expect("failed to invoke tyra binary");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "fast test must pass:\nstdout={stdout:?}\nstderr={stderr:?}"
        );
        assert!(
            stdout.contains("1 passed") || stdout.contains("ok"),
            "expected passing output:\nstdout={stdout:?}"
        );
    }
}
