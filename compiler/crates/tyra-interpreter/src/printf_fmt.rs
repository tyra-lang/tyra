// Simple printf-style formatter used by StringFormat instruction.
//
// Handles the format specifiers emitted by MIR lowering:
//   %s  — string arg
//   %ld — integer arg
//   %g  — float arg (same as Rust's {:?} for compact notation)
//   %%  — literal %

use crate::value::Value;

/// Format `fmt_str` (a C printf-style string) substituting `args` in order.
/// Panics if arg count or types don't match specifiers (indicates a compiler bug).
pub fn printf_format(fmt_str: &str, args: &[Value]) -> String {
    let mut out = String::with_capacity(fmt_str.len() + args.len() * 8);
    let mut arg_idx = 0;
    let bytes = fmt_str.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' {
            i += 1;
            if i >= bytes.len() {
                break;
            }
            match bytes[i] {
                b'%' => {
                    out.push('%');
                }
                b's' => {
                    let arg = next_arg(args, &mut arg_idx, fmt_str);
                    match arg {
                        Value::Str(s) => out.push_str(s.as_ref()),
                        _ => out.push_str(&format_value(arg)),
                    }
                }
                b'l' => {
                    // %ld — integer
                    i += 1;
                    if i < bytes.len() && bytes[i] == b'd' {
                        let arg = next_arg(args, &mut arg_idx, fmt_str);
                        match arg {
                            Value::Int(n) => out.push_str(&n.to_string()),
                            Value::Bool(b) => out.push_str(if *b { "1" } else { "0" }),
                            _ => out.push_str(&format_value(arg)),
                        }
                    }
                }
                b'd' => {
                    // %d — integer (short form)
                    let arg = next_arg(args, &mut arg_idx, fmt_str);
                    match arg {
                        Value::Int(n) => out.push_str(&n.to_string()),
                        Value::Bool(b) => out.push_str(if *b { "1" } else { "0" }),
                        _ => out.push_str(&format_value(arg)),
                    }
                }
                b'g' => {
                    // %g — float (compact notation, no trailing zeros)
                    let arg = next_arg(args, &mut arg_idx, fmt_str);
                    match arg {
                        Value::Float(f) => out.push_str(&format_float(*f)),
                        _ => out.push_str(&format_value(arg)),
                    }
                }
                other => {
                    out.push('%');
                    out.push(other as char);
                }
            }
        } else {
            out.push(bytes[i] as char);
        }
        i += 1;
    }

    out
}

fn next_arg<'a>(args: &'a [Value], idx: &mut usize, fmt: &str) -> &'a Value {
    if *idx >= args.len() {
        panic!("interpreter: printf_format arg count mismatch for fmt {:?}", fmt);
    }
    let v = &args[*idx];
    *idx += 1;
    v
}

fn format_value(v: &Value) -> String {
    match v {
        Value::Int(n) => n.to_string(),
        Value::Float(f) => format_float(*f),
        Value::Bool(b) => if *b { "true".into() } else { "false".into() },
        Value::Str(s) => s.as_ref().to_string(),
        Value::Unit => "()".into(),
        _ => "<value>".into(),
    }
}

/// Match C `%g` behaviour: use exponential notation when |x| >= 1e6 or |x| < 1e-4
/// (and x != 0), strip trailing zeros.
pub fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if f == 0.0 {
        return "0".into();
    }
    let abs = f.abs();
    if abs >= 1e6 || (abs < 1e-4) {
        // Exponential form: strip trailing zeros from mantissa
        let s = format!("{:e}", f);
        strip_exp_zeros(&s)
    } else {
        // Fixed form: strip trailing zeros (and possibly trailing dot)
        let s = format!("{}", f);
        strip_fixed_zeros(&s)
    }
}

fn strip_fixed_zeros(s: &str) -> String {
    if !s.contains('.') {
        return s.to_string();
    }
    let s = s.trim_end_matches('0');
    let s = s.trim_end_matches('.');
    s.to_string()
}

fn strip_exp_zeros(s: &str) -> String {
    // Rust's {:e} format: "1.5e6" or "1.23456e-4"
    if let Some(pos) = s.find('e') {
        let (mantissa, exp_part) = s.split_at(pos);
        let mantissa = strip_fixed_zeros(mantissa);
        format!("{}{}", mantissa, exp_part)
    } else {
        strip_fixed_zeros(s)
    }
}
