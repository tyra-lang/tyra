//! Inkwell I4b (slice B): string builtins that pass a `List<String>` by
//! reference.
//!
//! Three builtins exchange a `List<String>` (`{ ptr, i64 }`, §11) struct with
//! the runtime through a stack slot, mirroring the legacy text backend EXACTLY:
//!
//! - `__string_split_whitespace(s) -> List<String>` and
//!   `__string_split(s, sep) -> List<String>` — the runtime *fills* an
//!   out-parameter: alloca a `List__String` slot, pass its pointer last, then
//!   load the populated struct back.
//! - `__string_join(parts, sep) -> String` — the reverse: store the incoming
//!   `List<String>` value into a slot and pass that pointer *first*, so the
//!   runtime reads the `{ ptr, i64 }` layout by reference.
//!
//! `__string_replace` returns a plain `String` (no list), so it rides the
//! table-driven `SIMPLE` path in `inkwell_builtins.rs` rather than this module.
//!
//! No block split: every call is straight-line, so phi bookkeeping is untouched.

use inkwell::types::StructType;
use inkwell::values::BasicMetadataValueEnum;

use tyra_mir::Operand;
use tyra_types::Ty;

use crate::inkwell_codegen::CodeGen;

const STRING_LIST: &[&str] = &["__string_split_whitespace", "__string_split", "__string_join"];

impl<'ctx> CodeGen<'ctx> {
    /// Is `name` a string builtin that round-trips a `List<String>` by ref?
    pub(crate) fn is_string_list_builtin(name: &str) -> bool {
        STRING_LIST.contains(&name)
    }

    /// Emit a `List<String>`-by-reference string builtin. Returns `false` if
    /// `fname` is not in this slice (caller falls through to the next dispatch).
    pub(crate) fn emit_string_list_builtin(
        &mut self,
        dest: &Option<String>,
        fname: &str,
        args: &[Operand],
    ) -> bool {
        let d = dest.as_deref();
        match fname {
            "__string_split_whitespace" => {
                self.emit_string_split(d, args, "tyra_string_split_whitespace", false)
            }
            "__string_split" => self.emit_string_split(d, args, "tyra_string_split", true),
            "__string_join" => self.emit_string_join(d, args),
            _ => return false,
        }
        true
    }

    /// `List<String>` struct type (`{ ptr, i64 }`), registered by monomorphization.
    fn list_string_ty(&self) -> StructType<'ctx> {
        let mono = Ty::Generic("List".into(), vec![Ty::String]).monomorphized_name();
        *self
            .struct_types
            .get(&mono)
            .unwrap_or_else(|| panic!("`{mono}` struct must be registered for string split/join"))
    }

    /// `__string_split[_whitespace]` — out-parameter form. The runtime writes the
    /// `List<String>` into a stack slot whose pointer is passed last; we load it
    /// back as the result.
    fn emit_string_split(&mut self, dest: Option<&str>, args: &[Operand], callee: &str, has_sep: bool) {
        let d = dest.unwrap_or("_split");
        let list_ty = self.list_string_ty();
        let slot = self.builder.build_alloca(list_ty, &format!("{d}.slot")).unwrap();

        let mut call_args: Vec<BasicMetadataValueEnum<'ctx>> = vec![self.operand(&args[0]).into()];
        if has_sep {
            call_args.push(self.operand(&args[1]).into());
        }
        call_args.push(slot.into());

        let f = self
            .module
            .get_function(callee)
            .unwrap_or_else(|| panic!("runtime extern `{callee}` must be declared (I1)"));
        self.builder.build_call(f, &call_args, "").unwrap();

        let result = self.builder.build_load(list_ty, slot, d).unwrap();
        self.values.insert(d.to_string(), result);
    }

    /// `__string_join(parts, sep) -> String` — store the `List<String>` value
    /// into a slot and pass its pointer first (the runtime reads it by ref).
    fn emit_string_join(&mut self, dest: Option<&str>, args: &[Operand]) {
        let d = dest.unwrap_or("_join");
        let list_ty = self.list_string_ty();
        let list_val = self.operand(&args[0]);
        let slot = self.builder.build_alloca(list_ty, &format!("{d}.lslot")).unwrap();
        self.builder.build_store(slot, list_val).unwrap();
        let sep = self.operand(&args[1]);

        let f = self
            .module
            .get_function("tyra_string_join")
            .unwrap_or_else(|| panic!("runtime extern `tyra_string_join` must be declared (I1)"));
        let cs = self.builder.build_call(f, &[slot.into(), sep.into()], d).unwrap();
        let rv = cs.try_as_basic_value().basic().expect("tyra_string_join returns a ptr");
        self.values.insert(d.to_string(), rv);
    }
}
