//! 2D 障碍网格数据集：生成、BFS oracle、渲染。

pub const N: usize = 8;
pub const START: usize = 0;

pub struct Task {
    pub id: usize,
    pub grid: [u8; N * N],
    pub start: usize,
    pub goal: usize,
    pub path: Vec<usize>,
}

fn next(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

pub fn neighbors(i: usize) -> impl Iterator<Item = usize> {
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

pub fn bfs_dist(grid: &[u8; N * N], start: usize) -> Vec<u32> {
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

pub fn bfs_path(grid: &[u8; N * N], start: usize, goal: usize) -> Option<Vec<usize>> {
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

pub fn gen_task(id: usize, state: &mut u64) -> Task {
    let mut grid = [0u8; N * N];
    for cell in grid.iter_mut() {
        if next(state) % 100 < 20 {
            *cell = 1;
        }
    }
    grid[START] = 0;
    let dist = bfs_dist(&grid, START);
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

/// 把轨迹渲染成 32×32（8×8 每格放大 4×）三通道 NHWC uint8，供 driftnet 训练。
pub fn render_3ch(task: &Task, out: &mut Vec<u8>) {
    let mut img = [0u8; 32 * 32];
    for &p in &task.path {
        let (r, c) = (p / N, p % N);
        for dr in 0..4 {
            for dc in 0..4 {
                img[(r * 4 + dr) * 32 + c * 4 + dc] = 255;
            }
        }
    }
    for &v in img.iter() {
        out.push(v);
        out.push(v);
        out.push(v);
    }
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
        grid[1] = 1;
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
                .filter(|d| **d != u32::MAX)
                .map(|d| *d)
                .max()
                .unwrap();
            assert_eq!(dist[task.goal], best, "id={id} 终点不是最远可达点");
        }
    }
}
