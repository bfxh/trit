//! 多文档并行格式化：按文档分块，结果按原顺序归位。
//!
//! 为什么按文档并行：`format_json` 是纯函数，文档之间没有共享状态；
//! 单文档内部是顺序解析 + 打印，拆不开。基准见 `benches/threads.rs`。

use crate::{FormatError, format_json};

/// 并行格式化一批文档；结果顺序与输入**一致**（不因线程调度改变）。
///
/// `jobs` 是期望并行度：0 或 1 退化为串行；超过文档数时按文档数封顶。
/// 分块按**字节**均衡（不是按文档数）：语料里可能有 84 KB 的大文档，
/// 按条数分会把大文档全压到一个线程上，实测 4 线程只有 2.83×。
pub fn format_many(docs: &[&str], jobs: usize) -> Vec<Result<String, FormatError>> {
    if jobs <= 1 || docs.len() < 2 {
        return docs.iter().map(|doc| format_json(doc)).collect();
    }
    let jobs = jobs.min(docs.len());
    let mut slots: Vec<Option<Result<String, FormatError>>> =
        (0..docs.len()).map(|_| None).collect();
    let chunks = split_by_bytes(docs, jobs);
    std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .iter()
            .map(|(base, range)| {
                let base = *base;
                let slice = &docs[range.clone()];
                scope.spawn(move || {
                    slice
                        .iter()
                        .enumerate()
                        .map(|(offset, doc)| (base + offset, format_json(doc)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            for (index, result) in handle.join().expect("batch worker panicked") {
                slots[index] = Some(result);
            }
        }
    });
    slots
        .into_iter()
        .map(|slot| slot.expect("每个文档都应该有结果"))
        .collect()
}

/// 把文档切成 `jobs` 块，使每块**字节数**尽量接近（返回 `(起始下标, 下标区间)`）。
fn split_by_bytes(docs: &[&str], jobs: usize) -> Vec<(usize, std::ops::Range<usize>)> {
    let total: usize = docs.iter().map(|doc| doc.len()).sum();
    let target = total.div_ceil(jobs).max(1);
    let mut chunks = Vec::with_capacity(jobs);
    let mut start = 0usize;
    let mut acc = 0usize;
    for (index, doc) in docs.iter().enumerate() {
        acc += doc.len();
        let remaining_jobs = jobs.saturating_sub(chunks.len() + 1);
        let remaining_docs = docs.len() - index - 1;
        // 只有"还有线程要分、且剩余文档够分"时才切，避免末尾切出空块。
        if acc >= target && chunks.len() + 1 < jobs && remaining_docs >= remaining_jobs {
            chunks.push((start, start..index + 1));
            start = index + 1;
            acc = 0;
        }
    }
    if start < docs.len() {
        chunks.push((start, start..docs.len()));
    }
    chunks
}

/// 进程可用的并行度（`available_parallelism`，取不到时退回 1）。
pub fn available_jobs() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_fallback_matches_parallel() {
        let docs = [r#"{"a":1}"#, r#"[1,2,3]"#, r#"{"b":{"c":null}}"#];
        let serial = format_many(&docs, 0);
        for jobs in [1, 2, 3, 8] {
            let parallel = format_many(&docs, jobs);
            assert_eq!(parallel.len(), serial.len());
            for (left, right) in parallel.iter().zip(&serial) {
                assert_eq!(left.as_ref().ok(), right.as_ref().ok(), "jobs={jobs}");
            }
        }
    }

    #[test]
    fn errors_keep_their_position() {
        let docs = [r#"{"ok":1}"#, "{bad", r#"{"ok":2}"#];
        let out = format_many(&docs, 3);
        assert!(out[0].is_ok());
        assert!(out[1].is_err());
        assert!(out[2].is_ok());
    }
}
