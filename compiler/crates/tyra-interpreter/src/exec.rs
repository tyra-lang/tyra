// MIR execution engine.
//
// Runs a Program by walking each Function's flat body: Vec<MirStmt>.
// Control flow uses a label→index map and a program counter (pc).
// Mutable locals use the same HashMap as SSA temps (Alloca/Store/Load).
// Closures carry their fn_name + captured env values.
// For-each loops (MapForEachCall etc.) invoke the lifted lambda directly,
// passing key/value wrapped in BoxedPtr so PtrLoad can unbox them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use tyra_mir::{Constant, Function, Instruction, Operand, Program};

use crate::builtins::{call_builtin, call_collection_builtin, BuiltinResult};
use crate::printf_fmt::printf_format;
use crate::value::Value;
use crate::RunOutcome;

/// Maximum number of MIR instructions executed before aborting (browser freeze guard).
const EXEC_LIMIT: u64 = 100_000_000;

/// Interpreter state shared across all function calls.
struct Interpreter<'p> {
    program: &'p Program,
    stdout: String,
    stderr: String,
    instruction_count: u64,
}

/// Signal propagated upward through call frames.
enum Signal {
    /// Normal return value.
    Return(Value),
    /// sys.exit(code) — terminate immediately.
    Exit(i32),
    /// panic(msg) — terminate with exit 101, print panic message to stderr.
    Panic(String),
}

impl<'p> Interpreter<'p> {
    fn new(program: &'p Program) -> Self {
        Self {
            program,
            stdout: String::new(),
            stderr: String::new(),
            instruction_count: 0,
        }
    }

    fn resolve_operand(&self, op: &Operand, locals: &HashMap<String, Value>) -> Value {
        match op {
            Operand::Var(name) => locals
                .get(name)
                .unwrap_or_else(|| panic!("interpreter: undefined variable '{}'", name))
                .clone(),
            Operand::Const(c) => match c {
                Constant::Int(n) => Value::Int(*n),
                Constant::Float(f) => Value::Float(*f),
                Constant::Bool(b) => Value::Bool(*b),
                Constant::StringRef(i) => {
                    let s = self.program.string_constants.get(*i)
                        .unwrap_or_else(|| panic!("interpreter: string_constant[{}] out of bounds", i));
                    Value::Str(s.as_str().into())
                }
                Constant::Unit => Value::Unit,
            },
        }
    }

    /// Build the label→instruction-index map for a function body.
    fn build_label_map(body: &[tyra_mir::MirStmt]) -> HashMap<String, usize> {
        let mut map = HashMap::new();
        for (i, stmt) in body.iter().enumerate() {
            if let Instruction::Label(name) = &stmt.instr {
                map.insert(name.clone(), i);
            }
        }
        map
    }

    /// Execute a function, returning a Signal.
    fn run_frame(&mut self, func: &Function, args: Vec<Value>) -> Signal {
        let mut locals: HashMap<String, Value> = HashMap::new();
        let label_map = Self::build_label_map(&func.body);

        // Bind parameters.
        for ((name, _ty), val) in func.params.iter().zip(args.into_iter()) {
            locals.insert(name.clone(), val);
        }

        let mut pc: usize = 0;
        let mut prev_label: Option<String> = None;

        loop {
            if pc >= func.body.len() {
                return Signal::Return(Value::Unit);
            }

            self.instruction_count += 1;
            if self.instruction_count > EXEC_LIMIT {
                panic!("interpreter: execution limit exceeded ({}M instructions)", EXEC_LIMIT / 1_000_000);
            }

            let stmt = &func.body[pc];
            let instr = &stmt.instr;

            match instr {
                Instruction::Label(name) => {
                    prev_label = Some(name.clone());
                    pc += 1;
                }

                Instruction::Const { dest, value } => {
                    let v = self.resolve_operand(&Operand::Const(value.clone()), &locals);
                    locals.insert(dest.clone(), v);
                    pc += 1;
                }

                Instruction::Copy { dest, source } => {
                    let v = locals.get(source)
                        .unwrap_or_else(|| panic!("interpreter: Copy: undefined '{}'", source))
                        .clone();
                    locals.insert(dest.clone(), v);
                    pc += 1;
                }

                Instruction::BinOp { dest, op, lhs, rhs } => {
                    let l = self.resolve_operand(lhs, &locals);
                    let r = self.resolve_operand(rhs, &locals);
                    let result = eval_binop(*op, &l, &r);
                    locals.insert(dest.clone(), result);
                    pc += 1;
                }

                Instruction::Neg { dest, operand } => {
                    let v = self.resolve_operand(operand, &locals);
                    let result = match v {
                        Value::Int(n) => Value::Int(-n),
                        Value::Float(f) => Value::Float(-f),
                        _ => panic!("interpreter: Neg on non-numeric"),
                    };
                    locals.insert(dest.clone(), result);
                    pc += 1;
                }

                Instruction::Not { dest, operand } => {
                    let v = self.resolve_operand(operand, &locals);
                    let result = match v {
                        Value::Bool(b) => Value::Bool(!b),
                        _ => panic!("interpreter: Not on non-bool"),
                    };
                    locals.insert(dest.clone(), result);
                    pc += 1;
                }

                Instruction::Return { value } => {
                    let v = match value {
                        Some(op) => self.resolve_operand(op, &locals),
                        None => Value::Unit,
                    };
                    return Signal::Return(v);
                }

                Instruction::Jump { label } => {
                    let target = *label_map.get(label)
                        .unwrap_or_else(|| panic!("interpreter: Jump: unknown label '{}'", label));
                    pc = target;
                }

                Instruction::BranchIf { cond, true_label, false_label } => {
                    let c = self.resolve_operand(cond, &locals);
                    let label = if c.is_truthy() { true_label } else { false_label };
                    let target = *label_map.get(label)
                        .unwrap_or_else(|| panic!("interpreter: BranchIf: unknown label '{}'", label));
                    pc = target;
                }

                Instruction::Phi { dest, branches } => {
                    let pl = prev_label.as_deref();
                    let val = branches.iter().find(|(_, lbl)| Some(lbl.as_str()) == pl)
                        .map(|(op, _)| self.resolve_operand(op, &locals))
                        .unwrap_or_else(|| {
                            branches.first()
                                .map(|(op, _)| self.resolve_operand(op, &locals))
                                .unwrap_or(Value::Unit)
                        });
                    locals.insert(dest.clone(), val);
                    pc += 1;
                }

                Instruction::Alloca { dest } => {
                    locals.insert(dest.clone(), Value::Unit);
                    pc += 1;
                }

                Instruction::Store { dest, value } => {
                    let v = self.resolve_operand(value, &locals);
                    locals.insert(dest.clone(), v);
                    pc += 1;
                }

                Instruction::Load { dest, source } => {
                    let v = locals.get(source)
                        .unwrap_or_else(|| panic!("interpreter: Load: undefined slot '{}'", source))
                        .clone();
                    locals.insert(dest.clone(), v);
                    pc += 1;
                }

                Instruction::PtrLoad { dest, ptr, ty: _ } => {
                    let v = locals.get(ptr)
                        .unwrap_or_else(|| panic!("interpreter: PtrLoad: undefined ptr '{}'", ptr))
                        .clone();
                    let unboxed = match v {
                        Value::BoxedPtr(inner) => *inner,
                        other => other,
                    };
                    locals.insert(dest.clone(), unboxed);
                    pc += 1;
                }

                Instruction::StructInit { dest, type_name, fields } => {
                    let vals: Vec<Value> = fields.iter()
                        .map(|op| self.resolve_operand(op, &locals))
                        .collect();
                    let is_data = self.program.struct_defs.iter()
                        .find(|sd| sd.name == *type_name)
                        .map(|sd| sd.is_data)
                        .unwrap_or(false);
                    let v = if is_data {
                        Value::DataRef(Rc::new(RefCell::new(vals)))
                    } else {
                        Value::Struct { type_name: type_name.as_str().into(), fields: vals }
                    };
                    locals.insert(dest.clone(), v);
                    pc += 1;
                }

                Instruction::FieldGet { dest, obj, type_name: _, field_index } => {
                    let obj_val = self.resolve_operand(obj, &locals);
                    let v = match &obj_val {
                        Value::Struct { fields, .. } => fields.get(*field_index as usize)
                            .unwrap_or_else(|| panic!("interpreter: FieldGet[{}] on struct with {} fields", field_index, fields.len()))
                            .clone(),
                        Value::DataRef(rc) => rc.borrow().get(*field_index as usize)
                            .unwrap_or_else(|| panic!("interpreter: FieldGet[{}] on DataRef", field_index))
                            .clone(),
                        _ => panic!("interpreter: FieldGet on non-struct: {:?}", obj_val),
                    };
                    locals.insert(dest.clone(), v);
                    pc += 1;
                }

                Instruction::FieldSet { obj, type_name: _, field_index, value } => {
                    let new_val = self.resolve_operand(value, &locals);
                    let obj_val = self.resolve_operand(obj, &locals);
                    match &obj_val {
                        Value::DataRef(rc) => {
                            let mut fields = rc.borrow_mut();
                            if let Some(slot) = fields.get_mut(*field_index as usize) {
                                *slot = new_val;
                            } else {
                                panic!("interpreter: FieldSet[{}] out of range", field_index);
                            }
                        }
                        _ => panic!("interpreter: FieldSet on non-DataRef: {:?}", obj_val),
                    }
                    pc += 1;
                }

                Instruction::AdtInit { dest, type_name, tag, fields } => {
                    let mut all_fields = vec![Value::Int(*tag)];
                    for op in fields {
                        all_fields.push(self.resolve_operand(op, &locals));
                    }
                    locals.insert(dest.clone(), Value::Struct {
                        type_name: type_name.as_str().into(),
                        fields: all_fields,
                    });
                    pc += 1;
                }

                Instruction::AdtTag { dest, obj, type_name: _ } => {
                    let obj_val = self.resolve_operand(obj, &locals);
                    let tag = match &obj_val {
                        Value::Struct { fields, .. } => fields.first().map(|v| v.as_int()).unwrap_or(0),
                        _ => panic!("interpreter: AdtTag on non-struct: {:?}", obj_val),
                    };
                    locals.insert(dest.clone(), Value::Int(tag));
                    pc += 1;
                }

                Instruction::AdtPayload { dest, obj, type_name: _, field_index } => {
                    let obj_val = self.resolve_operand(obj, &locals);
                    let v = match &obj_val {
                        // Out-of-bounds is valid for None/tagless variants: emit Unit
                        // so __display_option__ receives (tag=1, Unit) and returns "None".
                        Value::Struct { fields, .. } => fields.get(*field_index as usize)
                            .cloned()
                            .unwrap_or(Value::Unit),
                        _ => panic!("interpreter: AdtPayload on non-struct: {:?}", obj_val),
                    };
                    locals.insert(dest.clone(), v);
                    pc += 1;
                }

                Instruction::StringFormat { dest, format_ref, args } => {
                    let fmt = self.program.string_constants.get(*format_ref)
                        .unwrap_or_else(|| panic!("interpreter: StringFormat: string_constant[{}] out of bounds", format_ref));
                    let arg_vals: Vec<Value> = args.iter()
                        .map(|op| self.resolve_operand(op, &locals))
                        .collect();
                    let result = printf_format(fmt, &arg_vals);
                    locals.insert(dest.clone(), Value::Str(result.into()));
                    pc += 1;
                }

                Instruction::ListInit { dest, elem_type: _, elements } => {
                    let elems: Vec<Value> = elements.iter()
                        .map(|op| self.resolve_operand(op, &locals))
                        .collect();
                    locals.insert(dest.clone(), Value::List(Rc::new(RefCell::new(elems))));
                    pc += 1;
                }

                Instruction::ListLen { dest, list } => {
                    let lv = self.resolve_operand(list, &locals);
                    let n = match &lv {
                        Value::List(rc) => rc.borrow().len() as i64,
                        _ => panic!("interpreter: ListLen on non-list: {:?}", lv),
                    };
                    locals.insert(dest.clone(), Value::Int(n));
                    pc += 1;
                }

                Instruction::ListGet { dest, list, index, elem_type: _ } => {
                    let lv = self.resolve_operand(list, &locals);
                    let idx = self.resolve_operand(index, &locals).as_int() as usize;
                    let v = match &lv {
                        Value::List(rc) => {
                            let borrowed = rc.borrow();
                            borrowed.get(idx)
                                .unwrap_or_else(|| panic!(
                                    "interpreter: ListGet: index {} out of bounds (len {})",
                                    idx, borrowed.len()
                                ))
                                .clone()
                        }
                        _ => panic!("interpreter: ListGet on non-list: {:?}", lv),
                    };
                    locals.insert(dest.clone(), v);
                    pc += 1;
                }

                Instruction::ListGetSafe { dest, list, index, elem_type: _ } => {
                    let lv = self.resolve_operand(list, &locals);
                    let idx = self.resolve_operand(index, &locals).as_int();
                    let opt = if idx < 0 {
                        Value::Struct {
                            type_name: "Option".into(),
                            fields: vec![Value::Int(1)],
                        }
                    } else {
                        match &lv {
                            Value::List(rc) => {
                                let borrowed = rc.borrow();
                                match borrowed.get(idx as usize) {
                                    Some(elem) => Value::Struct {
                                        type_name: "Option".into(),
                                        fields: vec![Value::Int(0), elem.clone()],
                                    },
                                    None => Value::Struct {
                                        type_name: "Option".into(),
                                        fields: vec![Value::Int(1)],
                                    },
                                }
                            }
                            _ => panic!("interpreter: ListGetSafe on non-list: {:?}", lv),
                        }
                    };
                    locals.insert(dest.clone(), opt);
                    pc += 1;
                }

                Instruction::ListPush { dest, list, elem, elem_type: _ } => {
                    let lv = self.resolve_operand(list, &locals);
                    let ev = self.resolve_operand(elem, &locals);
                    let new_list = match &lv {
                        Value::List(rc) => {
                            let mut new_vec = rc.borrow().clone();
                            new_vec.push(ev);
                            Value::List(Rc::new(RefCell::new(new_vec)))
                        }
                        _ => panic!("interpreter: ListPush on non-list: {:?}", lv),
                    };
                    locals.insert(dest.clone(), new_list);
                    pc += 1;
                }

                Instruction::MapGetOption { dest, handle, key, .. }
                | Instruction::LinkedMapGetOption { dest, handle, key, .. }
                | Instruction::SortedMapGetOption { dest, handle, key, .. } => {
                    let hv = unwrap_coll(self.resolve_operand(handle, &locals));
                    let kv = self.resolve_operand(key, &locals);
                    let found = match &hv {
                        Value::Map(rc) => rc.borrow().iter()
                            .find(|(k, _)| crate::value::key_eq(k, &kv))
                            .map(|(_, v)| v.clone()),
                        Value::SortedMap(rc) => rc.borrow().iter()
                            .find(|(k, _)| crate::value::key_eq(k, &kv))
                            .map(|(_, v)| v.clone()),
                        _ => panic!("interpreter: MapGetOption on non-map: {:?}", hv),
                    };
                    let opt = match found {
                        Some(v) => Value::Struct {
                            type_name: "Option".into(),
                            fields: vec![Value::Int(0), v],
                        },
                        None => Value::Struct {
                            type_name: "Option".into(),
                            fields: vec![Value::Int(1)],
                        },
                    };
                    locals.insert(dest.clone(), opt);
                    pc += 1;
                }

                Instruction::ClosureBuild { dest, fn_name, env_fields, env_struct_name, .. } => {
                    let env: Vec<Value> = env_fields.iter()
                        .map(|op| self.resolve_operand(op, &locals))
                        .collect();
                    locals.insert(dest.clone(), Value::Closure {
                        fn_name: fn_name.as_str().into(),
                        env,
                        env_struct_name: env_struct_name.as_str().into(),
                    });
                    pc += 1;
                }

                Instruction::IndirectCall { dest, fat_ptr, args, .. } => {
                    let closure = self.resolve_operand(fat_ptr, &locals);
                    let arg_vals: Vec<Value> = args.iter()
                        .map(|op| self.resolve_operand(op, &locals))
                        .collect();
                    match self.call_closure(closure, arg_vals) {
                        Signal::Return(v) => {
                            if let Some(d) = dest {
                                locals.insert(d.clone(), v);
                            }
                        }
                        sig => return sig,
                    }
                    pc += 1;
                }

                Instruction::Call { dest, func, args } => {
                    let arg_vals: Vec<Value> = args.iter()
                        .map(|op| self.resolve_operand(op, &locals))
                        .collect();

                    // List higher-order builtins (take a closure arg).
                    if let Some(sig) = self.try_list_higher_builtin(func, arg_vals.clone()) {
                        match sig {
                            Signal::Return(v) => {
                                if let Some(d) = dest {
                                    locals.insert(d.clone(), v);
                                }
                            }
                            other => return other,
                        }
                        pc += 1;
                        continue;
                    }

                    // Collection builtins.
                    if let Some(br) = call_collection_builtin(func, arg_vals.clone()) {
                        match br {
                            BuiltinResult::Value(v) => {
                                if let Some(d) = dest {
                                    locals.insert(d.clone(), v);
                                }
                            }
                            BuiltinResult::Exit(code) => return Signal::Exit(code),
                            BuiltinResult::Panic(msg) => return Signal::Panic(msg),
                            BuiltinResult::NotABuiltin => unreachable!(),
                        }
                        pc += 1;
                        continue;
                    }

                    // Standard builtins.
                    match call_builtin(func, arg_vals.clone(), &mut self.stdout, &mut self.stderr) {
                        BuiltinResult::Value(v) => {
                            if let Some(d) = dest {
                                locals.insert(d.clone(), v);
                            }
                        }
                        BuiltinResult::Exit(code) => return Signal::Exit(code),
                        BuiltinResult::Panic(msg) => return Signal::Panic(msg),
                        BuiltinResult::NotABuiltin => {
                            match self.call_function(func, arg_vals) {
                                Signal::Return(v) => {
                                    if let Some(d) = dest {
                                        locals.insert(d.clone(), v);
                                    }
                                }
                                other => return other,
                            }
                        }
                    }
                    pc += 1;
                }

                // ── For-each loops ───────────────────────────────────────────
                Instruction::MapForEachCall { handle, fat_ptr }
                | Instruction::LinkedMapForEachCall { handle, fat_ptr }
                | Instruction::SortedMapForEachCall { handle, fat_ptr } => {
                    let hv = unwrap_coll(self.resolve_operand(handle, &locals));
                    let closure = self.resolve_operand(fat_ptr, &locals);
                    let pairs: Vec<(Value, Value)> = match &hv {
                        Value::Map(rc) => rc.borrow().clone(),
                        Value::SortedMap(rc) => rc.borrow().clone(),
                        _ => panic!("interpreter: MapForEachCall on non-map: {:?}", hv),
                    };
                    for (k, v) in pairs {
                        match self.call_closure_for_each_kv(closure.clone(), k, v) {
                            Signal::Return(_) => {}
                            other => return other,
                        }
                    }
                    pc += 1;
                }

                Instruction::SetForEachCall { handle, fat_ptr }
                | Instruction::LinkedSetForEachCall { handle, fat_ptr }
                | Instruction::SortedSetForEachCall { handle, fat_ptr } => {
                    let hv = unwrap_coll(self.resolve_operand(handle, &locals));
                    let closure = self.resolve_operand(fat_ptr, &locals);
                    let elems: Vec<Value> = match &hv {
                        Value::Set(rc) => rc.borrow().clone(),
                        Value::SortedSet(rc) => rc.borrow().clone(),
                        _ => panic!("interpreter: SetForEachCall on non-set: {:?}", hv),
                    };
                    for elem in elems {
                        match self.call_closure_for_each_elem(closure.clone(), elem) {
                            Signal::Return(_) => {}
                            other => return other,
                        }
                    }
                    pc += 1;
                }

                // ── Async (unsupported) ──────────────────────────────────────
                Instruction::Spawn { .. } => panic!(
                    "interpreter: async 'spawn' is not supported (not available in Playground or --interpret mode)"
                ),
                Instruction::Await { .. } => panic!(
                    "interpreter: async 'await' is not supported (not available in Playground or --interpret mode)"
                ),
                Instruction::JoinAll { .. } => panic!(
                    "interpreter: async 'join_all' is not supported (not available in Playground or --interpret mode)"
                ),
                Instruction::Select { .. } => panic!(
                    "interpreter: async 'select' is not supported (not available in Playground or --interpret mode)"
                ),
            }
        }
    }

    fn call_function(&mut self, name: &str, args: Vec<Value>) -> Signal {
        let func = self.program.functions.iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("interpreter: function not found: '{}'", name))
            .clone();
        self.run_frame(&func, args)
    }

    fn call_closure(&mut self, closure: Value, args: Vec<Value>) -> Signal {
        match closure {
            Value::Closure { fn_name, env, env_struct_name } => {
                let env_val = make_env_value(env, &env_struct_name);
                let mut call_args = vec![env_val];
                call_args.extend(args);
                self.call_function(&fn_name, call_args)
            }
            _ => panic!("interpreter: IndirectCall on non-closure: {:?}", closure),
        }
    }

    fn call_closure_for_each_kv(&mut self, closure: Value, key: Value, val: Value) -> Signal {
        match closure {
            Value::Closure { fn_name, env, env_struct_name } => {
                let env_val = make_env_value(env, &env_struct_name);
                let args = vec![env_val, Value::BoxedPtr(Box::new(key)), Value::BoxedPtr(Box::new(val))];
                self.call_function(&fn_name, args)
            }
            _ => panic!("interpreter: MapForEachCall: not a closure: {:?}", closure),
        }
    }

    fn call_closure_for_each_elem(&mut self, closure: Value, elem: Value) -> Signal {
        match closure {
            Value::Closure { fn_name, env, env_struct_name } => {
                let env_val = make_env_value(env, &env_struct_name);
                let args = vec![env_val, Value::BoxedPtr(Box::new(elem))];
                self.call_function(&fn_name, args)
            }
            _ => panic!("interpreter: SetForEachCall: not a closure: {:?}", closure),
        }
    }

    fn try_list_higher_builtin(&mut self, fname: &str, args: Vec<Value>) -> Option<Signal> {
        match fname {
            "__list_map_int" | "__list_map_str" => {
                let list = list_vals(&args[0], fname);
                let closure = args[1].clone();
                let mut result = Vec::with_capacity(list.len());
                for elem in list {
                    match self.call_closure(closure.clone(), vec![elem]) {
                        Signal::Return(v) => result.push(v),
                        other => return Some(other),
                    }
                }
                Some(Signal::Return(Value::List(Rc::new(RefCell::new(result)))))
            }
            "__list_filter_int" | "__list_filter_str" => {
                let list = list_vals(&args[0], fname);
                let closure = args[1].clone();
                let mut result = Vec::new();
                for elem in list {
                    match self.call_closure(closure.clone(), vec![elem.clone()]) {
                        Signal::Return(pred) => {
                            if pred.as_bool() {
                                result.push(elem);
                            }
                        }
                        other => return Some(other),
                    }
                }
                Some(Signal::Return(Value::List(Rc::new(RefCell::new(result)))))
            }
            "__list_fold_int" | "__list_fold_str" => {
                let list = list_vals(&args[0], fname);
                let mut acc = args[1].clone();
                let closure = args[2].clone();
                for elem in list {
                    match self.call_closure(closure.clone(), vec![acc, elem]) {
                        Signal::Return(v) => acc = v,
                        other => return Some(other),
                    }
                }
                Some(Signal::Return(acc))
            }
            _ => None,
        }
    }
}

/// Unwrap a collection-type struct wrapper (e.g. `Struct { type_name: "Map__K__V", fields: [Map(rc)] }`)
/// to the raw collection value stored at field 0. Raw Map/Set/SortedMap/SortedSet pass through unchanged.
fn unwrap_coll(v: Value) -> Value {
    match v {
        Value::Struct { mut fields, .. } if !fields.is_empty() => fields.swap_remove(0),
        other => other,
    }
}

fn make_env_value(env: Vec<Value>, env_struct_name: &str) -> Value {
    if env_struct_name.is_empty() || env.is_empty() {
        Value::Unit
    } else {
        Value::Struct {
            type_name: env_struct_name.into(),
            fields: env,
        }
    }
}

fn list_vals(v: &Value, ctx: &str) -> Vec<Value> {
    match v {
        Value::List(rc) => rc.borrow().clone(),
        _ => panic!("interpreter: {}: expected List, got {:?}", ctx, v),
    }
}

fn eval_binop(op: tyra_mir::MirBinOp, l: &Value, r: &Value) -> Value {
    use tyra_mir::MirBinOp::*;
    match op {
        AddInt => Value::Int(l.as_int().wrapping_add(r.as_int())),
        SubInt => Value::Int(l.as_int().wrapping_sub(r.as_int())),
        MulInt => Value::Int(l.as_int().wrapping_mul(r.as_int())),
        DivInt => {
            let rhs = r.as_int();
            if rhs == 0 { panic!("interpreter: integer division by zero"); }
            Value::Int(l.as_int() / rhs)
        }
        RemInt => {
            let rhs = r.as_int();
            if rhs == 0 { panic!("interpreter: integer modulo by zero"); }
            Value::Int(l.as_int() % rhs)
        }
        AddFloat => Value::Float(l.as_float() + r.as_float()),
        SubFloat => Value::Float(l.as_float() - r.as_float()),
        MulFloat => Value::Float(l.as_float() * r.as_float()),
        DivFloat => Value::Float(l.as_float() / r.as_float()),
        EqInt => Value::Bool(l.as_int() == r.as_int()),
        NeqInt => Value::Bool(l.as_int() != r.as_int()),
        LtInt => Value::Bool(l.as_int() < r.as_int()),
        LeInt => Value::Bool(l.as_int() <= r.as_int()),
        GtInt => Value::Bool(l.as_int() > r.as_int()),
        GeInt => Value::Bool(l.as_int() >= r.as_int()),
        LtFloat => Value::Bool(l.as_float() < r.as_float()),
        LeFloat => Value::Bool(l.as_float() <= r.as_float()),
        GtFloat => Value::Bool(l.as_float() > r.as_float()),
        GeFloat => Value::Bool(l.as_float() >= r.as_float()),
        EqString => Value::Bool(l.as_str() == r.as_str()),
        NeqString => Value::Bool(l.as_str() != r.as_str()),
        And => Value::Bool(l.as_bool() && r.as_bool()),
        Or => Value::Bool(l.as_bool() || r.as_bool()),
    }
}

/// Entry point: interpret a Program and return stdout/stderr/exit_code.
pub fn interpret(program: &Program) -> RunOutcome {
    let main_fn = program.functions.iter()
        .find(|f| f.is_main)
        .unwrap_or_else(|| panic!("interpreter: no main function found"))
        .clone();

    let mut interp = Interpreter::new(program);

    match interp.run_frame(&main_fn, vec![]) {
        Signal::Return(_) => RunOutcome {
            stdout: interp.stdout,
            stderr: interp.stderr,
            exit_code: 0,
        },
        Signal::Exit(code) => RunOutcome {
            stdout: interp.stdout,
            stderr: interp.stderr,
            exit_code: code,
        },
        Signal::Panic(msg) => {
            interp.stderr.push_str(&msg);
            interp.stderr.push('\n');
            interp.stderr.push_str("__TYRA_PANIC__\n");
            RunOutcome {
                stdout: interp.stdout,
                stderr: interp.stderr,
                exit_code: 101,
            }
        }
    }
}
