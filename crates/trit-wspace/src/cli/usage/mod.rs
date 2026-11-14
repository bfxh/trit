//! CLI 用法文本：标题 + 命令行表（运行时拼接，命令行表可独立增长）。

mod commands;

pub(super) fn usage_text() -> String {
    let mut text = String::from(
        "trit-wspace <command>
",
    );
    text.push_str(&commands::LINES.join(
        "
",
    ));
    text
}
