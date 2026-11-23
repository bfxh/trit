//! CLI 用法串。单独成文件是为了让 `shared.rs` 不再随子命令说明增长。

use super::die;

pub(crate) fn usage() -> ! {
    die("用法: trit-data gate --jsonl <轨迹.jsonl> [--out <目录>] [--allow-unverified] [--lang-quality off|defer|strict]
      trit-data pipeline --trajectories <轨迹.jsonl|目录> [--cmd <重写器命令>] [--rewrite-timeout 30] [--mix-total N] [--mix-weights code=0.7,writing=0.3] [--out <目录>] [gate 参数…]
      trit-data rewrite --jsonl <defer.jsonl> --cmd <重写器命令> [--timeout 30] [--out <目录>] [gate 参数…]
      trit-data compare --a <a.jsonl> --b <b.jsonl>；decontaminate --jsonl <in> --bench <bench.jsonl> [--ngram 13] [--threshold 0.2] [--out <目录>]；mix --jsonl <in> [--total 1000] [--weights code=0.7,writing=0.3] [--out <目录>]；label --jsonl <in> --sample 500 [--out <目录>] / label --apply <label.csv> --out <目录>")
}
