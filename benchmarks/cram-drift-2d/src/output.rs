//! 任务写出的 JSONL 格式（`main.rs` 只留 CLI 解析，避免它膨胀）。

use std::io::Write;

use crate::dataset::Task;

pub fn write_task(out: &mut impl Write, task: &Task) -> std::io::Result<()> {
    let grid: String = task
        .grid
        .iter()
        .map(|&v| if v == 0 { '.' } else { '#' })
        .collect();
    let path: Vec<String> = task.path.iter().map(|p| p.to_string()).collect();
    writeln!(
        out,
        "{{\"id\":{},\"grid\":\"{}\",\"start\":{},\"goal\":{},\"len\":{},\"path\":[{}]}}",
        task.id,
        grid,
        task.start,
        task.goal,
        task.path.len(),
        path.join(",")
    )
}
