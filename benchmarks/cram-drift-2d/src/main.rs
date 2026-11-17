//! M0a：2D 障碍网格轨迹数据集 + BFS oracle（CLI）。
//!
//! 8×8 网格、20% 障碍、起点 (0,0)、终点取 BFS 可达集里最远的点（距离相同取索引最小）。
//! 路径由 BFS 给出（邻居顺序固定：上、右、下、左），所以整个数据集是确定性的。
//!
//! 运行：
//! ```text
//! cargo run --release -- --tasks 1000 --out dataset.jsonl [--images images.bin]
//! ```

mod dataset;

use std::fs::File;
use std::io::{BufWriter, Write};

use dataset::{Task, gen_task, render_3ch};

fn write_task(out: &mut impl Write, task: &Task) -> std::io::Result<()> {
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut tasks = 1000usize;
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut out_path: Option<String> = None;
    let mut images_path: Option<String> = None;
    let mut goals_path: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--tasks" => {
                tasks = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(tasks);
                i += 2;
            }
            "--seed" => {
                seed = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(seed);
                i += 2;
            }
            "--out" => {
                out_path = args.get(i + 1).cloned();
                i += 2;
            }
            "--images" => {
                images_path = args.get(i + 1).cloned();
                i += 2;
            }
            "--goals" => {
                goals_path = args.get(i + 1).cloned();
                i += 2;
            }
            other => {
                eprintln!("未知参数：{other}");
                std::process::exit(2);
            }
        }
    }
    let writer: Box<dyn Write> = match out_path {
        Some(path) => Box::new(BufWriter::new(File::create(path).expect("创建输出文件"))),
        None => Box::new(BufWriter::new(std::io::stdout())),
    };
    let mut out = writer;
    let mut images = images_path.map(|p| BufWriter::new(File::create(p).expect("创建图像文件")));
    let mut goals = goals_path.map(|p| BufWriter::new(File::create(p).expect("创建 goal 文件")));
    let mut state = seed;
    for id in 0..tasks {
        let task = gen_task(id, &mut state);
        write_task(&mut out, &task).expect("写任务");
        if let Some(img_out) = images.as_mut() {
            let mut buf = Vec::with_capacity(3072);
            render_3ch(&task, &mut buf);
            img_out.write_all(&buf).expect("写图像");
        }
        if let Some(g_out) = goals.as_mut() {
            g_out.write_all(&[task.goal as u8]).expect("写 goal");
        }
    }
    out.flush().expect("flush");
    if let Some(img_out) = images.as_mut() {
        img_out.flush().expect("flush images");
    }
    if let Some(g_out) = goals.as_mut() {
        g_out.flush().expect("flush goals");
    }
}
