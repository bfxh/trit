//! 表格格式判定：每行同一分隔符数量一致且至少两列。

pub fn is_table(lines: &[&str], sep: char) -> bool {
    if lines.len() < 2 {
        return false;
    }
    let cols: Vec<usize> = lines.iter().map(|l| l.matches(sep).count()).collect();
    cols[0] >= 1 && cols.iter().all(|c| *c == cols[0])
}
