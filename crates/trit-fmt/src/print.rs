//! JSON 打印：与 prettier 3.10.1 的 `--parser json` 逐字节对齐（含空行分组与跨行保持）。

use crate::{Obj, PRINT_WIDTH, Seq, Value};

fn is_scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

/// 需要强制展开的情况（对 prettier 3.10.1 实测）：
/// - 数组：**忽略**源码跨行；有空白行就保持展开（`[1,\n\n2]` 输出多行带空行）；
///   或"元素数 ≥ 2 且内层数组元素数也 ≥ 2"（`[[1,2],[3,4]]` 多行、`[[1],[2]]` 单行）；
/// - 对象：源码跨行就保持跨行；成员值里有需要展开的容器也会带下来。
fn forces_break(value: &Value) -> bool {
    match value {
        Value::Array(seq) => {
            seq.items.iter().any(|node| node.blank_before)
                || (seq.items.len() >= 2
                    && seq.items.iter().all(|node| match &node.value {
                        Value::Array(inner) => inner.items.len() >= 2,
                        _ => false,
                    }))
        }
        Value::Object(obj) => {
            obj.multiline
                || obj.fields.iter().any(|field| field.blank_before)
                || obj.fields.iter().any(|field| forces_break(&field.value))
        }
        _ => false,
    }
}

/// 单行（flat）渲染；用于宽度判断与 fill。
pub(crate) fn flat(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(true) => "true".to_owned(),
        Value::Bool(false) => "false".to_owned(),
        Value::Number(text) => text.clone(),
        Value::String(text) => text.clone(),
        Value::Array(seq) => {
            if seq.items.is_empty() {
                return "[]".to_owned();
            }
            let body: Vec<String> = seq.items.iter().map(|node| flat(&node.value)).collect();
            format!("[{}]", body.join(", "))
        }
        Value::Object(obj) => {
            if obj.fields.is_empty() {
                return "{}".to_owned();
            }
            let body: Vec<String> = obj
                .fields
                .iter()
                .map(|field| format!("{}: {}", field.key, flat(&field.value)))
                .collect();
            format!("{{ {} }}", body.join(", "))
        }
    }
}

fn indent(level: usize, out: &mut String) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn current_column(out: &str) -> usize {
    match out.rfind('\n') {
        Some(index) => out[index + 1..].chars().count(),
        None => out.chars().count(),
    }
}

pub(crate) fn print_value(value: &Value, level: usize, out: &mut String) {
    let inline = flat(value);
    let column = current_column(out);
    if !forces_break(value) && column + inline.chars().count() <= PRINT_WIDTH {
        out.push_str(&inline);
        return;
    }
    match value {
        Value::Array(seq) => print_array(seq, level, out),
        Value::Object(obj) => print_object(obj, level, out),
        _ => out.push_str(&inline),
    }
}

fn print_array(seq: &Seq, level: usize, out: &mut String) {
    if seq.items.is_empty() {
        out.push_str("[]");
        return;
    }
    // 全标量且没有空白行：能单行就单行（由 `print_value` 决定），超宽才 fill；
    // 有空白行时保持展开，空行照 prettier 的实测结果保留。
    let has_blank = seq.items.iter().any(|node| node.blank_before);
    if !has_blank && seq.items.iter().all(|node| is_scalar(&node.value)) {
        fill_scalars(&seq.items, level, out);
        return;
    }
    out.push_str("[\n");
    for (index, node) in seq.items.iter().enumerate() {
        if index > 0 && node.blank_before {
            out.push('\n');
        }
        indent(level + 1, out);
        print_value(&node.value, level + 1, out);
        if index + 1 < seq.items.len() {
            out.push(',');
        }
        out.push('\n');
    }
    indent(level, out);
    out.push(']');
}

/// prettier 对全标量数组用 fill：尽量把元素填满一行。
fn fill_scalars(items: &[crate::Node], level: usize, out: &mut String) {
    out.push_str("[\n");
    let base = (level + 1) * 2;
    let mut column = base;
    indent(level + 1, out);
    for (index, node) in items.iter().enumerate() {
        let text = flat(&node.value);
        let need_comma = index + 1 < items.len();
        let piece = text.chars().count() + usize::from(need_comma);
        if index > 0 {
            if column + 1 + piece > PRINT_WIDTH {
                out.push('\n');
                indent(level + 1, out);
                column = base;
            } else {
                out.push(' ');
                column += 1;
            }
        }
        out.push_str(&text);
        column += text.chars().count();
        if need_comma {
            out.push(',');
            column += 1;
        }
    }
    out.push('\n');
    indent(level, out);
    out.push(']');
}

fn print_object(obj: &Obj, level: usize, out: &mut String) {
    if obj.fields.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push_str("{\n");
    for (index, field) in obj.fields.iter().enumerate() {
        if index > 0 && field.blank_before {
            out.push('\n');
        }
        indent(level + 1, out);
        out.push_str(&field.key);
        out.push_str(": ");
        print_value(&field.value, level + 1, out);
        if index + 1 < obj.fields.len() {
            out.push(',');
        }
        out.push('\n');
    }
    indent(level, out);
    out.push('}');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Field, Node, Seq};

    #[test]
    fn blank_before_keeps_one_blank_line() {
        let seq = Seq {
            items: vec![
                Node {
                    value: Value::Number("1".into()),
                    blank_before: false,
                },
                Node {
                    value: Value::Number("2".into()),
                    blank_before: true,
                },
            ],
            multiline: true,
        };
        let mut out = String::new();
        print_array(&seq, 0, &mut out);
        assert_eq!(out, "[\n  1,\n\n  2\n]");
    }

    #[test]
    fn multiline_object_stays_broken_even_when_narrow() {
        let obj = Obj {
            fields: vec![Field {
                key: "\"a\"".into(),
                value: Value::Number("1".into()),
                blank_before: false,
            }],
            multiline: true,
        };
        let mut out = String::new();
        print_object(&obj, 0, &mut out);
        assert_eq!(out, "{\n  \"a\": 1\n}");
    }
}
