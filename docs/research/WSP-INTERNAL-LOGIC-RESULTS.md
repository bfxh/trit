# WSP 内部逻辑读取：M0 读数

> 状态：**已跑通**（M0，`benchmarks/wsp-internal-logic`）。设计见 [WSP-INTERNAL-LOGIC.md](WSP-INTERNAL-LOGIC.md)。
> 域声明：`max_len = 3`、字母表 `{0,1}`、词表 15 条。**域外不主张任何等价性。**

## 1. 一句话

三值前馈网络 → 穷举判决 → 前缀树 → Moore 最小化 → 最小 DFA，与参考语言的最小 DFA
逐串对拍：`diff = 0 / 0`，状态数两边都是 7。

## 2. 案例

`at_least_two_ones`：语言「串里至少 2 个 1」，长度 `0..=3`。

网络吃 2 维/位置的编码（3 位 ⇒ 6 维输入；`0 → (Zero, Neg)`、`1 → (Pos, Neg)`、
填充 → `(Zero, Pos)`）：

- 层 1：`is_one` 每位复制两份（6 个单元）+ 3 个常量单元（bias `+1`）；
- 层 2：前 6 项 `+1`、后 3 项 `−1` ⇒ `sign(2·#1 − 3)`。

填充位的 `is_one = Zero` 不贡献，所以判决与串长无关。

## 3. 读数

```text
case=at_least_two_ones max_len=3 words=15 accepted=5
  dfa states: net=7 ref=7
  diff: net_vs_dfa=0 dfa_vs_ref=0
  PASS
cargo test --release → 4 passed
```

行为表（15 条）：

| 串 | #1 | 判决 |
|---|---|---|
| （空） | 0 | 拒绝 |
| `0`、`1` | 0、1 | 拒绝 |
| `00`、`01`、`10` | 0、1、1 | 拒绝 |
| `11` | 2 | 接受 |
| `000`、`001`、`010`、`100` | 0、1、1、1 | 拒绝 |
| `011`、`101`、`110` | 2、2、2 | 接受 |
| `111` | 3 | 接受 |

接受共 5 条，与程序读数一致。

## 4. 为什么是 7 个状态，不是 3

「至少 2 个 1」在**全域**（任意长度）上的经典最小 DFA 是 3 状态。这里 7 是**相对声明域**
（长度 ≤ 3）的最小：

- 前缀的「剩余可扩展长度」参与未来行为，不同长度的前缀不再等价；
- 前缀树尾部还有一个越界 `DEAD` 自环。

这正是设计文档 §1 要求「域必须写进报告」的实证：**最小性相对于域**，域一变状态数就变。

## 5. 判据对照

| 判据（设计文档 §6） | 结果 |
|---|---|
| 提取 DFA 与网络逐串相等（`DIFF=0`） | 0 |
| 提取 DFA 与参考语言逐串相等 | 0 |
| 状态数一致（最小性） | 7 = 7 |
| Moore 最小化幂等 | 通过 |
| 参考 DFA 与语言定义逐串一致 | 通过 |

## 6. 复算

```bash
cd benchmarks/wsp-internal-logic
cargo run --release
cargo test --release
```


## 8. M0d：轨迹层读数（后加）

`--trace` 打印逐串轨迹（层 1 激活、输出层 pre-activation、输出、DFA 状态）。摘要：

```text
trace: hidden_classes=8 hidden_to_out_conflicts=0 hidden_to_state_conflicts=4 max_variants=4
```

| 读数 | 值 | 含义 |
|---|---|---|
| `hidden_classes` | 8 | 15 个串只产生 8 种不同的层 1 激活 |
| `hidden_to_out_conflicts` | **0** | 同一层 1 激活一定给同一输出 ⇒ 层 1 是信息瓶颈（causal sufficiency） |
| `hidden_to_state_conflicts` | 4 | 4 种层 1 激活各自对应多个 DFA 状态 |
| `max_variants` | 4 | 同一 DFA 状态最多对应 4 种层 1 激活 |

**第一个 0 是判据（不成立就是 FAIL）；第二个 4 是发现：**

- 层 1 只取 `is_one` 位，不编码「串长 / 结束」；而域内最小 DFA 的状态含「剩余可扩展长度」。
  例：`hidden=[0,0,0,0,0,0,1,1,1]` 同时对应 `""`（q0）、`"0"`（q1）、`"00"`/`"000"`（q3）。
- 所以**「提取的 DFA 状态」不等于「网络的内部状态」**：前者有一部分来自域截断，不是网络结构。
- 网络的真实内部状态是层 1 激活（域内 8 类），它决定输出，但不决定「还剩几位可扩展」。

这条负结果修正了一个读法错误：要读思考链，应该读 `hidden` 轨迹；DFA 状态是**行为等价类**，
不是内部状态。把两者当成一回事，就会把「域的人为截断」误读成「模型在数长度」。

### 8.1 一个具体的错读例子

若只报 DFA 状态，会看到 `""`→q0、`"0"`→q1、`"00"`→q3、`"000"`→q3，很容易得出
「网络在跟踪长度」的结论。但看 `hidden` 就知道：这四个串的层 1 激活**完全相同**，
网络根本没在数长度——长度信息只存在于我们的域声明里。


## 9. M0e：穷尽干预（承重矩阵）

`--intervene` 对层 1 的 9 个单元逐个做「清零 / 取反」，域内 15 个串全部穷尽，不抽样：

```text
intervene: units=9 live=9 words=15
  unit 0: flips_zero=3 flips_neg=3
  unit 1: flips_zero=3 flips_neg=3
  unit 2: flips_zero=3 flips_neg=3
  unit 3: flips_zero=3 flips_neg=3
  unit 4: flips_zero=2 flips_neg=2
  unit 5: flips_zero=2 flips_neg=2
  unit 6: flips_zero=6 flips_neg=6
  unit 7: flips_zero=6 flips_neg=6
  unit 8: flips_zero=6 flips_neg=6
```

| 单元 | 角色 | 翻转数（清零 / 取反） |
|---|---|---|
| 0、1 | pos0 的 `is_one`（复制两份） | 3 / 3 |
| 2、3 | pos1 的 `is_one` | 3 / 3 |
| 4、5 | pos2 的 `is_one` | 2 / 2 |
| 6、7、8 | 常量 `Pos`（提供 `−3` 阈值） | 6 / 6 |

三条读数：

1. **9/9 承重**：没有装饰单元，每个层 1 单元至少在一个串上改变输出。
2. **常量单元承重最高**（6/15）：它们是阈值的来源，改一个就移动判决边界——
   这也是三值网络里「偏置」不能当免费参数删掉的原因。
3. **位置不对称**（3 vs 2）：pos0/pos1 的 `is_one` 在更多串上可能取 1，而 pos2 只在长度 3
   时才可能。承重率反映的是**编码结构**，不是权重大小。

判据：`live_units() > 0` 已进测试（`intervene_has_live_units`）。本案例实际 9/9，
若将来某个案例出现 `live < units`，那说明存在装饰单元，需要解释而不是忽略。

## 10. 复算（含轨迹与干预）

```bash
cd benchmarks/wsp-internal-logic
cargo run --release -- --trace
cargo run --release -- --intervene
```

## 7. 未证栏

- 只有**一个案例、一个域**（长度 ≤ 3）。3-bit 加法器、括号匹配、Tomita #3 都未做。
- 7 个状态里哪些是「语言状态」、哪些是「域截断造成的」，目前只靠人工对照，没有程序化标注。
- 轨迹层（§8）与穷尽干预（§9）都已做；但**状态命名**仍是人工对照，没有程序化判据。
- Tracr 反向闭环**未做**，「恢复率 100%」未测。
- 状态爆炸预算未设：域变长会让等价类数快速增长，上界未测。
- 「最小 DFA 唯一」不等于「人类可读」：状态命名仍是人工步骤，没有判据。
- `cargo test` 只覆盖 4 条不变量，不构成对提取器在大规模上的正确性证明。
