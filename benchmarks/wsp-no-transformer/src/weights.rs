//! 从论文 `build.py` 导出的 npz 读取权重。

use std::path::Path;

use crate::npz::Npz;

pub struct Weights {
    /// 词 id → 概念 id（0 表示无概念）。
    pub concept: Vec<i64>,
    /// 每个概念的 idf。
    pub idf: Vec<f32>,
    /// 一阶关联矩阵 (C, C)，行主序。
    pub a: Vec<f32>,
    /// 二阶表 (H, R)：每行 R 个被回忆的概念。
    pub row_ids: Vec<i64>,
    /// 二阶表 (H, R)：对应权重。
    pub row_w: Vec<f32>,
    pub h: usize,
    pub r: usize,
    pub c: usize,
    pub mix: f32,
}

pub fn read_i64(npz: &mut Npz, name: &str) -> Vec<i64> {
    npz.by_name(name)
        .unwrap_or_else(|e| panic!("读 {name} 失败：{e}"))
        .unwrap_or_else(|| panic!("{name} 不存在"))
        .data::<i64>()
        .unwrap_or_else(|e| panic!("解析 {name} 失败：{e}"))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|e| panic!("读 {name} 数据失败：{e}"))
}

fn read_f32(npz: &mut Npz, name: &str) -> Vec<f32> {
    npz.by_name(name)
        .unwrap_or_else(|e| panic!("读 {name} 失败：{e}"))
        .unwrap_or_else(|| panic!("{name} 不存在"))
        .data::<f32>()
        .unwrap_or_else(|e| panic!("解析 {name} 失败：{e}"))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|e| panic!("读 {name} 数据失败：{e}"))
}

pub fn load_weights(path: &Path) -> Weights {
    let mut npz =
        Npz::open(path).unwrap_or_else(|e| panic!("打开 {} 失败：{e}", path.display()));
    let concept = read_i64(&mut npz, "concept");
    let idf = read_f32(&mut npz, "idf");
    let a = read_f32(&mut npz, "A");
    let row_ids = read_i64(&mut npz, "row_ids");
    let row_w = read_f32(&mut npz, "row_w");
    let mix = read_f32(&mut npz, "mix")[0];
    let c = idf.len();
    assert_eq!(a.len(), c * c, "A 形状与 idf 不一致");
    assert_eq!(row_ids.len(), row_w.len(), "二阶表长度不一致");
    // npy 里 row_ids 是 (H, R)；npz 头不在这里解析，R 从 build.py 的常量推断：
    // gen5 的 mem_assoc2 是 16，第一代是 40。用 H = len / R 反推需要先知道 R，
    // 所以让调用方通过环境变量覆盖，默认 16。
    let r = std::env::var("WSP_R")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16usize);
    assert_eq!(row_ids.len() % r, 0, "二阶表长度不是 R 的倍数");
    let h = row_ids.len() / r;
    Weights {
        concept,
        idf,
        a,
        row_ids,
        row_w,
        h,
        r,
        c,
        mix,
    }
}
