//! Markdown 打印：块之间空一行；列表标记归一（`-`，相邻独立列表交替 `-`/`*`）；
//! 有序列表按起始号递增；表格按显示宽度对齐。

use super::width::str_width;
use super::{Align, Block, List, Table};

pub(crate) fn print_blocks(blocks: &[Block], indent: usize, out: &mut String) {
    let mut previous_list: Option<char> = None;
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            out.push_str("\n\n");
        }
        let bullet = match (block, previous_list) {
            (Block::List(_), Some('-')) => '*',
            (Block::List(_), Some('*')) => '-',
            (Block::List(_), None) => '-',
            _ => '-',
        };
        let rendered = render_block(block, indent, bullet, index == 0);
        out.push_str(&rendered);
        previous_list = match block {
            Block::List(_) => Some(bullet),
            _ => None,
        };
    }
}

fn pad(indent: usize) -> String {
    " ".repeat(indent)
}

fn render_block(block: &Block, indent: usize, bullet: char, is_first: bool) -> String {
    let pad = pad(indent);
    match block {
        Block::Heading { level, text } => {
            let hashes = "#".repeat(*level as usize);
            if text.is_empty() {
                format!("{pad}{hashes}")
            } else {
                format!("{pad}{hashes} {text}")
            }
        }
        Block::Paragraph { lines } => {
            let joined = lines.join(&format!("\n{pad}"));
            format!("{pad}{joined}")
        }
        Block::Rule => {
            // prettier：容器内第一条分隔线用 `***`，其余用 `---`（实测 3.10.1）。
            if is_first {
                format!("{pad}***")
            } else {
                format!("{pad}---")
            }
        }
        Block::Fence { info, body } => render_fence(info, body, &pad),
        Block::IndentedCode { body } => {
            let mut out = String::new();
            for (index, line) in body.split('\n').enumerate() {
                if index > 0 {
                    out.push('\n');
                }
                if !line.is_empty() {
                    out.push_str(&pad);
                    out.push_str("    ");
                    out.push_str(line);
                }
            }
            out
        }
        Block::Quote(children) => {
            let mut inner = String::new();
            print_blocks(children, 0, &mut inner);
            inner
                .split('\n')
                .map(|line| {
                    if line.is_empty() {
                        format!("{pad}>")
                    } else {
                        format!("{pad}> {line}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        Block::Table(table) => render_table(table, &pad),
        Block::Raw(lines) => lines.join(&format!("\n{pad}")),
        Block::List(list) => render_list(list, indent, bullet),
    }
}

fn render_fence(info: &str, body: &str, pad: &str) -> String {
    let body = body.trim_end_matches('\n');
    let mut longest = 0usize;
    for line in body.split('\n') {
        let run = line.trim_start().chars().take_while(|c| *c == '`').count();
        longest = longest.max(run);
    }
    let width = 3.max(longest + 1);
    let marker = "`".repeat(width);
    let mut out = String::new();
    out.push_str(pad);
    out.push_str(&marker);
    out.push_str(info);
    if !body.is_empty() {
        out.push('\n');
        for (index, line) in body.split('\n').enumerate() {
            if index > 0 {
                out.push('\n');
            }
            if !line.is_empty() {
                out.push_str(pad);
                out.push_str(line);
            }
        }
        out.push('\n');
    }
    out.push_str(pad);
    out.push_str(&marker);
    out
}

fn render_list(list: &List, indent: usize, bullet: char) -> String {
    let all_ones = list.items.iter().all(|item| item.number == 1);
    let mut out = String::new();
    for (index, item) in list.items.iter().enumerate() {
        if index > 0 {
            out.push('\n');
            if list.loose {
                out.push('\n');
            }
        }
        let marker = if list.ordered {
            if all_ones {
                "1.".to_owned()
            } else {
                format!("{}.", list.start + index as u64)
            }
        } else {
            bullet.to_string()
        };
        let content_indent = indent + marker.chars().count() + 1;
        let mut body = String::new();
        print_item_blocks(&item.blocks, content_indent, &mut body);
        out.push_str(&pad(indent));
        out.push_str(&marker);
        if item.blocks.is_empty() {
            continue;
        }
        let first_is_paragraph = matches!(item.blocks.first(), Some(Block::Paragraph { .. }));
        if first_is_paragraph {
            let content_pad = pad(content_indent);
            if body.starts_with(&content_pad) {
                out.push(' ');
                out.push_str(&body[content_pad.len()..]);
                continue;
            }
        }
        out.push('\n');
        out.push_str(&body);
    }
    out
}

fn print_item_blocks(blocks: &[Block], indent: usize, out: &mut String) {
    let mut previous_list: Option<char> = None;
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            let tight_nested_list = matches!(block, Block::List(_))
                && matches!(blocks.get(index - 1), Some(Block::Paragraph { .. }));
            if tight_nested_list {
                out.push('\n');
            } else {
                out.push_str("\n\n");
            }
        }
        let bullet = match (block, previous_list) {
            (Block::List(_), Some('-')) => '*',
            (Block::List(_), Some('*')) => '-',
            (Block::List(_), None) => '-',
            _ => '-',
        };
        let rendered = render_block(block, indent, bullet, index == 0);
        out.push_str(&rendered);
        previous_list = match block {
            Block::List(_) => Some(bullet),
            _ => None,
        };
    }
}

fn render_table(table: &Table, pad: &str) -> String {
    let columns = table.header.len();
    let mut widths = vec![3usize; columns];
    for (index, cell) in table.header.iter().enumerate() {
        widths[index] = widths[index].max(str_width(cell));
    }
    for row in &table.rows {
        for (index, cell) in row.iter().enumerate().take(columns) {
            widths[index] = widths[index].max(str_width(cell));
        }
    }
    let mut lines: Vec<String> = Vec::new();
    lines.push(table_row(&table.header, &widths, &table.align));
    let mut delimiter = Vec::with_capacity(columns);
    for (index, align) in table.align.iter().enumerate() {
        delimiter.push(delimiter_cell(*align, widths[index]));
    }
    lines.push(format!("| {} |", delimiter.join(" | ")));
    for row in &table.rows {
        lines.push(table_row(row, &widths, &table.align));
    }
    lines
        .into_iter()
        .map(|line| format!("{pad}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn table_row(cells: &[String], widths: &[usize], align: &[Align]) -> String {
    let mut out = Vec::with_capacity(cells.len());
    for (index, cell) in cells.iter().enumerate() {
        let width = widths[index];
        let cell_width = str_width(cell);
        let padding = width.saturating_sub(cell_width);
        let (left, right) = match align[index] {
            Align::Right => (padding, 0),
            Align::Center => (padding / 2, padding - padding / 2),
            Align::None | Align::Left => (0, padding),
        };
        out.push(format!("{}{}{}", " ".repeat(left), cell, " ".repeat(right)));
    }
    format!("| {} |", out.join(" | "))
}

fn delimiter_cell(align: Align, width: usize) -> String {
    match align {
        Align::None => "-".repeat(width),
        Align::Left => format!(":{}", "-".repeat(width.saturating_sub(1))),
        Align::Right => format!("{}:", "-".repeat(width.saturating_sub(1))),
        Align::Center => format!(":{}:", "-".repeat(width.saturating_sub(2))),
    }
}
