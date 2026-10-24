//! CRC-3 的检错边界：逐重量枚举「改几个 trit」的错误，找出第一个检不出的。
//!
//! GF(3) 上的 CRC 是**线性**码：错误模式能不能被检出只取决于错误本身，与载荷内容无关，
//! 所以这里用全零载荷就能得到完整的检出矩阵。冗余 3 个 trit ⇒ Singleton 上界 `d ≤ 4`；
//! 实测（9 trit 载荷）最小不可检重量是 **3**：1/2 个 trit 的错误全能检出，3 个就未必。
//! 这个工具把漏检的例子找出来当判据，而不是只说「能检错」。

use crate::blob::crc3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Example {
    pub positions: Vec<usize>,
    pub deltas: Vec<i8>,
}

impl Example {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "positions": self.positions, "deltas": self.deltas })
    }

    /// 把错误作用到一份载荷上（`delta` 按 mod 3 加到 trit 上）。
    pub fn apply(&self, data: &mut [i8]) {
        for (position, delta) in self.positions.iter().zip(&self.deltas) {
            data[*position] = add_delta(data[*position], *delta);
        }
    }
}

#[derive(Debug, Clone)]
pub struct WeightRow {
    pub weight: usize,
    pub tested: u64,
    pub undetected: u64,
    pub example: Option<Example>,
}

#[derive(Debug, Clone)]
pub struct Scan {
    pub length: usize,
    pub rows: Vec<WeightRow>,
}

impl Scan {
    /// 第一个「存在不可检错误」的重量（重量从小到大）。
    pub fn min_undetected(&self) -> Option<&WeightRow> {
        self.rows.iter().find(|row| row.undetected > 0)
    }

    /// ≤ `upto` 重量是否全部可检。
    pub fn detects_all_up_to(&self, upto: usize) -> bool {
        self.rows
            .iter()
            .filter(|row| row.weight <= upto)
            .all(|row| row.undetected == 0)
    }
}

fn add_delta(value: i8, delta: i8) -> i8 {
    let digit = (value + 1 + delta).rem_euclid(3);
    digit - 1
}

/// 枚举重量 1..=max_weight 的全部错误模式，统计检出率并记录第一个漏检的例子。
pub fn scan(length: usize, max_weight: usize) -> Scan {
    let zeros = vec![0i8; length];
    let base = crc3(&zeros);
    let mut rows = Vec::new();
    for weight in 1..=max_weight.min(length) {
        let mut row = WeightRow {
            weight,
            tested: 0,
            undetected: 0,
            example: None,
        };
        let mut positions = Vec::with_capacity(weight);
        walk(length, weight, 0, &mut positions, &mut |chosen| {
            for mask in 0..3u64.pow(weight as u32) {
                let mut deltas = Vec::with_capacity(weight);
                let mut rest = mask;
                for _ in 0..weight {
                    deltas.push((rest % 3) as i8);
                    rest /= 3;
                }
                if deltas.iter().all(|delta| *delta == 0) {
                    continue;
                }
                row.tested += 1;
                let example = Example {
                    positions: chosen.to_vec(),
                    deltas: deltas.clone(),
                };
                let mut broken = zeros.clone();
                example.apply(&mut broken);
                if crc3(&broken) == base {
                    row.undetected += 1;
                    if row.example.is_none() {
                        row.example = Some(example);
                    }
                }
            }
        });
        rows.push(row);
    }
    Scan { length, rows }
}

/// 枚举 `weight` 个严格递增的位置。
fn walk(
    length: usize,
    weight: usize,
    start: usize,
    chosen: &mut Vec<usize>,
    visit: &mut impl FnMut(&[usize]),
) {
    if chosen.len() == weight {
        visit(chosen);
        return;
    }
    let need = weight - chosen.len();
    for position in start..=length - need {
        chosen.push(position);
        walk(length, weight, position + 1, chosen, visit);
        chosen.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_detects_every_error_up_to_weight_two() {
        let scan = scan(9, 4);
        assert!(
            scan.detects_all_up_to(2),
            "重量 ≤2 必须全检出：{:?}",
            scan.rows
        );
        let one = &scan.rows[0];
        assert_eq!(one.tested, 9 * 2, "重量 1：9 个位置 × 2 种非零 delta");
        assert_eq!(one.undetected, 0);
        let two = &scan.rows[1];
        assert_eq!(two.tested, 36 * 8, "重量 2：36 个位置组合 × 8 种 delta");
        assert_eq!(two.undetected, 0);
    }

    #[test]
    fn minimum_undetected_weight_is_reachable_and_verified() {
        let scan = scan(9, 5);
        let row = scan
            .min_undetected()
            .expect("冗余 3 个 trit ⇒ 必有重量 ≤4 的不可检错误");
        assert_eq!(row.weight, 3, "实测最小不可检重量（Singleton 上界是 4）");
        assert!(row.undetected > 0, "重量 3 存在漏检：{row:?}");
        let example = row.example.clone().expect("要给出实例");
        let zeros = vec![0i8; 9];
        let mut broken = zeros.clone();
        example.apply(&mut broken);
        assert_eq!(crc3(&broken), crc3(&zeros), "实例必须真的漏检");
        assert!(
            broken.iter().filter(|value| **value != 0).count() == 3,
            "实例重量必须是 3：{broken:?}"
        );
        // 同样的错误作用到**真实载荷**上一样漏检（线性码的性质）。
        let payload: Vec<i8> = vec![-1, 0, 1, 0, -1, 1, 1, 0, -1];
        let mut tampered = payload.clone();
        example.apply(&mut tampered);
        assert_eq!(crc3(&tampered), crc3(&payload));
    }

    #[test]
    fn zero_delta_is_not_counted_as_an_error() {
        let scan = scan(4, 1);
        assert_eq!(scan.rows[0].tested, 8, "4 个位置 × 2 种非零 delta");
        assert_eq!(scan.rows[0].undetected, 0);
    }
}
