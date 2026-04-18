// Method lowering helpers: .copy() and field assignment.

use tyra_ast::*;
use tyra_types::Ty;

use crate::ir::*;

impl super::LowerCtx {
    /// Lower a field assignment: `obj.field = val`.
    /// For data types: load the ptr, then GEP+store in-place (§8.6 reference semantics).
    pub(super) fn lower_field_assign(
        &mut self,
        obj_name: &str,
        obj_expr: &Expr,
        field: &str,
        val: &str,
        body: &mut Vec<Instruction>,
    ) {
        if let Some((type_name, field_defs)) = self.resolve_struct_type(obj_expr) {
            // Field mutation is only allowed on data types (§8.6).
            // Value types are immutable — use copy() instead.
            if !self.data_types.contains(&type_name) {
                return;
            }
            if let Some(field_idx) = field_defs.iter().position(|(n, _)| n == field) {
                // Data type: obj_name alloca holds a ptr to the heap struct.
                // Load the ptr, then GEP+store directly — no struct rebuild needed.
                let ptr = self.fresh_temp();
                body.push(Instruction::Load {
                    dest: ptr.clone(),
                    source: obj_name.to_string(),
                });
                body.push(Instruction::FieldSet {
                    obj: Operand::Var(ptr),
                    type_name: type_name.clone(),
                    field_index: field_idx as u32,
                    value: Operand::Var(val.to_string()),
                });
            }
        }
    }

    /// Lower a .copy() call on a value type.
    /// Extracts all fields from the original, overrides specified fields, builds new struct.
    pub(super) fn lower_copy(
        &mut self,
        obj_val: &str,
        type_name: &str,
        field_defs: &[(String, Ty)],
        args: &[Arg],
        body: &mut Vec<Instruction>,
    ) -> String {
        // If no args, return the original (value types are immutable, copy is identity)
        if args.is_empty() {
            return obj_val.to_string();
        }

        // Build override map: field_name → lowered operand
        let mut overrides: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for arg in args {
            if let Some(label) = &arg.label {
                let val = self.lower_expr(&arg.value, body);
                overrides.insert(label.clone(), val);
            }
        }

        // For each field: use override if present, otherwise extract from original
        let mut field_operands = Vec::with_capacity(field_defs.len());
        for (i, (fname, _fty)) in field_defs.iter().enumerate() {
            if let Some(override_val) = overrides.get(fname) {
                field_operands.push(Operand::Var(override_val.clone()));
            } else {
                // Extract original field value
                let extracted = self.fresh_temp();
                body.push(Instruction::FieldGet {
                    dest: extracted.clone(),
                    obj: Operand::Var(obj_val.to_string()),
                    type_name: type_name.to_string(),
                    field_index: i as u32,
                });
                field_operands.push(Operand::Var(extracted));
            }
        }

        let dest = self.fresh_temp();
        body.push(Instruction::StructInit {
            dest: dest.clone(),
            type_name: type_name.to_string(),
            fields: field_operands,
        });
        dest
    }
}
