//! Inkwell I4b (slice D): http *server* builtins (handle ptr↔int round-trip).
//!
//! The server handle is an opaque runtime pointer that Tyra stores as an `Int`
//! (`AppServer._handle`), so each builtin bridges the two representations,
//! mirroring the legacy text backend EXACTLY:
//!
//! - `__http_server_new() -> Int` — call returns a `ptr`; `ptrtoint` it to i64.
//! - `__http_server_listen(srv, port) -> Int` — `inttoptr` the handle, call
//!   (returns i32), `sext` to i64.
//! - `__http_server_route(srv, method, path, handler)` — `inttoptr` the handle;
//!   the handler is already a `ptr` value (a `ClosureBuild` fat-pointer object,
//!   or a function-typed param) so it passes straight through. Void result.
//!
//! The `ptrtoint`/`inttoptr` round-trip strips pointer provenance but is safe
//! because the handle is never dereferenced in Tyra IR — only handed back to
//! opaque extern calls (legacy `emit_http_server_new` TODO, ADR follow-up).
//!
//! http *client* builtins are mechanical (ptr/i64 only) and already ride the
//! table-driven `SIMPLE` path in `inkwell_builtins.rs` (I4a).
//!
//! No block split: every call is straight-line.

use inkwell::values::BasicMetadataValueEnum;

use tyra_mir::Operand;

use crate::inkwell_codegen::CodeGen;

const HTTP_SERVER: &[&str] =
    &["__http_server_new", "__http_server_route", "__http_server_listen"];

impl<'ctx> CodeGen<'ctx> {
    /// Is `name` an http *server* builtin (handle ptr↔int round-trip)?
    pub(crate) fn is_http_server_builtin(name: &str) -> bool {
        HTTP_SERVER.contains(&name)
    }

    /// Emit an http server builtin. Returns `false` if `fname` is not in this
    /// slice (caller falls through to the next dispatch).
    pub(crate) fn emit_http_server_builtin(
        &mut self,
        dest: &Option<String>,
        fname: &str,
        args: &[Operand],
    ) -> bool {
        let d = dest.as_deref();
        match fname {
            "__http_server_new" => self.emit_http_server_new(d),
            "__http_server_listen" => self.emit_http_server_listen(d, args),
            "__http_server_route" => self.emit_http_server_route(args),
            _ => return false,
        }
        true
    }

    /// `__http_server_new()` — runtime returns a `ptr`; store it as an `Int`
    /// handle via `ptrtoint`.
    fn emit_http_server_new(&mut self, dest: Option<&str>) {
        let d = dest.unwrap_or("_srv_new");
        let f = self.module.get_function("tyra_http_server_new").unwrap();
        let ptr = self
            .builder
            .build_call(f, &[], &format!("{d}.ptr"))
            .unwrap()
            .try_as_basic_value()
            .basic()
            .expect("tyra_http_server_new returns a ptr")
            .into_pointer_value();
        let handle = self.builder.build_ptr_to_int(ptr, self.ctx.i64_type(), d).unwrap();
        self.values.insert(d.to_string(), handle.into());
    }

    /// `__http_server_listen(srv, port)` — `inttoptr` the handle, call (i32
    /// result), `sext` to the Tyra `Int`.
    fn emit_http_server_listen(&mut self, dest: Option<&str>, args: &[Operand]) {
        let d = dest.unwrap_or("_srv_listen");
        let srv = self.operand(&args[0]).into_int_value();
        let sptr = self.builder.build_int_to_ptr(srv, self.ptr(), &format!("{d}.sptr")).unwrap();
        let port = self.operand(&args[1]);
        let f = self.module.get_function("tyra_http_server_listen").unwrap();
        let rv = self
            .builder
            .build_call(f, &[sptr.into(), port.into()], &format!("{d}.i32"))
            .unwrap()
            .try_as_basic_value()
            .basic()
            .expect("tyra_http_server_listen returns i32")
            .into_int_value();
        let v = self.builder.build_int_s_extend(rv, self.ctx.i64_type(), d).unwrap();
        self.values.insert(d.to_string(), v.into());
    }

    /// `__http_server_route(srv, method, path, handler)` — `inttoptr` the handle
    /// and pass the four pointers. Void result.
    fn emit_http_server_route(&mut self, args: &[Operand]) {
        let srv = self.operand(&args[0]).into_int_value();
        let sptr = self.builder.build_int_to_ptr(srv, self.ptr(), "srv.sptr").unwrap();
        let call_args: Vec<BasicMetadataValueEnum<'ctx>> = vec![
            sptr.into(),
            self.operand(&args[1]).into(),
            self.operand(&args[2]).into(),
            self.operand(&args[3]).into(),
        ];
        let f = self.module.get_function("tyra_http_server_route").unwrap();
        self.builder.build_call(f, &call_args, "").unwrap();
    }
}
