//! 实验开关的环境变量解析。

pub(super) fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

pub(super) fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

pub(super) fn env_layers(name: &str) -> Vec<usize> {
    let mut layers: Vec<usize> = std::env::var(name)
        .ok()
        .map(|value| {
            value
                .split(',')
                .filter(|item| !item.trim().is_empty())
                .map(|item| item.trim().parse().expect("层号"))
                .collect()
        })
        .unwrap_or_default();
    layers.sort_unstable();
    layers.dedup();
    layers
}
