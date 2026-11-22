//! 标注包 CSV 的读写：最小 RFC 4180 子集（逗号、引号、`""` 转义、字段内换行）。
//!
//! 不引第三方 csv crate：格式只有我们自己写、自己读两种用法，行为要能一眼验。

/// 写出：表头 + 行。含逗号/引号/换行的字段加引号，内部引号双写。
pub fn encode(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    out.push_str(&encode_row(&headers.iter().map(|h| h.to_string()).collect::<Vec<_>>()));
    for row in rows {
        out.push_str(&encode_row(row));
    }
    out
}

fn encode_row(fields: &[String]) -> String {
    let mut out = String::new();
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        if field.contains([',', '"', '\n', '\r']) {
            out.push('"');
            for ch in field.chars() {
                if ch == '"' {
                    out.push('"');
                }
                out.push(ch);
            }
            out.push('"');
        } else {
            out.push_str(field);
        }
    }
    out.push('\n');
    out
}

/// 读出：返回（表头, 数据行）。空行忽略；引号未闭合报错。
pub fn decode(text: &str) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if quoted {
            match ch {
                '"' => {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        field.push('"');
                    } else {
                        quoted = false;
                    }
                }
                _ => field.push(ch),
            }
        } else {
            match ch {
                '"' if field.is_empty() => quoted = true,
                ',' => row.push(std::mem::take(&mut field)),
                '\r' => {}
                '\n' => {
                    row.push(std::mem::take(&mut field));
                    if !(row.len() == 1 && row[0].is_empty()) {
                        rows.push(std::mem::take(&mut row));
                    } else {
                        row.clear();
                    }
                }
                _ => field.push(ch),
            }
        }
    }
    if quoted {
        return Err("引号未闭合".to_string());
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        if !(row.len() == 1 && row[0].is_empty()) {
            rows.push(row);
        }
    }
    let mut headers = rows.first().cloned().unwrap_or_default();
    for header in &mut headers {
        *header = header.trim().to_string();
    }
    Ok((headers, rows.into_iter().skip(1).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_roundtrips_commas_quotes_and_newlines() {
        let rows = vec![vec![
            "0".to_string(),
            "defer".to_string(),
            "含,逗号".to_string(),
            "带\"引号\"".to_string(),
            "两行\n文本".to_string(),
        ]];
        let text = encode(&["id", "decision", "a", "b", "text"], &rows);
        let (headers, back) = decode(&text).expect("解析");
        assert_eq!(headers, vec!["id", "decision", "a", "b", "text"]);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0][2], "含,逗号");
        assert_eq!(back[0][3], "带\"引号\"");
        assert_eq!(back[0][4], "两行\n文本");
    }

    #[test]
    fn empty_cells_and_crlf_are_kept() {
        let (headers, rows) = decode("a,b,c\r\n1,,3\r\n").expect("解析");
        assert_eq!(headers.len(), 3);
        assert_eq!(rows, vec![vec!["1".to_string(), String::new(), "3".to_string()]]);
    }

    #[test]
    fn unclosed_quote_is_an_error() {
        assert!(decode("a,b\n\"x,y").is_err());
    }
}
