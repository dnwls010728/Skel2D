//! Diff-friendly JSON output: containers that fit on one line are written
//! inline, larger ones are expanded one entry per line. Keeps files short to
//! read (for humans and for AI context windows) and keys diff cleanly.

use serde::Serialize;
use serde_json::{Number, Value};

const MAX_INLINE: usize = 140;

pub fn to_string<T: Serialize>(value: &T) -> serde_json::Result<String> {
    let mut value = serde_json::to_value(value)?;
    clean_floats(&mut value);
    let mut out = String::new();
    write_value(&mut out, &value, 0);
    out.push('\n');
    Ok(out)
}

/// Single-line JSON with the same float cleanup, for tool output.
pub fn to_compact<T: Serialize>(value: &T) -> serde_json::Result<String> {
    let mut value = serde_json::to_value(value)?;
    clean_floats(&mut value);
    Ok(value.to_string())
}

/// serde_json widens f32 to f64, so 0.6f32 would print as
/// 0.6000000238418579. Numbers that round-trip through f32 are rewritten
/// with their shortest f32 representation.
pub fn clean_floats(v: &mut Value) {
    match v {
        Value::Number(n) if n.is_f64() => {
            let f = n.as_f64().unwrap_or_default();
            let narrow = f as f32;
            if narrow as f64 == f
                && let Some(clean) = narrow.to_string().parse::<f64>().ok().and_then(Number::from_f64)
            {
                *n = clean;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(clean_floats),
        Value::Object(map) => map.values_mut().for_each(clean_floats),
        _ => {}
    }
}

fn write_value(out: &mut String, v: &Value, indent: usize) {
    let inline = v.to_string();
    let expandable = matches!(v, Value::Array(a) if !a.is_empty()) || matches!(v, Value::Object(o) if !o.is_empty());
    if !expandable || indent * 2 + inline.len() <= MAX_INLINE {
        out.push_str(&inline);
        return;
    }
    let pad = "  ".repeat(indent + 1);
    match v {
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                out.push_str(if i == 0 { "\n" } else { ",\n" });
                out.push_str(&pad);
                write_value(out, item, indent + 1);
            }
            out.push('\n');
            out.push_str(&"  ".repeat(indent));
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (k, item)) in map.iter().enumerate() {
                out.push_str(if i == 0 { "\n" } else { ",\n" });
                out.push_str(&pad);
                out.push_str(&Value::String(k.clone()).to_string());
                out.push_str(": ");
                write_value(out, item, indent + 1);
            }
            out.push('\n');
            out.push_str(&"  ".repeat(indent));
            out.push('}');
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn f32_noise_is_removed() {
        let out = super::to_compact(&serde_json::json!({ "a": 0.6f32, "b": [0.8f32, 59.31f32], "c": 0.1f64 })).unwrap();
        assert_eq!(out, r#"{"a":0.6,"b":[0.8,59.31],"c":0.1}"#);
    }
}
