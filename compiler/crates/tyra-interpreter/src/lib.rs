// MIR interpreter for the Tyra language.
//
// Interprets a `tyra_mir::Program` directly without LLVM/native code generation.
// Intended for the Playground (WASM) and `tyra run --interpret` (parity testing).
//
// Unsupported: async (Spawn/Await/JoinAll/Select) — panics with a clear message.
// io (stdin) is excluded at the check phase (E0200), so it never reaches here.

pub mod builtins;
pub mod exec;
pub mod printf_fmt;
pub mod value;

pub use exec::interpret;

use tyra_mir::Program;

/// Result of interpreting a program.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Interpret `program`, returning stdout/stderr/exit_code.
/// Panics (with a clear message) on unsupported MIR instructions (async, etc.).
pub fn run(program: &Program) -> RunOutcome {
    interpret(program)
}

#[cfg(test)]
mod tests {
    use tyra_frontend::source_to_mir;

    use super::*;

    fn run_source(src: &str) -> RunOutcome {
        let fr = source_to_mir("test.ty", src);
        assert!(!fr.check.report.has_errors(), "compile error: {:?}", fr.check.report);
        let program = fr.program.expect("MIR should be produced");
        run(&program)
    }

    #[test]
    fn test_hello_world() {
        let out = run_source("fn main()\n  println(\"hello\")\nend\n");
        assert_eq!(out.stdout, "hello\n");
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn test_arithmetic() {
        let out = run_source("fn main()\n  let x = 2 + 3 * 4\n  println(\"#{x}\")\nend\n");
        assert_eq!(out.stdout, "14\n");
    }

    #[test]
    fn test_if_else() {
        let src = "fn main()\n  let x = 10\n  if x > 5\n    println(\"big\")\n  else\n    println(\"small\")\n  end\nend\n";
        let out = run_source(src);
        assert_eq!(out.stdout, "big\n");
    }

    #[test]
    fn test_recursive_factorial() {
        let src = "fn fact(n: Int) -> Int\n  if n <= 1\n    return 1\n  end\n  return n * fact(n - 1)\nend\nfn main()\n  println(\"#{fact(5)}\")\nend\n";
        let out = run_source(src);
        assert_eq!(out.stdout, "120\n");
    }

    #[test]
    fn test_bool_logic() {
        let src = "fn main()\n  let a = 3 > 1\n  let b = 10 < 5\n  if a and not b\n    println(\"ok\")\n  end\nend\n";
        let out = run_source(src);
        assert_eq!(out.stdout, "ok\n");
    }

    #[test]
    fn test_exit_code() {
        let src = "import core.sys\nfn main()\n  sys.exit(42)\nend\n";
        let out = run_source(src);
        assert_eq!(out.exit_code, 42);
    }

    #[test]
    fn test_string_interp() {
        let src = "fn main()\n  let name = \"Tyra\"\n  println(\"Hello, #{name}!\")\nend\n";
        let out = run_source(src);
        assert_eq!(out.stdout, "Hello, Tyra!\n");
    }

    #[test]
    fn test_while_loop() {
        let src = "fn main()\n  mut i = 0\n  while i < 3\n    println(\"#{i}\")\n    i = i + 1\n  end\nend\n";
        let out = run_source(src);
        assert_eq!(out.stdout, "0\n1\n2\n");
    }
}
