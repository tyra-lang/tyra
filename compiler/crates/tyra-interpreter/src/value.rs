// Runtime value representation for the MIR interpreter.

use std::cell::RefCell;
use std::rc::Rc;

/// A Tyra runtime value.
#[derive(Clone, Debug)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(Rc<str>),
    Unit,

    /// Value type struct AND ADT (tagged union).
    /// For ADTs, fields[0] = Int(tag). For value types, just fields in order.
    Struct {
        type_name: Rc<str>,
        fields: Vec<Value>,
    },

    /// Data type: reference semantics — mutations are visible via shared Rc.
    DataRef(Rc<RefCell<Vec<Value>>>),

    /// Immutable list; ListPush creates a new Rc clone with extra element.
    List(Rc<RefCell<Vec<Value>>>),

    /// HashMap / LinkedHashMap variant (stored in insertion order).
    Map(Rc<RefCell<Vec<(Value, Value)>>>),

    /// SortedMap variant (maintained in ascending key order).
    SortedMap(Rc<RefCell<Vec<(Value, Value)>>>),

    /// HashSet / LinkedHashSet variant (stored in insertion order).
    Set(Rc<RefCell<Vec<Value>>>),

    /// SortedSet variant (maintained in ascending element order).
    SortedSet(Rc<RefCell<Vec<Value>>>),

    /// Closure fat pointer: fn_name is the lifted lambda, env holds captures.
    Closure {
        fn_name: Rc<str>,
        env: Vec<Value>,
        env_struct_name: Rc<str>,
    },

    /// Boxed pointer for for-each callback unboxing (PtrLoad).
    BoxedPtr(Box<Value>),
}

impl Value {
    pub fn as_int(&self) -> i64 {
        match self {
            Value::Int(n) => *n,
            // Booleans are lowered to i1/i64 in LLVM; interpreter follows suit.
            Value::Bool(b) => if *b { 1 } else { 0 },
            _ => panic!("interpreter: expected Int, got {:?}", self),
        }
    }

    pub fn as_float(&self) -> f64 {
        match self {
            Value::Float(f) => *f,
            _ => panic!("interpreter: expected Float, got {:?}", self),
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            _ => panic!("interpreter: expected Bool, got {:?}", self),
        }
    }

    pub fn as_str(&self) -> Rc<str> {
        match self {
            Value::Str(s) => s.clone(),
            _ => panic!("interpreter: expected Str, got {:?}", self),
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            _ => panic!("interpreter: expected Bool for branch condition, got {:?}", self),
        }
    }
}

/// Total ordering on values that support it (Int, Bool, Str).
/// Used to maintain sort order in SortedMap/SortedSet.
pub fn key_cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Str(x), Value::Str(y)) => x.as_ref().cmp(y.as_ref()),
        _ => panic!("interpreter: unsupported key type for sorted collection"),
    }
}

/// Equality on key-capable values (Int, Bool, Str).
pub fn key_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        _ => false,
    }
}
