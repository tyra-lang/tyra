//! Inkwell I4g: closures and indirect calls (ADR-0011 fat-pointer model).
//!
//! A closure value is a heap `__closure_fat { fn_ptr: ptr, env_ptr: ptr }`
//! (declared in I1 by `declare_closure_type`). `ClosureBuild` allocates the fat
//! pointer, stores the target function's global pointer at field 0, and either
//! allocates+populates an environment struct (capturing lambda) or stores null
//! (non-capturing) at field 1. `IndirectCall` loads both fields and dispatches
//! through `fn_ptr`, prepending `env_ptr` as the implicit first argument.
//!
//! Mirrors legacy `emit_closure_build` / `emit_indirect_call` (instr_emit.rs).
//! The value-handle model removes the text backend's `getelementptr null` +
//! `ptrtoint` size trick (it uses `StructType::size_of`) and its explicit
//! per-field store types (the operand handle carries its own type).

use inkwell::types::{BasicMetadataTypeEnum, BasicType};
use inkwell::values::BasicMetadataValueEnum;

use tyra_mir::Operand;
use tyra_types::Ty;

use crate::inkwell_codegen::CodeGen;

/// The fat-pointer struct key registered by `declare_closure_type` (I1).
const CLOSURE_FAT: &str = "__closure_fat";

impl<'ctx> CodeGen<'ctx> {
    /// `dest = closure(fn_name, env_fields...)` — build a fat-pointer closure.
    /// `env_struct_name` is the (registered) environment struct; empty / no
    /// fields means a non-capturing lambda (env_ptr = null).
    pub(crate) fn emit_closure_build(
        &mut self,
        dest: &str,
        fn_name: &str,
        env_fields: &[Operand],
        env_struct_name: &str,
    ) {
        let fat_ty = self.struct_types[CLOSURE_FAT];
        let gc = self.module.get_function("GC_malloc").unwrap();
        let size = fat_ty.size_of().expect("closure fat struct is sized");
        let fat = self
            .builder
            .build_call(gc, &[size.into()], dest)
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();

        // field 0 = target function's global pointer.
        let fnp = self.fn_values[fn_name].as_global_value().as_pointer_value();
        let f0 = self
            .builder
            .build_struct_gep(fat_ty, fat, 0, &format!("{dest}.fn_gep"))
            .unwrap();
        self.builder.build_store(f0, fnp).unwrap();

        // field 1 = env pointer: allocate+populate when capturing, else null.
        let env_ptr = if !env_struct_name.is_empty() && !env_fields.is_empty() {
            let env_ty = self.struct_types[env_struct_name];
            let esize = env_ty.size_of().expect("env struct is sized");
            let env = self
                .builder
                .build_call(gc, &[esize.into()], &format!("{dest}.env"))
                .unwrap()
                .try_as_basic_value()
                .basic()
                .unwrap()
                .into_pointer_value();
            for (i, fop) in env_fields.iter().enumerate() {
                let v = self.operand(fop);
                let gep = self
                    .builder
                    .build_struct_gep(env_ty, env, i as u32, &format!("{dest}.envf{i}"))
                    .unwrap();
                self.builder.build_store(gep, v).unwrap();
            }
            env
        } else {
            self.ptr().const_null()
        };
        let f1 = self
            .builder
            .build_struct_gep(fat_ty, fat, 1, &format!("{dest}.ep_gep"))
            .unwrap();
        self.builder.build_store(f1, env_ptr).unwrap();

        self.values.insert(dest.to_string(), fat.into());
    }

    /// `dest = fat_ptr(args...)` — dispatch through a fat-pointer closure.
    /// Loads `fn_ptr`/`env_ptr` from the fat struct and issues an indirect call
    /// to the signature `ret (ptr env, param_types...)`, with `env_ptr` as the
    /// implicit first argument.
    pub(crate) fn emit_indirect_call(
        &mut self,
        dest: &Option<String>,
        fat_ptr: &Operand,
        args: &[Operand],
        param_types: &[Ty],
        return_type: &Ty,
    ) {
        let fat = self.operand(fat_ptr).into_pointer_value();
        let fat_ty = self.struct_types[CLOSURE_FAT];
        let ptr = self.ptr();
        // For a void call there is no dest SSA value; reuse the legacy prefix.
        let pfx = dest.as_deref().unwrap_or("__ic");

        let fnp_gep = self
            .builder
            .build_struct_gep(fat_ty, fat, 0, &format!("{pfx}.fnp_gep"))
            .unwrap();
        let fnp = self
            .builder
            .build_load(ptr, fnp_gep, &format!("{pfx}.fnp"))
            .unwrap()
            .into_pointer_value();
        let envp_gep = self
            .builder
            .build_struct_gep(fat_ty, fat, 1, &format!("{pfx}.envp_gep"))
            .unwrap();
        let envp = self
            .builder
            .build_load(ptr, envp_gep, &format!("{pfx}.envp"))
            .unwrap()
            .into_pointer_value();

        // Signature: ret (ptr env, param_types...).
        let mut sig: Vec<BasicMetadataTypeEnum<'ctx>> = vec![ptr.into()];
        for pty in param_types {
            sig.push(self.ty_to_basic_type(pty).into());
        }
        let fn_ty = match return_type {
            Ty::Unit | Ty::Never => self.ctx.void_type().fn_type(&sig, false),
            ret => self.ty_to_basic_type(ret).fn_type(&sig, false),
        };

        // env_ptr is the implicit first argument.
        let mut call_args: Vec<BasicMetadataValueEnum<'ctx>> = vec![envp.into()];
        for a in args {
            call_args.push(self.operand(a).into());
        }
        let cs = self
            .builder
            .build_indirect_call(fn_ty, fnp, &call_args, pfx)
            .unwrap();
        if let Some(d) = dest
            && let Some(rv) = cs.try_as_basic_value().basic()
        {
            self.values.insert(d.clone(), rv);
        }
    }
}
