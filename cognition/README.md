# cognition/ —— 未来编程模型的独立训练层

**定位**：这个文件夹是为**用户未来的编程模型**准备的训练/研究层——语料契约
（SCHEMA.md）、收集器（collect.py）、知识快照（knowledge/）。它默认不依赖本仓的
任何东西：

- 不 import `tools/` 的任何代码，不依赖任何 trit crate，无网络；
- collect.py 只用 Python 3 标准库，直接 `python collect.py` 就能跑；
- 对 tools/ 的唯一依赖是**数据形状契约**（几份 JSONL 的字段），把它写成文档
  （SCHEMA.md），不是代码依赖；
- **整体可搬**：整个文件夹将来直接 mv 到模型仓库，不拖泥带水（见下文"搬去模型
  仓库"）。

## 目录结构

```
cognition/
  README.md      本文件：定位、隐私规则、搬家说明
  SCHEMA.md      四份数据契约逐字段写清（signals / watch / pitfalls / corpus 投影）
  collect.py     收集器：读 owner root 的 .agent/ store，产出 corpus/ 与 knowledge/
  .gitignore     只有一条 corpus/——随文件夹走，搬去新仓库后私有语料照样被忽略
  corpus/        （collect 运行后出现；gitignore）交互语料的脱敏投影，私有
  knowledge/     （collect 运行后出现；入库）pitfalls 痛点语料快照，公开知识
```

## 数据流

```
<owner-root>/.agent/
  signals.jsonl   ─┐
  watch.jsonl     ─┼─> collect.py --root <owner-root> ─┬─> corpus/events.jsonl   （私有，gitignore）
  pitfalls.jsonl  ─┘                                   └─> knowledge/pitfalls.jsonl（公开，入库）
```

- `--root` 是 **owner root**（`.agent/` 所在的目录），每次运行时给出，工具里没有写死
  任何路径；`--out` 可把两个产物指到别处，默认落在 collect.py 自身旁边（跟着文件夹走）。
- 某个 store 不存在 → 跳过并在摘要里注明，exit 0——**骨架状态可收集，没有数据不是错误**。
- 坏行 fail-closed：任何一家 store 有坏行，整个收集拒绝并指出行号，**先于任何写盘**。
- 摘要是纯 ASCII 单行，例如：
  `collect: corpus=12 events knowledge=10 pitfalls (skipped: watch absent)`

## 隐私规则

1. **corpus/ 是私有语料，不出克隆。** 交互信号（分诊/惩罚投影、行为事件投影）整目录
   gitignore，克隆下来没有它。内容是结构化投影（无原文），但整体按私有对待。
2. **默认不含原文，也没有 --with-text 这个后门。** corpus 里没有 `text` / `task_hints` /
   `reason` / `note`——抽出来的原文片段也是原文。训练层要原文时，用户自己手动交付，
   工具不代劳。
3. **signals 原始台账（含原文）永远留在 owner root**，collect 只读不写；owner 仓自己
   已经把它 gitignore。
4. **knowledge/ 是公开知识，随仓提交。** pitfalls 痛点语料生来就是要教下一个模型的；
   collect 在快照门口再过一道用户路径隐私闸（写入方 add 拦一道，collect 是入库前的
   最后一道——store 是手改得了的）。
5. **坏行拒绝。** 手改坏、写崩、截断的 store 都拒绝产出语料并报行号——不允许"半读
   半猜"的语料进训练层。

## 搬去模型仓库

整个文件夹 `mv` 到模型仓库即可，**什么都不用改**：

- collect.py 没有硬编码路径——owner root 是运行参数（`--root`），搬家后换成模型仓库
  的训练工作目录，改的是命令行参数的值，不是代码；
- 输出落点默认相对 collect.py 自身，`corpus/` 与 `knowledge/` 跟着文件夹走；
- `cognition/.gitignore` 里的 `corpus/` 随文件夹走，新仓库里私有语料照样被忽略；
- 唯一如实说明的残留：本仓根 .gitignore 里有一行 `cognition/corpus/`，是给"文件夹还
  住在本仓"的阶段用的；搬走后那行在新仓库不存在也无妨（文件夹自带的 .gitignore 接管），
  本仓这行留着无害——不是需要清理的依赖。

## 边界（honest boundary）

collect 是**快照不是账本**：重跑覆盖产物，store 不变则字节不变；store 缺席时不写空
文件冒充数据。它只搬运与投影，不做任何推断——分诊分数是信号不是事实（写入方自己的
声明），行为事件是行为证据不是读心。本层对"模型该怎么训练"不做任何主张：契约收窄
成数据形状，训练怎么做是模型仓库的事。
