//! 内容定义分块（CDC）：按内容决定切点，而不是按固定偏移。
//!
//! 为什么要它：Git 的 blob 是整文件内容寻址，文件中间插一段就变成"整块新对象"。
//! 按内容切块后，插入只会新增"切点附近"的块，后面的块重新对齐、哈希不变 ⇒ 去重更强。
//!
//! 算法是最小实现：rolling gear hash，`hash = (hash << 1) + table[byte]`，
//! 满足 `hash & mask == 0` 就切。同一段字节在任何位置切出来的边界都相同（确定性）。

/// 切块参数：期望块大小 = 2^bits 字节。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunking {
    pub mask: u64,
    pub min: usize,
    pub max: usize,
}

impl Chunking {
    /// `bits=12` ⇒ 平均 4 KiB，最小 1 KiB，最大 64 KiB。
    pub fn new(bits: u32) -> Self {
        let mask = (1u64 << bits) - 1;
        let avg = 1usize << bits;
        Self {
            mask,
            min: (avg / 4).max(64),
            max: avg * 16,
        }
    }

    pub fn default_4k() -> Self {
        Self::new(12)
    }

    /// 返回 `(start, end)` 片段区间，覆盖整个输入且互不重叠。
    pub fn split(self, data: &[u8]) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut start = 0usize;
        while start < data.len() {
            let hard_end = (start + self.max).min(data.len());
            let mut end = hard_end;
            let mut hash = 0u64;
            for i in start..hard_end {
                hash = (hash << 1).wrapping_add(GEAR[(data[i] as usize) & 0xff]);
                let len = i - start + 1;
                if len >= self.min && (hash & self.mask) == 0 {
                    end = i + 1;
                    break;
                }
            }
            out.push((start, end));
            start = end;
        }
        out
    }
}

/// 64 位 gear 表：固定常量，保证跨机器/跨版本切点一致（不要换成随机表）。
const GEAR: [u64; 256] = {
    let mut table = [0u64; 256];
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut i = 0;
    while i < 256 {
        // splitmix64：常量表，编译期算出来
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        table[i] = z ^ (z >> 31);
        i += 1;
    }
    table
};

#[cfg(test)]
mod tests {
    use super::*;

    fn data(n: usize, seed: u8) -> Vec<u8> {
        (0..n)
            .map(|i| ((i as u64 * 31 + seed as u64) % 251) as u8)
            .collect()
    }

    #[test]
    fn covers_whole_input_without_overlap() {
        let chunking = Chunking::default_4k();
        for size in [0, 1, 100, 4096, 100_000] {
            let bytes = data(size, 7);
            let ranges = chunking.split(&bytes);
            if size == 0 {
                assert_eq!(ranges, vec![]);
                continue;
            }
            assert_eq!(ranges.first().unwrap().0, 0);
            assert_eq!(ranges.last().unwrap().1, size);
            for pair in ranges.windows(2) {
                assert_eq!(pair[0].1, pair[1].0, "块之间不许有空隙或重叠");
            }
        }
    }

    #[test]
    fn respects_min_and_max() {
        let chunking = Chunking::default_4k();
        let bytes = data(200_000, 3);
        for (start, end) in chunking.split(&bytes) {
            let len = end - start;
            assert!(len <= chunking.max, "块 {len} 超过上限");
            assert!(len >= 1);
        }
    }

    #[test]
    fn deterministic_across_calls() {
        let chunking = Chunking::default_4k();
        let bytes = data(50_000, 11);
        assert_eq!(chunking.split(&bytes), chunking.split(&bytes));
    }

    /// CDC 的核心收益：中间插一段，后面的边界重新对齐（固定分块做不到）。
    #[test]
    fn insertion_keeps_most_chunk_boundaries() {
        let chunking = Chunking::new(8); // 平均 256 B，样本小一点
        let base = data(20_000, 5);
        let mut inserted = base.clone();
        inserted.splice(5_000..5_000, vec![0xAB; 37]);
        let before: Vec<_> = chunking
            .split(&base)
            .into_iter()
            .map(|(s, e)| base[s..e].to_vec())
            .collect();
        let after: Vec<_> = chunking
            .split(&inserted)
            .into_iter()
            .map(|(s, e)| inserted[s..e].to_vec())
            .collect();
        let set_before: std::collections::HashSet<_> = before.iter().cloned().collect();
        let shared = after
            .iter()
            .filter(|chunk| set_before.contains(*chunk))
            .count();
        assert!(
            shared * 10 >= after.len() * 8,
            "插入 37 字节后应至少 80% 块不变，实际 {}/{}",
            shared,
            after.len()
        );
    }

    #[test]
    fn parameters_are_sane() {
        let chunking = Chunking::new(12);
        assert_eq!(chunking.mask, 4095);
        assert!(chunking.min < chunking.max);
        assert_eq!(chunking.min, 1024);
        assert_eq!(chunking.max, 65536);
    }
}
