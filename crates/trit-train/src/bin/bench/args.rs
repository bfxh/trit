//! `train-bench` 的命令行参数解析。

pub(super) fn arg(args: &[String], name: &str) -> Option<usize> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
}

pub(super) fn thread_list(args: &[String]) -> Vec<usize> {
    args.iter()
        .position(|a| a == "--threads")
        .and_then(|a| args.get(a + 1))
        .map(|s| {
            s.split(',')
                .filter_map(|v| v.parse::<usize>().ok())
                .collect()
        })
        .filter(|v: &Vec<usize>| !v.is_empty())
        .unwrap_or_else(|| vec![1, 2, 4, 8])
}
