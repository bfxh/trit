//! Markdown 块级解析：行扫描 + 容器递归（引用块 / 列表项内部再解析）。
//!
//! 只做**块级**；行内文本保持原样，只折叠代码段之外的连续空格。
//! 不认识的构造返回 `Unsupported`——不猜、不静默改写。

use super::{Align, Block, List, ListItem, Table};
use crate::FormatError;

pub(crate) fn parse_document(source: &str) -> Result<Vec<Block>, FormatError> {
    let lines: Vec<String> = source
        .split('\n')
        .map(|line| line.trim_end_matches('\r').to_owned())
        .collect();
    parse_blocks(&lines)
}

fn parse_blocks(lines: &[String]) -> Result<Vec<Block>, FormatError> {
    let mut out: Vec<Block> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        if is_blank(line) {
            i += 1;
            continue;
        }
        if let Some((level, text)) = heading(line) {
            out.push(Block::Heading { level, text });
            i += 1;
            continue;
        }
        if let Some((fence, run, info)) = fence_open(line) {
            let (body, next) = collect_fence(lines, i, fence, run);
            out.push(Block::Fence { info, body });
            i = next;
            continue;
        }
        if is_rule(line) {
            if is_frontmatter(lines, i) {
                return Err(FormatError::Unsupported("markdown: frontmatter"));
            }
            out.push(Block::Rule);
            i += 1;
            continue;
        }
        if quote_body(line).is_some() {
            let (blocks, next) = collect_quote(lines, i)?;
            out.push(Block::Quote(blocks));
            i = next;
            continue;
        }
        if let Some(marker) = list_marker(line) {
            let (list, next) = collect_list(lines, i, marker)?;
            out.push(Block::List(list));
            i = next;
            continue;
        }
        if let Some((table, next)) = try_table(lines, i) {
            out.push(Block::Table(table));
            i = next;
            continue;
        }
        if is_html_start(line) {
            let (raw, next) = collect_html(lines, i);
            out.push(Block::Raw(raw));
            i = next;
            continue;
        }
        if leading_spaces(line) >= 4 {
            let (body, next) = collect_indented_code(lines, i);
            out.push(Block::IndentedCode { body });
            i = next;
            continue;
        }
        let (block, next) = collect_paragraph(lines, i)?;
        out.push(block);
        i = next;
    }
    Ok(out)
}

// ---------- 行级判定 ----------

pub(crate) fn is_blank(line: &str) -> bool {
    line.chars().all(|c| c == ' ' || c == '\t')
}

pub(crate) fn leading_spaces(line: &str) -> usize {
    let mut count = 0;
    for c in line.chars() {
        match c {
            ' ' => count += 1,
            '\t' => count += 4,
            _ => break,
        }
    }
    count
}

/// ATX 标题：0–3 空格缩进 + 1–6 个 `#` + 空格/制表符或行尾。
fn heading(line: &str) -> Option<(u8, String)> {
    let indent = leading_spaces(line);
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let hashes = rest.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let after = &rest[hashes..];
    if !after.is_empty() && !after.starts_with([' ', '\t']) {
        return None;
    }
    let mut text = after.trim_start_matches([' ', '\t']).trim_end().to_owned();
    let hash_tail = text.chars().rev().take_while(|c| *c == '#').count();
    if hash_tail > 0 {
        let before = &text[..text.len() - hash_tail];
        if before.is_empty() || before.ends_with([' ', '\t']) {
            text = before.trim_end().to_owned();
        }
    }
    Some((hashes as u8, normalize_inline(&text)))
}

/// 围栏开头：返回（字符、长度、语言标记）。
fn fence_open(line: &str) -> Option<(char, usize, String)> {
    let indent = leading_spaces(line);
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let first = rest.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let run = rest.chars().take_while(|c| *c == first).count();
    if run < 3 {
        return None;
    }
    let info = rest[run..].trim().to_owned();
    if first == '`' && info.contains('`') {
        return None;
    }
    Some((first, run, info))
}

fn collect_fence(lines: &[String], start: usize, fence: char, run: usize) -> (String, usize) {
    let indent = leading_spaces(&lines[start]);
    let mut body = String::new();
    let mut i = start + 1;
    while i < lines.len() {
        if let Some((close, close_run, extra)) = fence_close(&lines[i]) {
            if close == fence && close_run >= run && extra.is_empty() {
                i += 1;
                break;
            }
        }
        let line = &lines[i];
        let cut = leading_spaces(line).min(indent);
        body.push_str(&line[cut..]);
        body.push('\n');
        i += 1;
    }
    (body, i)
}

fn fence_close(line: &str) -> Option<(char, usize, String)> {
    let indent = leading_spaces(line);
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let first = rest.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let run = rest.chars().take_while(|c| *c == first).count();
    if run < 3 {
        return None;
    }
    Some((first, run, rest[run..].trim().to_owned()))
}

fn is_rule(line: &str) -> bool {
    let indent = leading_spaces(line);
    if indent > 3 {
        return false;
    }
    let rest = line[indent..].trim_end();
    let mut marker = None;
    let mut count = 0;
    for c in rest.chars() {
        match c {
            ' ' | '\t' => {}
            '*' | '-' | '_' => {
                if marker.is_none() {
                    marker = Some(c);
                }
                if marker != Some(c) {
                    return false;
                }
                count += 1;
            }
            _ => return false,
        }
    }
    count >= 3
}

/// 引用行：返回 `>` 之后的内容（去掉一个空格）。
fn quote_body(line: &str) -> Option<String> {
    let indent = leading_spaces(line);
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let after = rest.strip_prefix('>')?;
    Some(match after.strip_prefix(' ') {
        Some(text) => text.to_owned(),
        None => after.to_owned(),
    })
}

fn collect_quote(lines: &[String], start: usize) -> Result<(Vec<Block>, usize), FormatError> {
    let mut inner: Vec<String> = Vec::new();
    let mut i = start;
    while i < lines.len() {
        if let Some(body) = quote_body(&lines[i]) {
            inner.push(body);
            i += 1;
            continue;
        }
        break;
    }
    Ok((parse_blocks(&inner)?, i))
}

#[derive(Debug, Clone, Copy)]
struct Marker {
    ordered: bool,
    number: u64,
    indent: usize,
    /// 标记字符：无序是 `-`/`+`/`*`，有序是 `.`/`)`。
    kind_char: char,
    /// 内容起始列：续行按这个列去缩进。
    content_indent: usize,
    /// 首行内容在原始行里的字节偏移。
    content_offset: usize,
}

/// 列表标记：0–3 空格缩进 + `-`/`+`/`*` 或 `1.` / `1)`，后面至少一个空格。
fn list_marker(line: &str) -> Option<Marker> {
    let indent = leading_spaces(line);
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let first = rest.chars().next()?;
    if matches!(first, '-' | '+' | '*') {
        let after = rest[1..].chars().next()?;
        if after != ' ' && after != '\t' {
            return None;
        }
        let spaces = count_space(&rest[1..]);
        return Some(Marker {
            ordered: false,
            number: 0,
            indent,
            kind_char: first,
            content_indent: indent + 1 + spaces.min(4),
            content_offset: indent + 1 + spaces,
        });
    }
    if !first.is_ascii_digit() {
        return None;
    }
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 9 {
        return None;
    }
    let delimiter = rest[digits..].chars().next()?;
    if delimiter != '.' && delimiter != ')' {
        return None;
    }
    let after = rest[digits + 1..].chars().next()?;
    if after != ' ' && after != '\t' {
        return None;
    }
    let number: u64 = rest[..digits].parse().ok()?;
    let spaces = count_space(&rest[digits + 1..]);
    Some(Marker {
        ordered: true,
        number,
        indent,
        kind_char: delimiter,
        content_indent: indent + digits + 1 + spaces.min(4),
        content_offset: indent + digits + 1 + spaces,
    })
}

fn count_space(text: &str) -> usize {
    text.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

fn collect_list(
    lines: &[String],
    start: usize,
    first: Marker,
) -> Result<(List, usize), FormatError> {
    let mut items: Vec<ListItem> = Vec::new();
    let mut loose = false;
    let mut i = start;
    loop {
        let marker = match list_marker(&lines[i]) {
            Some(marker)
                if marker.ordered == first.ordered
                    && marker.kind_char == first.kind_char
                    && marker.indent == first.indent =>
            {
                marker
            }
            _ => break,
        };
        let mut item_lines: Vec<String> = vec![lines[i][marker.content_offset..].to_owned()];
        let mut j = i + 1;
        let mut blanks = 0usize;
        while j < lines.len() {
            let line = &lines[j];
            if is_blank(line) {
                blanks += 1;
                j += 1;
                continue;
            }
            let indent = leading_spaces(line);
            if let Some(next) = list_marker(line) {
                if next.indent <= first.indent + 1 {
                    if blanks > 0 {
                        loose = true;
                    }
                    break;
                }
            }
            if indent >= marker.content_indent {
                if blanks > 0 {
                    loose = true;
                }
                for _ in 0..blanks {
                    item_lines.push(String::new());
                }
                blanks = 0;
                let cut = marker.content_indent.min(indent);
                item_lines.push(line[cut..].to_owned());
                j += 1;
                continue;
            }
            if blanks == 0 && indent >= first.indent + 2 {
                item_lines.push(line.trim_start().to_owned());
                j += 1;
                continue;
            }
            break;
        }
        let blocks = parse_blocks(&item_lines)?;
        if !blocks.is_empty() {
            items.push(ListItem {
                number: marker.number,
                blocks,
            });
        }
        i = j;
        if i >= lines.len() {
            break;
        }
        match list_marker(&lines[i]) {
            Some(next)
                if next.ordered == first.ordered
                    && next.kind_char == first.kind_char
                    && next.indent == first.indent => {}
            _ => break,
        }
    }
    Ok((
        List {
            ordered: first.ordered,
            start: first.number,
            items,
            loose,
        },
        i,
    ))
}

fn is_html_start(line: &str) -> bool {
    let indent = leading_spaces(line);
    if indent > 3 {
        return false;
    }
    let rest = &line[indent..];
    rest.starts_with('<')
        && rest[1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '/')
}

fn collect_html(lines: &[String], start: usize) -> (Vec<String>, usize) {
    let mut raw = Vec::new();
    let mut i = start;
    while i < lines.len() && !is_blank(&lines[i]) {
        raw.push(lines[i].clone());
        i += 1;
    }
    (raw, i)
}

fn collect_indented_code(lines: &[String], start: usize) -> (String, usize) {
    let mut body = String::new();
    let mut i = start;
    while i < lines.len() {
        let line = &lines[i];
        if is_blank(line) {
            body.push('\n');
            i += 1;
            continue;
        }
        if leading_spaces(line) < 4 {
            break;
        }
        let cut = line
            .char_indices()
            .nth(4)
            .map(|(index, _)| index)
            .unwrap_or(line.len());
        body.push_str(&line[cut..]);
        body.push('\n');
        i += 1;
    }
    (body.trim_end_matches('\n').to_owned(), i)
}

fn try_table(lines: &[String], start: usize) -> Option<(Table, usize)> {
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

fn collect_paragraph(lines: &[String], start: usize) -> Result<(Block, usize), FormatError> {
    let mut body: Vec<String> = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let line = &lines[i];
        if is_blank(line) {
            break;
        }
        if i > start && starts_block(line) {
            break;
        }
        if is_setext_underline(line) {
            return Err(FormatError::Unsupported("markdown: setext heading"));
        }
        body.push(normalize_paragraph_line(line));
        i += 1;
    }
    Ok((Block::Paragraph { lines: body }, i))
}

fn starts_block(line: &str) -> bool {
    heading(line).is_some()
        || fence_open(line).is_some()
        || is_rule(line)
        || quote_body(line).is_some()
        || list_marker(line).is_some()
        || is_html_start(line)
        || leading_spaces(line) >= 4
}

fn is_frontmatter(lines: &[String], start: usize) -> bool {
    if start != 0 || lines[0].trim_end() != "---" {
        return false;
    }
    lines.iter().skip(1).any(|line| {
        let text = line.trim_end();
        text == "---" || text == "..."
    })
}

fn is_setext_underline(line: &str) -> bool {
    let indent = leading_spaces(line);
    if indent > 3 {
        return false;
    }
    let text = line[indent..].trim_end();
    !text.is_empty() && (text.chars().all(|c| c == '=') || text.chars().all(|c| c == '-'))
}

/// 段落归一：去掉前导空白、折叠内部空白、保留硬换行（行尾 ≥2 空格）。
fn normalize_paragraph_line(line: &str) -> String {
    let text = line.trim_start_matches([' ', '\t']);
    let trimmed = text.trim_end();
    let trailing = text.len() - trimmed.len();
    let mut out = normalize_inline(trimmed);
    if trailing >= 2 {
        out.push_str("  ");
    }
    out
}

/// 行内归一：折叠空白 + 强调符归一（prettier 用 `_` 表示斜体、`**` 表示加粗）。
pub(crate) fn normalize_inline(text: &str) -> String {
    normalize_emphasis(&collapse_spaces(text))
}

/// 强调符归一（只在不改变文字内容的前提下换标记）：
/// - `*x*` → `_x_`，`__x__` → `**x**`；
/// - 要求开符前是行首/空白/标点、开符后不是空白；闭符前不是空白、闭符后是行尾/空白/标点；
/// - 反引号代码段里的内容原样保留。
fn normalize_emphasis(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            let run = chars[i..].iter().take_while(|x| **x == '`').count();
            // 行内代码：整段原样拷贝，避免把代码里的 `*`/`_` 当强调符。
            let mut j = i + run;
            let mut closed = false;
            while j < chars.len() {
                if chars[j] == '`' {
                    let width = chars[j..].iter().take_while(|x| **x == '`').count();
                    if width == run {
                        let mut body: Vec<char> = chars[i + run..j].to_vec();
                        // CommonMark：代码段内容若以空格开头且以空格结尾、且不全为空格，去掉各一个。
                        if body.len() >= 2
                            && body.first() == Some(&' ')
                            && body.last() == Some(&' ')
                            && body.iter().any(|c| *c != ' ')
                        {
                            body.remove(0);
                            body.pop();
                        }
                        for _ in 0..run {
                            out.push('`');
                        }
                        for c in &body {
                            out.push(*c);
                        }
                        for _ in 0..run {
                            out.push('`');
                        }
                        i = j + width;
                        closed = true;
                        break;
                    }
                    j += width;
                } else {
                    j += 1;
                }
            }
            if closed {
                continue;
            }
            for _ in 0..run {
                out.push('`');
            }
            i += run;
            continue;
        }
        if (c == '*' || c == '_') && is_boundary(out.chars().last()) {
            let run = chars[i..].iter().take_while(|x| **x == c).count();
            if run <= 2 && i + run < chars.len() && !chars[i + run].is_whitespace() {
                if let Some(close) = find_closer(&chars, i + run, c, run) {
                    let marker = match (c, run) {
                        ('*', 1) => '_',
                        ('_', 2) => '*',
                        _ => c,
                    };
                    for _ in 0..run {
                        out.push(marker);
                    }
                    let inner: String = chars[i + run..close].iter().collect();
                    out.push_str(&normalize_emphasis(&inner));
                    for _ in 0..run {
                        out.push(marker);
                    }
                    i = close + run;
                    continue;
                }
            }
            for _ in 0..run {
                out.push(c);
            }
            i += run;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn find_closer(chars: &[char], from: usize, marker: char, run: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            let width = chars[i..].iter().take_while(|x| **x == '`').count();
            i += width;
            continue;
        }
        if c == marker {
            let width = chars[i..].iter().take_while(|x| **x == marker).count();
            if width == run
                && i > 0
                && !chars[i - 1].is_whitespace()
                && is_boundary(chars.get(i + width).copied())
            {
                return Some(i);
            }
            i += width;
            continue;
        }
        i += 1;
    }
    None
}

fn is_boundary(c: Option<char>) -> bool {
    match c {
        None => true,
        Some(c) => {
            c.is_whitespace()
                || matches!(
                    c,
                    '，' | '。'
                        | '、'
                        | '；'
                        | '：'
                        | '！'
                        | '？'
                        | '（'
                        | '）'
                        | '【'
                        | '】'
                        | '「'
                        | '」'
                        | '《'
                        | '》'
                        | '〈'
                        | '〉'
                        | '“'
                        | '”'
                        | '‘'
                        | '’'
                        | '…'
                        | '—'
                        | ','
                        | '.'
                        | ';'
                        | ':'
                        | '!'
                        | '?'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '<'
                        | '>'
                        | '"'
                        | '\''
                )
        }
    }
}

/// 折叠代码段之外的连续空白（制表符也算空白）；行内代码里的空格原样保留。
pub(crate) fn collapse_spaces(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_code: Option<usize> = None;
    let mut pending = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            let run = chars[i..].iter().take_while(|x| **x == '`').count();
            match in_code {
                None => in_code = Some(run),
                Some(width) if width == run => in_code = None,
                _ => {}
            }
            if pending && !out.is_empty() {
                out.push(' ');
            }
            pending = false;
            for _ in 0..run {
                out.push('`');
            }
            i += run;
            continue;
        }
        if in_code.is_none() && (c == ' ' || c == '\t') {
            pending = true;
            i += 1;
            continue;
        }
        if pending && !out.is_empty() {
            out.push(' ');
        }
        pending = false;
        out.push(c);
        i += 1;
    }
    out
}
