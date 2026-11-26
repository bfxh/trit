//! `direct.c` 的运算符表声明：紧凑数组，不写长度与多余空格。

pub(crate) fn push_table(text: &mut String, name: &str, table: &[i8]) {
    text.push_str("static const int8_t ");
    text.push_str(name);
    text.push_str("[]={");
    let values: Vec<String> = table.iter().map(|value| value.to_string()).collect();
    text.push_str(&values.join(","));
    text.push_str("};\n");
}
