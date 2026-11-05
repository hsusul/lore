//! JSON text the way Python's `json.dumps` writes it by default.
//!
//! The labeling judge wrote `tool_pattern` regexes against the prototype's
//! rendering of a tool call, `f"{name} {json.dumps(input)}"`: `", "` and
//! `": "` separators and non-ASCII escaped as `\uXXXX`. Matching the same text
//! keeps a pattern's verdict the same in both graders. Keys come out sorted
//! (serde_json's map order), where Python keeps the agent's order; a pattern
//! that spans two keys could differ, and none of the labeled ones do.

use serde_json::Value;

/// `json.dumps(value)` with Python's defaults.
#[must_use]
pub fn dumps(value: &Value) -> String {
    let mut out = String::new();
    write(value, &mut out);
    out
}

fn write(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                string(k, out);
                out.push_str(": ");
                write(v, out);
            }
            out.push('}');
        }
    }
}

fn string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if c.is_ascii() && !c.is_ascii_control() => out.push(c),
            c => {
                let mut units = [0_u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_python_defaults() {
        let v = json!({"command": "echo \"hé\" 😀\n", "n": [1, 2.5, null, true]});
        assert_eq!(
            dumps(&v),
            r#"{"command": "echo \"h\u00e9\" \ud83d\ude00\n", "n": [1, 2.5, null, true]}"#
        );
    }
}
