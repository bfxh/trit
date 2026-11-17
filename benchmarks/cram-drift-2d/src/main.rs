//! M0a：2D 障碍网格轨迹数据集 + BFS oracle。
//!
//! 8×8 网格、20% 障碍、起点 (0,0)、终点取 BFS 可达集里最远的点（距离相同取索引最小）。
//! 路径由 BFS 给出（邻居顺序固定：上、右、下、左），所以整个数据集是确定性的。
//!
//! 运行：
//! ```text
//! cargo run --release -- --tasks 1000 --out dataset.jsonl
//! ```

use std::fs::File;
use std::io::{BufWriter, Write};

const N: usize = 8;
const START: usize = 0;

struct Task {
    id: usize,
    grid: [u8; N * N],
    start: usize,
    goal: usize,
    path: Vec<usize>,
}

fn next(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn neighbors(i: usize) -> impl Iterator<Item = usize> {
    let (r, c) = (i / N, i % N);
    let mut out = [usize::MAX; 4];
    if r > 0 {
        out[0] = i - N;
    }
    if c + 1 < N {
        out[1] = i + 1;
    }
    if r + 1 < N {
        out[2] = i + N;
    }
    if c > 0 {
        out[3] = i - 1;
    }
    out.into_iter().filter(|&v| v != usize::MAX)
}

fn bfs_dist(grid: &[u8; N * N], start: usize) -> Vec<u32> {
    let mut dist = vec![u32::MAX; N * N];
    let mut queue = std::collections::VecDeque::new();
    dist[start] = 0;
    queue.push_back(start);
    while let Some(i) = queue.pop_front() {
        for j in neighbors(i) {
            if grid[j] == 0 && dist[j] == u32::MAX {
                dist[j] = dist[i] + 1;
                queue.push_back(j);
            }
        }
    }
    dist
}

fn bfs_path(grid: &[u8; N * N], start: usize, goal: usize) -> Option<Vec<usize>> {
    if grid[start] != 0 || grid[goal] != 0 {
        return None;
    }
    let mut prev = vec![usize::MAX; N * N];
    let mut seen = [false; N * N];
    let mut queue = std::collections::VecDeque::new();
    seen[start] = true;
    queue.push_back(start);
    while let Some(i) = queue.pop_front() {
        if i == goal {
            break;
        }
        for j in neighbors(i) {
            if grid[j] == 0 && !seen[j] {
                seen[j] = true;
                prev[j] = i;
                queue.push_back(j);
            }
        }
    }
    if !seen[goal] {
        return None;
    }
    let mut path = vec![goal];
    let mut cur = goal;
    while cur != start {
        cur = prev[cur];
        path.push(cur);
    }
    path.reverse();
    Some(path)
}

fn gen_task(id: usize, state: &mut u64) -> Task {
    let mut grid = [0u8; N * N];
    for cell in grid.iter_mut() {
        if next(state) % 100 < 20 {
            *cell = 1;
        }
    }
    grid[START] = 0;
    let dist = bfs_dist(&grid, START);
    // 可达集里最远的点；距离相同取索引最小的。
    let goal = dist
        .iter()
        .enumerate()
        .filter(|(_, d)| **d != u32::MAX)
        .max_by_key(|(i, d)| (**d, std::cmp::Reverse(*i)))
        .map(|(i, _)| i)
        .unwrap_or(START);
    grid[goal] = 0;
    let path = bfs_path(&grid, START, goal).unwrap_or_else(|| vec![START]);
    Task {
        id,
        grid,
        start: START,
        goal,
        path,
    }
}

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
    let mut state = seed;
    for id in 0..tasks {
        let task = gen_task(id, &mut state);
        write_task(&mut out, &task).expect("写任务");
    }
    out.flush().expect("flush");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_path_is_contiguous_and_clear() {
        let mut state = 0xDEAD_BEEF_1234_5678u64;
        for id in 0..200 {
            let task = gen_task(id, &mut state);
            assert_eq!(task.path.first(), Some(&task.start), "id={id}");
            assert_eq!(task.path.last(), Some(&task.goal), "id={id}");
            for pair in task.path.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                assert!(neighbors(a).any(|n| n == b), "id={id} 非相邻 {a}->{b}");
                assert_eq!(task.grid[b], 0, "id={id} 踩到障碍 {b}");
            }
        }
    }

    #[test]
    fn path_length_matches_bfs_distance() {
        let mut state = 0x1234_5678_9ABC_DEF0u64;
        for id in 0..200 {
            let task = gen_task(id, &mut state);
            let dist = bfs_dist(&task.grid, task.start);
            assert_eq!(
                task.path.len() as u32 - 1,
                dist[task.goal],
                "id={id} 路径不是最短"
            );
        }
    }

    #[test]
    fn unreachable_returns_none() {
        let mut grid = [0u8; N * N];
        grid[N] = 1;
        grid[1] = 1; // 把 (0,0) 围死
        assert!(bfs_path(&grid, 0, N * N - 1).is_none());
    }

    #[test]
    fn goal_is_farthest_reachable() {
        let mut state = 0x0BAD_F00D_CAFE_BABEu64;
        for id in 0..200 {
            let task = gen_task(id, &mut state);
            let dist = bfs_dist(&task.grid, task.start);
            let best = dist
                .iter()
                .enumerate()
                .filter(|(_, d)| **d != u32::MAX)
                .map(|(_, &d)| d)
                .max()
                .unwrap();
            assert_eq!(dist[task.goal], best, "id={id} 终点不是最远可达点");
        }
    }
}
