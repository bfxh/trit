//! GFM 表格：表头 + 分隔行 + 行；行可以不带竖线（prettier 会把整行当单格行）。

use super::parse::{is_blank, normalize_inline, starts_block};
use super::{Align, Table};

pub(crate) fn try_table(lines: &[String], start: usize) -> Option<(Table, usize)> {
    let header = &lines[start];
    if !header.contains('|') {
        return None;
    }
    let delimiter = lines.get(start + 1)?;
    let header_cells = split_cells(header);
    let delimiter_cells = split_cells(delimiter);
    if header_cells.len() != delimiter_cells.len() || delimiter_cells.is_empty() {
        return None;
    }
    let mut align = Vec::with_capacity(delimiter_cells.len());
    for cell in &delimiter_cells {
        let text = cell.trim();
        let left = text.starts_with(':');
        let right = text.ends_with(':') && text.len() > 1;
        let dashes = text.trim_start_matches(':').trim_end_matches(':');
        if dashes.is_empty() || !dashes.chars().all(|c| c == '-') {
            return None;
        }
        align.push(match (left, right) {
            (true, true) => Align::Center,
            (true, false) => Align::Left,
            (false, true) => Align::Right,
            (false, false) => Align::None,
        });
    }
    let mut rows = Vec::new();
    let mut i = start + 2;
    while i < lines.len() {
        let line = &lines[i];
        if is_blank(line) {
            break;
        }
        if !line.contains('|') && starts_block(line) {
            break;
        }
        let mut cells = split_cells(line);
        cells.truncate(header_cells.len());
        rows.push(
            cells
                .into_iter()
                .map(|cell| normalize_inline(cell.trim()))
                .collect(),
        );
        i += 1;
    }
    Some((
        Table {
            align,
            header: header_cells
                .into_iter()
                .map(|cell| normalize_inline(cell.trim()))
                .collect(),
            rows,
        },
        i,
    ))
}

fn split_cells(line: &str) -> Vec<String> {
    let text = line.trim();
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for c in text.chars() {
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => {
                current.push(c);
                escaped = true;
            }
            '|' => {
                cells.push(current.clone());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    cells.push(current);
    if text.starts_with('|') && cells.first().is_some_and(|cell| cell.trim().is_empty()) {
        cells.remove(0);
    }
    if text.ends_with('|') && cells.last().is_some_and(|cell| cell.trim().is_empty()) {
        cells.pop();
    }
    cells
        .into_iter()
        .map(|cell| cell.trim().to_owned())
        .collect()
}
