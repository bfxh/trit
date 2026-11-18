# wsp-internal-logic — M0

独立 workspace，不进主 workspace、不进 CI（同 `dot_variants` / `wsp-no-transformer`）。

目的：把 `docs/research/WSP-INTERNAL-LOGIC.md` 的路线 1 跑通一条最小链——

```
三值前馈网络 ──穷举判决──> 前缀树自动机 ──Moore 最小化──> 最小 DFA
                                                          │
参考语言  ──前缀树──> ──Moore 最小化──> 参考最小 DFA ──────┴──> 逐串对拍
```

## 判据

- `diff net_vs_dfa = 0`：提取的 DFA 与原网络在声明域上逐串相等；
- `diff dfa_vs_ref = 0`：与参考语言的最小 DFA 逐串相等；
- `states(net) == states(ref)`：状态数一致（Moore 最小化给出最小）。

任何一条不满足即 `FAIL` 并以退出码 1 结束。

## 当前案例

`at_least_two_ones`：串长 `0..=3`，语言「至少 2 个 1」。网络吃 2 维/位置的编码
（3 位 ⇒ 6 维输入）：

- 层 1：`is_one` 每位复制两份（6 个隐藏单元），再加 3 个常量单元（bias `+1`）；
- 层 2：前 6 项 `+1`、后 3 项 `−1` ⇒ `sign(2·#1 − 3)`。

`#1=2 → 1 → Pos`、`#1=1 → −1 → Neg`、`#1=0 → −3 → Neg`、`#1=3 → 3 → Pos`。

## 读数

```text
case=at_least_two_ones max_len=3 words=15 accepted=5
  dfa states: net=7 ref=7
  diff: net_vs_dfa=0 dfa_vs_ref=0
  PASS
```

## 为什么是 7 个状态，不是 3

「至少 2 个 1」在全域（任意长度）上的经典最小 DFA 是 3 状态。这里 7 是**相对声明域**
（长度 ≤ 3）的最小：前缀的「剩余可扩展长度」参与了未来行为，导致等价类变多；前缀树末尾
还有一个越界 `DEAD` 自环。

这正是 `WSP-INTERNAL-LOGIC.md` §1 要求「域必须写进报告」的原因——**最小性是相对于域的**，
域一变状态数就变。域外不主张任何等价性。

## 跑

```bash
cargo run --release
cargo test --release
```

## 未做（后续里程碑）

- 3-bit 加法器、括号匹配、Tomita #3；
- 轨迹提取（`pre_activations` → 状态命名）与穷尽干预；
- Tracr 反向闭环（程序 → 权重 → 程序 的恢复率）；
- 状态爆炸预算（等价类数上限与负结果入册）。
