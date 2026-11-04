//! 三值基准测量的**底件**：把散在各 crate 的计时套路收成一份。
//!
//! 立项依据是实测（2026-11-05 清点）：仓内同型 `xorshift64` 手写副本 **8 份**，
//! 五处主要 bench 里 **3 处完全不交错**（热漂移 ±25% 会把 32% 读成 13%）、
//! **2 处只报单次或总均值**（没有 min/median 概念）。本 crate 只解决这三件事，
//! 不做调度、不做统计检验、不碰被测代码。

#![forbid(unsafe_code)]

use std::hint::black_box;
use std::time::Instant;

/// 确定性三值序列生成器（xorshift64，与仓内既有手写副本同型，便于读数对账）。
pub struct SeedRng(u64);

impl SeedRng {
    /// 用给定种子建生成器；种子为 0 会被换成非零（xorshift 的退化点）。
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    /// 下一个 `u64`。
    pub fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    /// 长度为 `len` 的三值向量，取值 `{-1, 0, +1}`。
    #[must_use]
    pub fn trits(&mut self, len: usize) -> Vec<i32> {
        (0..len)
            .map(|_| (self.next_u64() % 3) as i8 as i32 - 1)
            .collect()
    }
}

/// 一个样本集的三分位口径。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spread {
    /// 最小值（纳秒）。
    pub min_ns: f64,
    /// 中位数（纳秒）。
    pub median_ns: f64,
    /// 最大值（纳秒）。
    pub max_ns: f64,
}

impl Spread {
    /// 每个测量单位多少纳秒（用中位数口径）。
    #[must_use]
    pub fn ns_per_unit(&self, units: usize) -> f64 {
        self.median_ns / units as f64
    }

    /// 每个测量单位多少皮秒（用中位数口径）。
    #[must_use]
    pub fn ps_per_unit(&self, units: usize) -> f64 {
        self.ns_per_unit(units) * 1_000.0
    }
}

impl Spread {
    /// 由样本算 min/median/max。空样本返回全零（调用方不该拿它做除法）。
    #[must_use]
    pub fn from_samples(samples: &[f64]) -> Self {
        if samples.is_empty() {
            return Self {
                min_ns: 0.0,
                median_ns: 0.0,
                max_ns: 0.0,
            };
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        Self {
            min_ns: sorted[0],
            median_ns: sorted[sorted.len() / 2],
            max_ns: sorted[sorted.len() - 1],
        }
    }
}

/// 一次交错测量：若干实现轮流跑同样的轮数。
///
/// 每轮从不同位序起步（`round % 实现数`），这样靠后登记的实现不会因为
/// 总跑在"机器已经热了"的位置而系统性吃亏——这是从 `trit-ops/src/bench.rs`
/// 的做法抄来的，比同一顺序反复跑更稳。
pub struct Bench<'a> {
    items: Vec<(String, Box<dyn FnMut() -> u64 + 'a>)>,
    units: usize,
    reps: usize,
}

impl<'a> Bench<'a> {
    /// `units` 是每次计时段处理的量（元素数、MAC 数……），用来换算单位耗时。
    #[must_use]
    pub fn new(units: usize) -> Self {
        Self {
            items: Vec::new(),
            units,
            reps: 1,
        }
    }

    /// 每次计时段内把同一份工作重复 `reps` 次（读数是平均值）。
    ///
    /// 本机 `Instant` 分辨率约 100 ns：量 8192 个三值的点积整个只有 1～2 个 tick，
    /// min/median 会被量化到 100/200 ns 这种台阶上（实测过）。重复到几微秒才有意义。
    #[must_use]
    pub fn reps(mut self, reps: usize) -> Self {
        self.reps = reps.max(1);
        self
    }

    /// 登记一个被测实现。闭包必须返回一个会消费计算的数（`black_box` 兜住）。
    /// 登记一个被测实现。闭包返回一个会消费计算的数（`black_box` 兜住），
    /// 允许直接借用调用方的数据（不必为 `'static` 而 clone 或套 `Arc`）。
    pub fn add(&mut self, label: impl Into<String>, runner: impl FnMut() -> u64 + 'a) {
        self.items.push((label.into(), Box::new(runner)));
    }

    /// 登记的实现名（按登记顺序）。
    #[must_use]
    pub fn labels(&self) -> Vec<&str> {
        self.items.iter().map(|(label, _)| label.as_str()).collect()
    }

    /// 跑 `rounds` 轮（每轮每个实现各一次，轮内顺序轮换），返回按登记顺序对齐的样本。
    pub fn run(&mut self, rounds: usize) -> Vec<Vec<f64>> {
        let count = self.items.len();
        let mut samples = vec![Vec::with_capacity(rounds); count];
        for round in 0..rounds {
            for step in 0..count {
                let index = (round + step) % count;
                let start = Instant::now();
                let mut sink = 0u64;
                for _ in 0..self.reps {
                    sink = sink.wrapping_add(self.items[index].1());
                }
                let elapsed = start.elapsed();
                black_box(sink);
                samples[index].push(elapsed.as_nanos() as f64 / self.reps as f64);
            }
        }
        samples
    }

    /// `run` 之后直接换算成 [`Spread`] 与单位耗时。
    pub fn measure(&mut self, rounds: usize) -> Vec<(String, Spread)> {
        let labels: Vec<String> = self.items.iter().map(|(label, _)| label.clone()).collect();
        self.run(rounds)
            .into_iter()
            .zip(labels)
            .map(|(sample, label)| (label, Spread::from_samples(&sample)))
            .collect()
    }

    /// 每单位的纳秒换算（配合 [`Bench::measure`] 的读数打印）。
    #[must_use]
    pub fn units(&self) -> usize {
        self.units
    }

    /// 一行可读读数：`label  min/med/max ns  = X ps/单位`。
    #[must_use]
    pub fn format(spread: &Spread, units: usize) -> String {
        format!(
            "min={:.1} med={:.1} max={:.1} ns ⇒ {:.3} ps/单位",
            spread.min_ns,
            spread.median_ns,
            spread.max_ns,
            spread.ps_per_unit(units)
        )
    }
}
