# Changelog

## [Unreleased]

### Added（三进制构建工具 M0：`trit build`）

- `trit-build <src.tri> [--out DIR] [--algebra k3] [--force] [--check]`（独立实验 crate）：解析 → `simplify` → **穷举复核**
  （不等价即构建失败、不写产物）→ 产物 `expr.txt` / `truth.csv` / `truth.tritbin` / `lookup.c`
  + `build.json`；内容哈希增量缓存，`--check` 校验产物完整性。
- `TRB1` 容器：魔数 + 版本 + 语义 + 变量数 + 长度 + **双导轨位平面**（2 bit/trit）+ FNV-64 尾；
  基 3 打包/GF(3) CRC-3 因发布图门禁（实验 crate 不许互依）留到 M2。
- `lookup.c` 是刻意边界：三值语义固化为查表，执行交给 cc/rustc（不重写二进制后端）。
- 测试 +4（产物/缓存命中/篡改检测/重建不命中缓存）；变异门 251 → **253**（trit-build 3/3）。
- 文档 `docs/TERNARY-TOOLCHAIN.md`：论文依据（LLVM/MLIR/egg/BitNet/平衡三进制，DOI 已核）、
  分层设计、M0–M3 验收判据、继续靠 LLVM/Rust 的职责表、未验证清单。

### Added（GGUF → MOD 入向导入）

- `trit-texture::gguf::import_gguf`：读 **GGUF v2/v3**，把未量化张量（`F32`/`F16`/`BF16`）
  按 `v > ε → +1 / v < -ε → -1 / 其余 0` 三值化，组装成 `TritModel`（MOD 语义）；
  `*.weight`（2D）+ 同名 `*.bias`（可选，缺省 0）按名排序成层，层间衔接复用 `TritModel::new`。
- CLI：`trit-texture` 新增 bin `gguf-to-mod`（`--threshold` / `--tensor` / `--max-bytes`），
  输出 TTX2 模型容器。
- 硬边界：只支持未量化类型；`TQ1_0`/`TQ2_0`/`Q*_K` 等**显式拒绝**（位布局未在本仓验证，
  猜错会静默产出错误权重）；预算：文件 256 MiB / 权重 5000 万 / KV 10 万 / 张量 10 万。
- 测试 +8（GGUF 头/元数据/张量校验、F16/BF16 解码、阈值稀疏、NaN/截断/超预算、
  层链不匹配、MOD 往返）；实测 232 字节 GGUF → 41 字节 MOD（2 层 8 权重）。
  变异门 244 → **248**。

### Fixed（复审 P3：双导轨封装 / BLIF 导出校验 / 解析方向结构门）

- **L01 `trit-dot::TernaryBits` 封装**：字段改私有，新增 `TernaryBits::new(pos, neg, len)`
  校验「平面长度 = ⌈len/64⌉、正负导轨不重叠、尾部填充位为 0」；`dot`/`dot_portable`/`dot_naive`/
  `dot_sparse` 与 `dot_words*` 统一返回 `Result<i64, DotError>`，长度不一致不再 panic 或静默截断。
  旧路径 `dot` 会索引越界 panic、`dot_portable` 按最短平面静默少算。
- **L03 BLIF 导出校验**：`to_blif` 先跑 `circuit::validate`（元数/重复驱动/悬空/环），再检查
  标识符（空、含空白、`.` 开头逐项拒绝）；新增 `BlifError::Structure` / `BlifError::Name`。
- **同批修正（导出/解析对称）**：`parse_blif` / `parse_netlist` 返回前同样过 `validate`，
  不再返回"能解析但导不回去"的电路；新增 `NetlistError::Structure`；fuzz 往返测试改为
  「基线可解析 + 变异样本只要求不 panic、接受即可往返」。
- 测试 +4（workspace **734/0**）；`cargo check -p trit-gpu --features gpu` 通过（编译门，
  无硬件执行）。变异门 240 → **247**（本批 7 条：双导轨三处、BLIF 导出两处、双向解析各一）。

### Fixed（复审 P2 批次：AIGER 头 / 逻辑运算 / 平衡三进制精度 / 编码校验 / 训练与 SNN / 解析预算）

- **M01 AIGER ASCII 头**：解析只接受 `aag`（标准 ASCII），`aig`（二进制）返回
  `AigerError::BinaryFormat`；导出改为写 `aag`，CLI fixture 更名 `and2.aag`。
  之前标准 ASCII 文件会被拒、二进制头被当文本硬读。
- **M02 逻辑 vs 逐位**：`&&` / `||` 不再被静默当成逐位 `&` / `|`，返回
  `VerilogErrorKind::Unsupported`（逐位运算保留）；向量逻辑表达式不再产出错误位宽。
- **M06 平衡三进制 `range`**：`checked_pow` + 饱和（#113 已修），本批补回归记录。
- **M07 浮点精度**：`digit_count` 改按**平衡三进制位数**（`(3^n-1)/2` 上界），
  舍入改为「相邻指数两个候选、各自 clamp 到合法尾数、取误差小者」——
  `5` 在 `p=1` 时得到 `3`（旧实现给 `9`）；补 `±2/±5/±8` 边界与穷举最近性测试。
- **M08 模型编码**：`format::encode` 改为 `Result`，先 `Model::validate`，再查
  `u16` 范围与乘积；非法模型/权重/空模型都拒绝，不再 `as u16` 截断或把 `2` 静默写成 `+1`。
- **M09 训练输入**：`learning_rate` 必须有限且 > 0、`epochs ≥ 1`，逐轮 `loss` 非有限立即报错，
  不再返回 NaN 报告或改写损坏模型。
- **M10 SNN**：新增 `Network::validate`（方阵/权重 `{-1,0,1}`/阈值/decay/规模），
  `simulate`/`simulate_with_drive` 改为 `Result`，膜电位用 `i64` 中间量并限制 tick 预算。
- **M11 解析预算**：`parse_netlist` / `parse_blif` 增加字节/网络/门数（BLIF 另有 cover 行）
  预算与 `*_with_limits` 入口，默认 1 MiB / 10 万 / 10 万 / 20 万，分配前拒绝。
- 测试 +9（workspace **730/0**）；变异门 228 → **240**（本批 12 条新锚点 + 3 条随迁）。
  兼容性：`aiger` 文本必须是 `aag`；`trit-net`/`trit-machines` 的 encode/simulate 变 fallible
  （两者都是 `publish = false` 实验层，不进稳定 API）。

### Fixed（复审 P0/P1 批次 4：sparse 视图越界 H08 + P1 五项）

- **H08 sparse `View::new` 越界/语义分叉**（`trit-codec/src/plan.rs`）：构造期完整验证
  下标严格递增、域内、符号区未用填充位为 0。修复前 `len=1,index=1` 让 `copy_into`
  在 `out[index]` 越界 panic；`[2,0]` 让 `get`（二分）与 `copy_into`（顺序写）给出
  不同向量。三条读取路径现在同语义。
- **H09 trit-net API 边界**（`infer.rs`/`train.rs`）：`forward`/`analyze`/`train` 入口
  先 `Model::validate()`；训练**逐条**校验样本维数与非有限值（此前只查第一条，第二条
  短样本在 `Shadow::forward` 索引时 panic）。手搓非法模型返回 `ModelError` /
  `TrainError::Model`，不再靠索引 panic。
- **H11 SDF 经典 XOR 映射**：`XOR2`/`XOR3` 此前映射成三值强加 `Xor`，与 Verilog `xor`
  展开的 `XorClassic` 不相等 ⇒ 按实例覆盖静默落空、延迟回退默认 1。改为经典映射后
  复审复现探针（SDF `XOR2` 延迟 100）关键路径 1 → **100**。
- **M03 ATPG 最优性误报**：预算在子调用里耗尽、且还有未试的兄弟分支时置 `exhausted`
  （旧实现只 `break` 不置位）。复审复现集合（full=63、budget=4）不再把 3 个向量冒充
  最优；穷举仍给出 2 且 `proven=true`。
- **M04 检查点索引把锁目录当任务目录**：`index()`/`len()` 只认「偶数长度全十六进制」目录，
  锁目录（`.lock`、隔离目录）与噪声目录自然排除；持锁期间读索引不再 `InvalidState`。
- **M05 锁在进程崩溃后永久残留**：锁目录写 `owner.json`（pid/主机/获取时间，`create_new`
  抢记录，绝不覆盖别人的）；`break_stale_lock` 用 `ps`（unix）/`tasklist`（Windows）探测
  持有进程，只清**确定已死**的锁（探测不可用、pid 无法判定、跨主机记录一律视为占有）；
  清理走「先改名隔离 → 复核 owner」，回滚不覆盖已存在目录；`Drop` 也只删 owner 仍是自己
  的那把。无新依赖、保持 `forbid(unsafe_code)`；已知局限（pid 复用只朝保守方向、命令
  缺失时不恢复、跨主机不自动清理）写进模块文档。
- **顺带封堵三处公开 API 隐患**：空 task id 的十六进制是空串（`task_dir("")` = 版本目录），
  `save/load/remove/prune` 现在一律拒绝，`remove("")` 不再能一次删掉全部任务；`len()` 与
  `index()` 用同一判据（十六进制且可解 UTF-8），不再对「有几个任务」给出不同答案；
  sparse `View::new` 的 `8 + count*4 + …` 改 checked 运算（32 位目标上 count≈2^30 会溢出）。
- 测试 +10；变异门 216 → **228**（新增 12 条：越界下标、填充位、forward/train 校验、
  XOR 映射、最优性、索引判据、陈旧锁回收、无主锁不误杀、Drop 所有权、空 task id）；
  workspace **721/0**；fmt/clippy/模块预算绿。

### Added（TTX 格式转换落地 + GPU 端到端接线——真机测试抓到双轨跨行真 bug）

- **纯转换层 `trit_texture::convert`**（默认构建，零新依赖）：RGBA ↔ TTX1/TTX2、
  TTX1 ↔ TTX2 升降级（降级仅单级 TEX，mip 链显式拒绝）、**盒式 mip 链构造**
  （2×2 滤波、尺寸向下取到块对齐、不能严格递减即停）。
- **PNG 文件 I/O（`png` feature，默认关）+ `ttx-convert` CLI**：png-to-ttx
  （`--v1`/`--mip`）、ttx-to-png、ttx-upgrade、ttx-downgrade；CI 的 check 门补
  `--features png` 的 test+clippy（feature 代码默认构建碰不到，不加门就是死角）。
  **架构如实记**：原计划独立 trit-convert crate 被**发布图门**拦下——实验层之间
  禁止常规路径依赖（政策带自测）；改为可选 feature 收进格式 crate，默认构建
  依旧零图像解码依赖。CLI 真机冒烟：PNG→TTX(mip,96B)→PNG 往返、mip 链降级正确拒绝。
- **GPU 端到端（trit-gpu `forward`，gpu feature）**：`matmul_packed` 预打包入口
  （naive 内核，绑定与 matmul 同一套）——真机测试把 MOD 权重 `to_dual_rail()`
  缓冲直上 GPU，与标量逐元素对拍（inputs=33 跨字边界 + 容器往返重跑一致性）。
  **该测试抓到真 bug**：`to_dual_rail` 曾用平面 `index/32` 定字，`inputs % 32 != 0`
  时第 2 行起点不在字边界（pack_rows 布局是每行整字）——单输出单元测试测不出，
  真机多行立即失配。已按行×字布局修复，多行常量（第 1 行首字 = 字 2）钉死。
- **trit-net 兼容测试**（dev-dep）：`Model`/`Layer` ↔ `TritModelLayer` 经
  `rows()` 无损往返；断链（上层输出 ≠ 下层输入）两边同语义拒绝。
- 测试 +11（trit-texture 26→31、trit-net 16→18、trit-gpu 真机 +1）；
  变异门 216 → **220**（新增 4 条：双轨行界错位〔真 bug 形态〕、降级放行 mip、
  v2 解码取末级、盒式滤波只取首像素），trit-texture 14/14 全灭；
  workspace **710/0**；发布图/预算/docs 门绿。GGUF → MOD 入向转换未做
  （需读 GGUF 元数据层，体量另计）。

### Changed（MOD 载荷按「兼容/强度/三进制原生」重构 + 溢出加固 + 扩展名规范）

- **MOD 重构**（用户三优先级：兼容、强度、适合三进制本身；双轨 2 bit/权重
  的 B 类税方案废弃）：
  - **权重改基 3 打包**（5 trit/字节 = **1.6 bit/权重**，比 Q2_K ≈2.56 省
    37.5%，距理论下限 1.585 仅 +0.9%）——权重/偏置/纹理索引/校验流全套同一
    套打包，格式内没有第二进制制；GPU 改经 `TritModelLayer::to_dual_rail()`
    一次显式转换（位布局手算常量钉死），不再把双轨当存储格式。
  - **容器校验升级 GF(3) CRC-16**（g = **x¹⁶+x⁷+2** 本原，x 模 g 阶 =
    3¹⁶−1，素因子逐一排除，独立 Python 验算）：≤16 连续 trit 突发错**全检**
    （40 权重全部 16-trit 窗口穷举测试），随机错漏检 ≈ 3⁻¹⁶；CRC-3 尾 1 字节
    → CRC-16 尾 4 字节，TEX/MOD 两类同享。TTX1 的 CRC-3 行为冻结（实际除数
    是 x³+2x+1 的互反 x³+2x²+1，注释如实更正）。
  - **层间衔接进容器校验**（上层输出 = 下层输入，读写同一来源，与 trit-net
    `Model::validate` 同语义）；`rows()` 无损取回 `Vec<Vec<i8>>`。
- **溢出加固**（自查发现，32 位目标路径）：TTX1 `from_ttx` 的 `blocks × 12`
  与 TTX2 `level_blocks` 的字节预算改 checked 乘法，解析游标全部 checked——
  恶意超大声明在 64 位上被长度对账挡住、在 32 位上不再回绕放行；两条超大
  声明回归测试（v1/v2 各一，拒绝且不按声明分配）。
- **`docs/TTX.md` §4 文件扩展名规范**：唯一 `.ttx`（小写）、魔数是唯一事实源、
  禁止 .ttx1/.ttx2/.ttm 变体、MIME `application/x-ttx`；**新后缀准入政策**：
  仅当格式主体数据有三进制原生红利（A 类）才注册新后缀，其余复用现有后缀。
  §2/§6 随重构重写（模型侧 1.6 bit/权重、CRC-16 布局、GPU 转换一次如实记）。
- 测试 +5（trit-texture 21 → 26：基 3 打包常量、双轨转换布局、层间衔接、
  16-trit 突发穷举、超大声明×2）；金样字节重铸（CRC-16 尾 [144,81,14,1]
  独立 Python 长除法验算后冻结）；变异门 215 → **216**（ttx2 锚点随迁：
  双轨/字数/轨重叠 5 条退役，基 3 打包/digit 映射/层间衔接/CRC 常数 4 条
  新增 + 等价变异体「空层照收」退役并注明原因）；workspace **704/0**。
- 同轮仓库面：bfxh 名下 17 仓全部转公开（strata 直接翻；归档仓 uiscan 走
  解除归档→翻公开→重新归档；翻前扫过凭据类文件名，strata 的 baseline.env
  为性能基线数字非密钥）。

### Added（TTX v2：多载荷容器——GPU 纹理 mip 链 + 三值模型双轨权重，先写定位文档）

- **`docs/TTX.md`**（先于实现）：TTX 对业界格式的竞争力定位——纹理侧对比
  PNG/QOI/BC1/BC7/ETC2/ASTC/BasisU（TTX 2.40 bpp < BC1 4.0，3 色调色板天花板、
  无硬件采样器如实记），模型侧对比 safetensors/GGUF Q2_K/BitNet b1.58
  （双轨 2 bit/权重 < Q2_K ≈2.56，比理论下限 1.585 多付的 26% 是 B 类税），
  特点清单 + TTX1/TTX2 字节级规范 + 转换路线。
- **TTX2 容器**（trit-texture::ttx2，拆 `mod.rs` 容器 / `model.rs` 模型域）：
  魔数 `TTX2` + kind（TEX/MOD）/flags（非零显式拒绝，前向兼容不猜）/段长头 +
  kind 段 + GF(3) CRC-3 尾（对 kind 的**规范 trit 流**算，与 TTX1 同哲学）。
- **kind=TEX**：纹理 + mip 链——每级自带宽高与块载荷，宽高不增且至少一维
  严格递减（同级/未对齐/空链都是结构化错误）；4×10 块高的 mip 只能降 2–3 级
  是方块格式没有的诚实约束，写进文档。读写共用 `validate_levels` 单一来源。
- **kind=MOD**：三值模型（`TritModel`/`TritModelLayer`）——权重 {-1,0,+1} 存
  **双轨位平面**（行整字、bit = `1<<(i%32)`），与 trit-gpu matmul 内核的
  storage buffer 布局逐位一致（手算常量钉死），**上传零重排**；偏置基 3 打包
  （5 trit/字节）。pos/neg 同字同位重叠 = 结构化拒绝（PR #116 的教训：非法轨
  不许 panic 也不许静默）。层头 words_per_row 与 ceil(inputs/32) 对账。
- 校验流覆盖**权重 + 零填充尾 + 偏置**：偏置字节打翻必被抓（测试钉住）；
  金样字节测试（CRC 尾字节经独立 Python GF(3) 实现交叉验算）杀死任何序列化
  漂移。v1 文件喂给 v2 读器得到显式 `TtxMagic(['T','T','X','1'])`。
- 测试 +9（trit-texture 12 → 21）；workspace **699/0**；变异门 207 → **215**
  （新增 8 条：mip 同级放行、打包/解轨位序错 31、字数对账用 floor、空层照收、
  写/读侧轨重叠失明、偏置打包换基 2）。ttx2.rs 拆 mod/model 后 532/301 行，
  模块预算零债务保持。

### Changed（CI：计费分钟削峰——macOS 门移至 main，不降覆盖）

- GitHub Actions 月度配额 10 月告警 90%（1801/2000 分钟）。按 runner 计费倍率
  （Linux 1× / Windows 2× / macOS 10×）抽样 40 次近期运行：**macOS 一个 job 占
  计费分钟 74%**（UBUNTU=46 / WINDOWS=102 / MACOS=426），另有冷缓存单次 153 分钟离群。
- `check` 矩阵改为按事件分流：**PR 门跑 ubuntu+windows，macOS（aarch64 信号）在
  每次合并后的 main push 上全量跑**——每个合入 main 的提交仍被三平台验证，覆盖不减，
  单次 PR 计费预期从 ~11-14 分钟降到 ~4-6 分钟。
- `mutation.yml` 补并发取消组：同一 PR 的新 push 立即取消被顶掉的变异门运行
  （此前无 concurrency 组，多 push 的 PR 每次都全额跑完）；定时/手动运行用唯一
  run_id 分组不受影响。其余 9 个 Linux 门（module-budget/stress/publish-graph/
  docs/action/msrv/package/wasm/release）与 PR 变异门全部保持原样。

### Added（trit-codec：量化/稀疏/交织/校验四个小编解码组件，PR #116 修后合入）

- `quantize`：f32 ↔ trit 三值量化/反量化（TWN 风格，`beta=0.7` 默认，scale = max(|w|)），
  附 L2 量化误差读数；**有损是设计目的**（crate 文档的无损不变式已随之修订）。
- `sparse`：三值 COO 稀疏容器——只存非 Unknown 的 `(index, value)`，解码校验
  下标严格递增/不越界（结构化错误）。
- `interleave`：块交织/解交织（行写列读），把 burst error 打散配合 hamming 纠错；
  `block=1` 或 `≥ len` 恒等，尾巴 ragged 可逆。
- `checksum`：GF(3) CRC-3（`x³+2x+1`），只检错不纠错；文档声明的**本原性（x 模 g 阶 = 26）
  已钉成测试**（多项式乘法/约减逐位验证，26 步循环）。
- CLI 新增 quantize / dequantize / sparse / interleave / checksum 子命令。
- 测试 +14（codec 52 → 66）；变异门 202 → **207**（新增 5 条：
  量化 scale 折向、阈值边界、稀疏重复下标、交织行列、verify 截断放行）。
- **同 PR 的 `layout` 模块被设计否决剔除**（另设第二套双导轨类型 + panic 策略），
  记录见 `docs/DESIGN.md`「不批的设计提案」。外部贡献（bamboo-echo），ZCode 复审合入。

### Added（三态结构体收尾 + 自有纹理容器格式 TTX）

- **`TriStateDriver`**（trit-core）：三态驱动器——`enable` 时把 `drive` 推上总线、
  未使能呈高阻。这是 TriState 故事缺的最后一块拼图（值/冲突解析/总线有了，
  **使用方**没有）。
- **`resolve_drivers`**（trit-core）：多驱动总线解析——`drivers[lane]` 为挂在该车道上的
  全部驱动器，一次**收集全部冲突车道**（每条列出完整肇事方清单），与
  `TriStateBus::resolve` 的「两个既定总线、首错即断」互补（诊断/复位场景）。
- **自有格式 TTX**（trit-texture::ttx，TritText eXchange）：TT3 块纹理容器——
  魔数 `TTX1` + 尺寸/块数头 + 12 字节块载荷 + **GF(3) CRC-3 校验尾**（对全部索引
  trit 按三进制多项式 `x³+2x+1` 计算，单符号错全检——二进制容器用比特 CRC，
  TTX 按内容自己的基数算校验，这是 A 类设计选择）。加载逐段验证：魔数/尺寸
  对齐/块数/载荷长度/校验和，任何不一致都是结构化错误。
- 测试 +8（trit-core 84、trit-texture 12）；workspace **690/0**；fmt/clippy/
  模块预算/发布图绿。trit-core 新 API：`TriStateDriver`、`resolve_drivers`、`LaneConflict`。

### Changed（trit-gpu：矩阵乘加寄存器阻塞——三值再 1.6×，f32 无收益，最优对最优 21.1×）

- tiled 之上新增**寄存器阻塞入口** `main_tiled2`（两侧同构）：workgroup 8×8、每线程
  2×2 四个累加器——共享内存的 A/B 行段每次读取喂 4 个输出，算术强度再翻倍。
- **真机读数（RTX 4060 Ti，512×4096×512）**：三值 tiled 0.208 → **0.129 ps/MAC**（再 1.6×，
  popcount 内循环的指令级并行吃到了）；f32 tiled 2.641 → 2.712（**无收益**，FMA 已饱和、
  occupancy 下降抵消——如实记负结果）。**最优对最优：14.5× → 21.1×**。
- 修一个真 bug：`row_base` 曾用 `global_invocation_id × 16`（线程号！）——相邻 workgroup
  互相覆盖输出（首差 out[0]/out[1]，真机 `reg_matches=false` 暴露）；改用 `workgroup_id`。
- 正确性：三侧（naive/tiled/reg）各自与 CPU 镜像 `matmul_words` 逐元素一致（真机断言）。
  reg 内核同样在 `gpu` feature 后，默认构建变异门测不到——真机对拍钉住（如实记）。

### Changed（trit-gpu：矩阵乘内核升级到共享内存分块，「最优对最优」同尺）

- naive 之外新增 **tiled 内核**（同 bind group、新入口点 `main_tiled`，2D dispatch）：
  三值侧 16×16 输出 tile、每段协作装载 8-word（256 trit）A 行段 / B 列段进
  workgroup storage——A 段被 16 个列线程复用、B 段被 16 个行线程复用；
  f32 侧经典 16×16 SM tiling。
- **真机读数（RTX 4060 Ti，512×4096×512）**：三值 naive 1.60 → tiled **0.183 ps/MAC**（8.8×）；
  f32 naive 25.89 → tiled **2.641 ps/MAC**（9.8×）。**最优对最优：14.5×**
  （naive 对 naive 是 16.2×）——分块把两边流量各砍一个量级后，比值从 16 收窄到 14.5：
  f32 的 FMA 算力在带宽瓶颈缓解后占比上升，与「三值的红利是流量密度」自洽。
- 正确性：tiled 与 naive 各自与 CPU 位并行镜像 `matmul_words` 逐元素一致（真机断言）。
  开发中修掉一个真 bug：三值 tile 的装载曾写 16 宽、累加读 8 宽——相邻 chunk 的字
  被混入（f32 的 TILE=线程数恰好无此问题；构造测试 + 随机纹理双重对拍暴露）。
  tiled 内核在 `gpu` feature 后，默认构建变异门测不到——正确性由真机对拍钉住（如实记）。

### Added（三进制 GPU 内核研究：三值矩阵乘 + TT3 纹理解码，真机实测）

trit-gpu 新增两个三进制 GPU 内核（`gpu` feature 后），全部与 CPU 镜像真机对拍：

- **三值矩阵乘内核**（`matmul.wgsl` + `matmul_f32.wgsl` + `matmul.rs`）：A 行主序、
  B 转置行主序的双导轨布局，`C[m,n]` = 四组 POPC 点积（复用 dot 内核公式）。
  **真机读数（RTX 4060 Ti / Vulkan，512×4096×512，naive 一线程一输出）**：
  三值 **1.66 ps/MAC** vs 同结构 f32 基线 **25.89 ps/MAC** —— **同设备 15.6×**。
  机理：naive 内核受显存带宽支配，每 MAC 三值搬 2 bit、f32 搬 32 bit，
  16× 流量差兑现成 15.6×（A 类：数据表示的红利）。GPU 结果与 CPU 位并行
  镜像逐元素一致。
- **TT3 块纹理解码内核**（`texture.wgsl` + `texture.rs`）：4×10 块、基 3 取位
  （`byte/3^i % 3`，不是位段）、块网格行主序映射 `(x/4, y/10)`；与 trit-texture
  的 CPU 解码在 256×60 合成纹理上**逐字节一致**（真机验证）。修掉两个真 bug：
  基 3 打包曾被当 3-bit 位段解；块映射曾按平铺扫描序（只有 block 0 的构造
  测试测不出——随机纹理首差 texel 4 暴露）。
- **CPU 镜像进默认构建**（CI 可测）：`pack_columns`（B 转置列打包）、`pack_rows`
  （A 行打包）、`matmul_words`（复用 `dot_words`）；非整字边界（k=100）+ 非方阵
  与标量参考对拍。
- **发布图门修正**：dev-dependencies 不进发布链——`cargo package` 打包时剥离
  path dev-deps（实证：trit-gpu dev-dep 指向 trit-texture，package rc=0）。
  门跳过 dev 边并新增反证例（自测 5 → 6 条全绿）。
- 测试 +2（CPU 镜像对拍、GPU matmul 对拍含域检查）+ GPU 纹理对拍 4 条。
  变异门 200 → **202**（新增 2 条：列打包导轨互换、matmul 行基址），trit-gpu
  **5/5** 实跑全抓。workspace **649/0**；fmt/clippy（默认 + gpu feature）/
  模块预算/发布图绿。真机 11 条 GPU 测试全过。

### Changed（trit-texture 提速/省显存/清税：4×10 零浪费块 + 栈数组 + into 解码）

- **块 4×8 → 4×10（40 纹素）：清掉打包粒度税**。40 恰被 5 整除，trit 载荷 8 字节
  **零槽位浪费**（旧 4×8 的 7 字节容 35 槽只用 32，≈8.6% 税）。等调色板码率差
  **8.3% → 14.3%**（12 vs 14 字节/块；TT3 **2.40 bpp** vs 基线 2.80 bpp），
  合成纹理实测 PSNR 差仍为 **0.000 dB**（22.58 dB 持平）——`cargo run -p trit-texture` 可复算。
  显存账（crate 文档）：4096×4096 RGBA 原图 64 MiB ⇒ TT3 约 5 MiB（12.7×），基线约 5.9 MiB。
- **编码提速 27%**：`encode_rgba`/`encode_rgba_baseline` 逐块的 `Vec<[u8;4]>` 堆分配
  （每块 128 B、4096 块/512×260 图）改**栈数组**。基准（新增 `benches/texture.rs`，
  `cargo bench -p trit-texture`）：encode 628 → **491 µs**（**808 → 1025 MB/s**），
  512×260 合成纹理。
- **免分配解码**：新增 `decode_rgba_into(&texture, &mut out)`——热路径复用调用方缓冲，
  长度不符显式报错不 panic；`decode_rgba` 改为它的分配包装。基准如实记：0.5 MiB 纹理上
  into 仅快 ~1%（371 → 366 µs，分配成本在此规模不显著），价值在复用热路径与大图。
- 测试 8 条（新增 into == 分配逐字节一致 + alpha 通道显式钉 255 + 短缓冲拒绝）。
  变异门 199 → **200**（新增 into alpha 锚点，trit-texture **5/5** 实跑全抓；
  首轮存活——两条路径共用实现导致 alpha 突变对拍不出，补通道断言后判红）。
  workspace **648/0**；fmt/clippy/模块预算/发布图绿。

### Added（三进制块纹理压缩实验层 trit-texture）

- 新 crate `trit-texture`（`publish = false`）：格式 **TT3**——4×8 = 32 纹素块，
  RGB565 端点对（4 字节）+ **每纹素 1 trit** 的 3 级调色板 `{e0, mid, e1}`，
  5 trit/字节打包 7 字节 ⇒ **11 字节/块 = 2.75 bpp**。附 `trit-texture` 二进制
  在合成纹理上打同机对比表（`cargo run -p trit-texture` 可复算）。
- **二进制基线同尺对比**（同块结构、同端点、同 3 级调色板、同最近色搜索，
  唯一变量是索引的进制——2 bit/纹素）：基线 12 字节/块 = 3.0 bpp。
  合成纹理实测：**TT3 2.75 bpp vs 基线 3.00 bpp，PSNR 相同（24.63 dB，差 0.000 dB）**
  ——等调色板下 trit 索引省 1 字节/块（码率 -8.3%）而画质不变，这是可复算的
  A 类（原生红利）结论。
- **边界如实记账**（docs/TERNARY-BOUNDARIES.md 分类，写入 crate 文档）：
  A = 3 级索引 1 trit 装得下、2 bit 装不满；B = 7 字节容 35 槽只用 32（≈8.6%
  打包粒度税）+ RGB565 端点量化税；C = 端点对抄 BC1 生态格式；D = 最近色搜索
  与进制无关。
- 测试 7 条：243 种 5-trit 组合逐位往返、端点色零误差往返（可表示色用 crate 自己的
  pack→unpack 构造，色对须全通道单调——端点是逐通道 min/max，与 BC1 一致）、
  mid 槽位精确落位、并列色取小下标（确定性）、块字节 11/12 与码率代数、
  形状/尺寸/超限/块数错误路径、PSNR 排序。变异门 195 → **199**（新增 4 条，
  trit-texture **4/4** 实跑全抓）。workspace **647/0**；fmt/clippy/模块预算/发布图绿。

### Fixed（复审 P0 批次 3：trit-schedule 三件套，H04/H05/H06）

- **H04 调度延期传播 O(n²)**：不动点传播（每轮全量扫描所有任务，反拓扑链每轮只推进
  一层）改为**工作表传播 O(V+E)**——`propagate_unknown_deferrals` 抽成 `lib.rs` 的
  `pub(crate)` helper，`schedule`/`resumable`/`resumable_threaded`/runtime 四处拷贝
  全部收编。传播闭包与旧实现一致（`BlockedBy` 原因取标记时按声明序首个已延期前驱）。
  10 万项反向链（旧实现 10^10 次内层访问）现在秒过。
- **H05 runtime 资源累计溢出 panic**：`capacity = None` 不做单任务预检，两个
  `cpu = u32::MAX` 需求并发累计曾 `checked_add(...).expect(...)` panic——毒化锁后
  其余 worker 无法收尾、调用方挂死。改为**按任务失败处理**（`failed` + `stop` +
  `cancel_pending` + notify），与任务失败路径同一收尾。
- **H06 可恢复多线程执行器不接 step panic**：`task.step` 的 panic 曾直接穿透
  worker（running/资源不归还、不 notify）→ 其余 worker 与调用方永久等待。现在
  `catch_unwind` 包住并走 `Progress::Failed` 同一收尾路径。
- 测试 +3：10 万项反向链（原因逐点断言 + src 正常排定）、无容量双 MAX 资源溢出
  （确定性构造：先占满者持锁睡 100ms）、step panic 不挂死（boom 失败 + ok 完成）。
- 变异门 192 → **195**（新增 3 条，全部选择**不产生挂起**的变异——挂起变异会把
  变异门本身挂死；另 2 条旧锚点随重构迁移：`不传播延迟` → helper 反向边、
  `资源占用不累计` → `new_used` 赋值）。trit-schedule **34/34** 实跑全抓；
  workspace **640/0**；fmt/clippy/模块预算绿。

### Fixed（复审 P0 批次 2：AIGER 头部预算，H07 panic 封堵）

- **H07**（`aiger.rs`）：头部 `O` 计数此前直接 `Vec::with_capacity(outputs)`——一行
  `aig 0 0 0 18446744073709551615 0` 就触发 capacity overflow panic；`inputs + ands`
  的 `usize` 相加也会溢出 panic。现在：新增 `MAX_AIGER_BYTES = 1 MiB` 字节预算（解析前）；
  `I+O+A` 与 `I+A` 全部 `checked_add`；声明数超过文本行数提前拒绝（每条声明至少占一行，
  超行数必然解析不出来）；`with_capacity` 移除。`AigerError` 新增 `Limit` 变体。
- 测试 +1（四类恶意头部/超大文本全走 `Limit`）；变异门 190 → **192**（新增 2 条，
  trit-circuit **92/92** 实跑全抓）。workspace **637/0**；fmt/clippy/模块预算绿。

### Fixed（复审 P0 批次 1：解析递归护栏，H01–H03 栈溢出封堵）

全面复审（`_handoff/trit-v0.12.0-全面代码复审.md`）的三个**可远程触发的栈溢出**（隔离探针
实测 abort 134）：解析器是递归下降，文本字节上限拦不住深层嵌套——三处分别加护栏，
超限返回 `Limit` 类错误而非进程 abort：

- **H01 Verilog assign 右值**（`verilog/expr.rs`）：拼接 `{a,{a,{…}}}` 此前不计深度
  （只有括号/一元计）→ 纳入 `MAX_RHS_DEPTH`（256）；二元平链构造成左深 AST 后
  `bit_expr`/`synth_v` 递归遍历栈溢出 → 新增 `MAX_RHS_OPS = 256`（每消耗一个运算符计数，
  超 `+1` 即报错）。2 万层拼接、3 万项平链现在都返回 `VerilogErrorKind::Limit`。
- **H02 SDF**（`sdf.rs`）：`parse_node` 对括号嵌套无上限、`collect_sdf` 不拒绝超大文本 →
  新增 `MAX_SDF_DEPTH = 256` 深度护栏与 `MAX_SDF_BYTES = 1 MiB` 字节预算；
  `SdfError` 新增 `Limit` 变体。
- **H03 Verilog 参数表达式**（`verilog/params.rs`）：`ExprParser::factor` 对括号/一元
  递归不设防 → `MAX_PARAM_DEPTH = 256`（`enter_then` 闭包保证两条路径都还原深度）。
- 测试 +5（拼接深度、链上限 +1、预算内链可综合、SDF 深度/字节、参数深括号——其中
  SDF 250 层与预算内链同时验证**护栏不误伤**）。变异门 185 → **190**（新增 5 条，
  trit-circuit **90/90** 实跑全抓）。workspace **636/0**；fmt/clippy/模块预算绿。
- 教训一条：长链测试最初写 3 万项，「阈值 ×40」的变异仍会报 Limit 而存活——改成
  「上限 +1」项后阈值任何放大立即暴露，且不再依赖栈溢出 abort 才判红。

### Fixed（复审修复：平衡三进制 codec 与 trit-float 的溢出/语义缺陷）

外部复审（无 Rust 工具链、按 std 契约推断）+ 本地逐条实测确认后修复；实验层
（`publish = false`）按其建议优先级 H1 → M1 → M2 → M3 落地：

- **`balanced::range(n)` 溢出 panic（H1）**：`3i128.pow(n)` 在 `n ≥ 81` 越界 panic；
  改 `checked_pow`，越界**饱和到 `i128::MAX`**（`range(80) ≈ 7.4e37` 仍精确）。
- **`balanced::decode` 无长度上限（M3）**：超长全零串会按位循环百万次。现在先剥
  前导 `0`（`"00+0"` 与 `"+0"` 同值，行为不变、写入文档），剥后超 41 位直接
  `Overflow`——41 位已覆盖整个 i64，42 位数值必然越界。
- **`TritFloat` 极值 panic（M1，实测比复审多两处）**：`digit_count` 对 `i128::MIN`
  取 `abs()` panic → 改 `unsigned_abs()`；`drop_digit` 的 `(value - remainder)` 对
  `±i128` 极值下溢 → 改 `div_euclid`（数学等价）；指数 `+=` / 相加 / 相减全部
  `saturating`；`Neg` 改 `saturating_neg`。
- **`TritFloat` 加法静默改值（复审 H10，比 panic 更重）**：`x + 0` 对大指数 `x` 曾被
  `saturating_mul` 改写约 72%。对齐溢出时较小项低于任何精度的半个 ulp，**丢弃**
  而不是饱和——`x + 0 == x` 恢复成立。
- **`TritFloat` 的 `Eq` 把 `precision` 算进去且无 `Ord`（M2）**：改为**数值**相等/全序
  （规格化后 `(mantissa, exponent)` 是数值规范形，逐字段比较即数值比较）；
  `from_f64(1.0, 10) == from_f64(1.0, 20)` 成立，补 `PartialOrd + Ord`（对齐溢出按
  符号定序）。
- 测试：codec +2（range 80/81/饱和、剥零/41 位边界/i64::MAX 往返）、float +3
  （`x+0` 恒等、极值不 panic、数值相等与全序含对齐溢出分支）。变异门 178 → **185**
  （1 条随 `saturating` 迁移，新增 7 条；codec 7/7、float 8/8 实跑全抓——其中
  「对齐溢出判成更小」首轮存活，补齐 shift>80 与 `3^40×3^41` 两类断言后判红）。

### Changed（拆最后一个上帝模块：verilog/parse.rs，预算台账清零）

- `verilog/parse.rs` **861 → 471 行**，按职责拆出三个子模块，代码原样搬移（语义零改动）：
  - `build.rs`（119）：`Builder`——assign 右值到门的降级（临时信号、门数预算、表达式综合）；
  - `decl.rs`（195）：端口/线网声明解析（位宽范围、名字列表、`declare_ports`/`mark_used`）；
  - `instance.rs`（108）：模块实例语句（命名/位置连接）。
  `parse.rs` 只留顶层编排（模块扫描、header、语句分发）。
- **模块预算台账清零**：`module_budget_baseline.json` 的 `over_budget` 从 1 件
  （parse.rs 861）变为 **0 件**——全仓 157 个 `.rs` 文件无一超 800 行预算，
  之后任何新上帝对象直接红，不再有登记缓冲。
- 变异门锚点随迁 3 条（`向量声明记成 1 位`/`向量端口只展开 bit 0` → `decl.rs`，
  `assign 的 ^` → `build.rs`），总数不变（178）；`升序 LHS`/`多输入 NAND/NOR` 两条留在
  `parse.rs` 原锚点。顺带把两条双斜杠路径归一化。

### Changed（分支无关化收尾：stride/bmask 编码 + 保守区间 min/max）

- `Simulator` 热循环去掉最后两个结构性分支（`if kind == Const` / `else if arity == 2`）：
  门类型/元数编码进 `CompiledGate.stride`（常量 0、一元 1、二元 3）与 `bmask`
  （二元 3、其余 0），热循环只剩同一条 `lut[a*stride + (b & bmask)]`。ATPG 同机读数
  98.1–102.8 µs（上一组 99.3–106.4 µs 区间内，无回归）。常量槽位只填可寻址的 `lut[0]`、
  不做防御性 `fill`——错编址会立刻读到未填的 `False`，测试抓得住。
- **分支预测失败机制读数**（`benches/core.rs` 新增 branch probe，2×2 表）：
  `match` 求值在随机输入 **7.1–7.3 ns/op** vs 固定输入 **1.6–1.9 ns**（≈4×——分支预测器
  学不会的分支）；查表两种输入持平（0.66–1.05 ns），随机输入下 match/LUT 差 **9–11×**。
  这张表把「图里说的分支预测税」变成了可复算的同机数字。
- `trit-net` 保守区间分析去数据分支：每权重 `if weight >= 0` 改为端点项 `min`/`max`
  各取一次（`w<0` 时区间端点翻转由 min/max 吸收，任意权重都成立）；逐层符号压缩从
  neg/zero/pos 三连分支化成 `sign(x) = (x>0) - (x<0)`（原 `zero_possible` 对 `sign`
  是冗余条件——`high >= low` 恒成立）。同机读数：`analyze 128×128×3`
  **131.96 → 39.9–40.1 µs（≈3.3×）**；`forward` 13.8 → 14.2–14.3 µs（噪声边缘，如实记）。
- `trit-net` 接入首个基准 `benches/net.rs`（前向 + 区间分析的同机读数）。
- 测试：逐点对拍补 Const 门（stride=0 路径）；`trit-net` 新增 2 条（负权重区间必须弃权、
  40 组随机「保守区间不得与精确枚举矛盾」对拍）。变异门 174 → **178**
  （3 条锚点随重构迁移到 stride/bmask 与 min/max 形态，另新增 4 条，逐条实跑判红）。

### Changed（底层：去分支查表 + 硬件 POPCNT 派发）

- `Simulator` 的门求值从「按门类型 9 路 `match`」改成**每门预计算真值表**：
  `CompiledGate` 带上 `lut: [Ternary; 9]`（一元 3 项、二元 9 项），热循环里只剩
  一次下标运算 + 一次查表。ATPG 主路径同机实测 **115.1 / 125.7 / 113.8 µs →
  99.3 / 99.8 / 106.4 µs**（多次取中位，约 13%），`analyze` 在噪声内（~535–558 µs）。
- `Ternary` 加 `#[repr(u8)]` 与 `index()` / `from_index()`，`False = 0`、`Unknown = 1`、
  `True = 2`，LUT 下标 `a*3 + b` 直接来自序号，零转换成本。
- `trit-dot` 的三值/二值点积加**运行时派发**：`std::arch::is_x86_feature_detected!("popcnt")`
  命中时走 `#[target_feature(enable = "popcnt")]` 的 4 路展开内核，否则回退便携单累加器。
  默认构建里 `objdump` 原本**一条 `popcnt` 都没有**（`count_ones` 被展开成约 12 条软件位运算），
  派发后 37 条。同机同构建（Arrow Lake Ultra 7 270K Plus）每 trit 实测：
  1 Mi 派发 **0.0088–0.0092 ns** vs 便携 **0.0128–0.0137 ns**（1.35–1.45×）；
  16 Mi 派发 **0.0103–0.0106 ns** vs 便携 **0.0128–0.0129 ns**（1.22×，已开始吃带宽）。
  纯 4 路展开而不开 `popcnt` 反而慢 12%（软件 popcount 延迟长、展开只增寄存器压力）——
  这条负结果保留在 `dot_words_unrolled` 的 A/B 读数里。
- 二值基线同待遇派发（否则比的不是编码而是谁碰巧拿到指令）：1 Mi **0.0038** vs 便携
  **0.0057–0.0058 ns/元素**；三值/二值比值仍约 2.3×，与「双导轨 2 bit vs 1 bit」相符。
  `f32` 标量 4 累加器 0.14–0.16 ns/元素，三值位并行约 **16–18×** 于它（1 Mi）。
- `trit-dot` 的 `#![forbid(unsafe_code)]` 收窄为 `#![deny(unsafe_code)]`，唯一放开处是
  `popcnt` 模块（两个 `#[target_feature]` 内核），其余代码仍禁止 unsafe。
- 测试：`dot` 新增 3 条（派发 vs 便携逐值一致、各种尾巴长度、4 个 lane 独立贡献），
  `trit-core` 新增 1 条（序号往返 + 9 槽不重叠）。变异门 168 → **174**。

### Changed（精确传播：索引化内循环 + 有上限的并行）

- `propagate_exact` 的枚举不再对每个完成调用 `propagate`（那会为**每个信号**建
  `BTreeMap<String, _>`）；改成复用 `Simulator` 的索引化求值，只分配一份向量。
  同机同电路实测：k=8 **74×**、k=10 **209×**、k=12 **334×**（14.36 ms → 194 µs / 57.8 ms →
  277 µs / 239 ms → 716 µs），`k=8` 的永久基线读数见 `benches/core.rs`。
- 新增 `propagate_exact_with_jobs(..., jobs)`：完成之间独立，按块分给 `jobs` 个线程，
  再按「值不同记 `X`」合并；结果与并行度无关（`sim_tests` 在真分块的 k=12 上对拍串行）。
- **并行度按工作量收敛**：每线程至少 1024 个完成（`MIN_COMPLETIONS_PER_JOB`），
  `jobs = 0` 表示自动。实测 k ≤ 10 串行更快，k=12 四线程 1.6×，k=14 2.2×，k=16 5.4×；
  不设上限时 `available_parallelism` 会在亚毫秒任务上炸出几十个线程，反而更慢（k=12 auto 0.92 ms
  vs 四线程 0.46 ms）。`ExactPropagation` 新增 `jobs` 字段报告**实际**并行度。
- CLI：`trit circuit FILE --assign ... --exact [--jobs N]`，表格输出枚举完成数与实际并行度，
  JSON 增加 `exact` 对象（`exact` / `completions` / `budget` / `jobs` / `unknown_inputs`）。
- 测试：新增 `sim_tests`（串并行等价、**真分块**断言 `jobs == 4`、完成数/预算上报、超预算回退）
  + 3 条 CLI 测试；变异门 139 → **141**（新增 3 条并逐条实跑判红，删 1 条被重写覆盖的旧锚点）。

### Changed（三值拓扑：两趟 BFS + 三值验证证书）

- `TernaryTopology::reachability` 从「逐边松弛到不动点」改为**两趟 BFS**：先沿 `True` 边求
  `True` 集，再从 `True` 集出发沿非 `False` 边求 `Unknown` 集。结果与旧最小不动点逐点一致
  （200 个随机图上与参考实现逐点对拍），但**不再依赖边的插入顺序**。
- 实测（研究探针，未入库）：稀疏随机图 1.7×；长链且边按拓扑序 0.3×（旧实现一轮扫完）；
  同一条链边逆序时旧实现 12,500 轮 / 20.8 ms，新实现 0.10 ms（**199×**）；稠密图 1.1×。
- 邻接表随 `add_edge` 持久维护：第一版每次调用重建邻接表，实测反而慢 3×，已改。

### Added（三值自己的验证结论）

- `TernaryTopology::explain(origin, target) -> ReachEvidence`：三值结论
  `Verdict::{Proved, Undetermined, Refuted}` + `value` + `path` + `unknown_edges`。
  `Proved` 给纯 `True` 路径；`Undetermined` 给一条路径与路径上需要补全的 `Unknown` 边；
  `Refuted` 表示任何补全都不可达。结论是**通过 / 未定 / 否定**，不是二值的「可能」。
- 新增 `trit topology FILE [--origin N] [--to M] [-f json]`：JSON 输入
  （`nodes` / `origin` / `edges`，边值 `0`/`1`/`X`）；不给 `--to` 输出整张传播表，
  给了则输出单点判定与证书。`TernaryTopology` 此前没有任何消费方，现在有了对外入口。
- 测试：9 条库测试（200 组随机图与旧不动点对拍、100 组随机图 explain 与 reachability 一致）
  + 4 条 CLI 集成测试。
- 变异门 136 → **139**：删 1 条已失效的（拆模块后锚点落进测试代码），新增 4 条并逐条实跑判红。
- 变异门 `preflight` 增加规则：锚点落在 `#[cfg(test)] mod ...` 之后直接报错——
  改测试代码不改变产品行为，不该算覆盖。

### Changed（清模块预算债务：5 → 1）

- `verilog/tests.rs` 1094 → **576** + `verilog/hierarchy_tests.rs` 528（层级/参数化测试独立成文件）；
- `trit-cli/tests/cli.rs` 1100 → **459** + `tests/cli_io.rs` 674（每文件自带 `trit_bin`/`run` 助手）；
- `trit-core/src/bits.rs` 801 → **534** + `src/bits/tests.rs` 264；
- `trit-circuit/src/map.rs` 842 → **480** + `src/map/tests.rs` 359；
- 台账从 5 件缩到 **1 件**（`verilog/parse.rs` 861，生产代码，需按职责拆，未做）。
- 测试条数不变（verilog 58、CLI 42+36、core 80、circuit 182）；fmt/clippy 全绿。

### Added（底层补齐：`Ternary` 运算符 + `View::copy_into`）

- `trit-core`：`Ternary` 现在实现 `Default`（`False`）、`From<bool>`、`Not`、
  `BitAnd` / `BitOr`（**Kleene**）、`BitXor`（**经典异或**，任一侧 `X` 则 `X`）。
  Ł3 / B3 的差异仍走 [`Algebra`]，运算符只给三套语义一致的默认值；测试逐条断言。
  这让值层能直接写 `!a | (b & c)`，不必为每个门手写 match。
- `trit-codec`：`View::copy_into(&mut [Ternary])` 顺序批量拷贝——Binary / Packed 走专用循环，
  Sparse 先填 `X` 再回填非零位；`decode` 改用它（不再逐元素 `get`）。
  稀疏 1% 的「整条解码 + 随机取」从 **417 µs → 22.8 µs（18×）**；
  零拷贝视图相对它的优势相应从 242× 收窄到 **11.7×**——视图仍是随机访问最优，物化现在是廉价选项。
- 变异门 166 → **168**，两条新变异按**断言**判红（复核过不是编译错误导致的假红）。

### Added（零拷贝解码 `plan::View`）

- `plan::View::new(encoding, bytes, len)`：只校验、**不分配、不拷贝**；`get(i)` 按需从字节算。
  - `Binary` / `Packed`：O(1) 位/三进制运算；`Sparse`：对升序下标二分，O(log nnz)；
  - `decode` 改为 `View::new(...).iter().collect()`，两条路径共用同一套逻辑。
- 实测（65536 trit，随机取 200 个元素）：

  | 分布 | 整条解码 + 取 | 视图取 | 加速 |
  |---|---|---|---|
  | binary | 57.7 µs | 195 ns | **296×** |
  | packed（1/3 X） | 188 µs | 2.17 µs | 87× |
  | sparse 1% | 417 µs | 1.73 µs | 242× |
  | sparse 4% | 639 µs | 2.13 µs | 300× |
  | sparse 10% | 173 µs | 2.13 µs | 81× |

- **不适用于全量扫描**：视图逐元素 `get` 带边界/分支，整条 `decode` 顺序收集更划算（实测 0.95–1.09×）。
  零拷贝的赢面是「只取少数元素」。
- 测试：视图与 owned 解码逐元素一致、越界返回 `None`、坏缓冲（短/非法字节/长度不符）拒绝。

### Added（按分布选编码：把模拟税按实际情况降下来）

- 新模块 `trit-codec::plan`：给出**字节数最小**的编码，而不是一律 2 bit/trit（双导轨）。
  - `Binary`：整条向量无 `X` → **1 bit/trit**（省 50%）；
  - `Sparse`：非零占比低 → 每个非零 33 bit（u32 下标 + 符号位）；
  - `Packed`：其余 → 1.6 bit/trit（5 trits/byte，利用率 94.9%）。
- 稀疏的赢面是**算术结果**：`33·p < 1.6` ⇒ 非零占比 `< 4.85%` 才选索引；测试按这个阈值双向验证。
- `trit-codec plan` 实测（4096 trit / 双导轨 1024 B）：

  | 分布 | 选中编码 | 字节 | bit/trit | 相对双导轨 |
  |---|---|---|---|---|
  | 纯二值 | binary | 512 | 1.00 | **−50%** |
  | 1/3 是 X | packed | 820 | 1.60 | −20% |
  | 1% 非零 | sparse | 178 | 0.35 | **−83%** |
  | 4% 非零 | sparse | 685 | 1.34 | −33% |
  | 10% 非零 | packed | 820 | 1.60 | −20% |

- 测试 5 条（4 种分布往返、稀疏阈值双向、空向量）；编码/解码可往返。
- 边界：稀疏下标是 u32（大向量可换 varint）；解码是逐元素拷贝，未做原地/借用视图；
  这里只降**存储/带宽**税，不降 CPU 解包税。

### Changed（三值点积优化：CPU 1.65×，GPU 已到带宽上限）

- **CPU**：`pp+nn` 与 `pn+np` 各合成一次 POPC（`(a⁺&b⁺)|(a⁻&b⁻)`、`(a⁺&b⁻)|(a⁻&b⁺)`），
  四组 POPC → 两组。16M 实测 **21.7 → 13.1 ps/trit（1.65×）**；`dot_naive` / `dot_words_naive`
  保留作 A/B 对照。
- **GPU**：同场三内核对比（预热 20 轮 + 5 批中位数）：

  | 变体 | 1M | 16M | 64M |
  |---|---|---|---|
  | A 四组 POPC + 每字 atomic（**默认**） | 21.2 ps | 1.88 ps | 1.069 ps |
  | B 两组 POPC + 每字 atomic | 23.6（−10%） | 1.89（−1%） | 1.070（0%） |
  | C 两组 POPC + workgroup 归约 | 23.4（−10%） | 2.16（−13%） | 1.040（+3%） |

- **结论**：GPU 内核**带宽受限**，合并 POPC 与 workgroup 归约都不带来收益（±10% 内，部分配置更慢）；
  CPU 在同样规模**计算受限**，同一合并直接 1.65×。**同一改动两端结论相反**，必须分设备量。
- **测量方法修正**：GPU 时钟爬坡会让「第一个被测内核」系统性偏慢（同一变体先测 47.6、后测 21.2 ps），
  现在所有内核先预热 20 轮再计时。
- 下一步的唯一有效方向是**减少字节**（1.6 bit/trit 的 5-trit 打包），而不是减少指令；未实现、未测。

### Changed（`trit-gpu`：1 号/2 号编号 + 设备上限 + 四档规模读数）

- `trit-gpu bench` 按约定输出四组：**1 号 = 三值、2 号 = 二进制**，设备分 GPU / CPU，
  并给同设备加速比（1 号 / 2 号）。
- 设备改为按 adapter 的**实际上限**申请：默认 `max_storage_buffer_binding_size = 128MB`，
  64M trit 的 f32 缓冲（256MB）会在 `create_bind_group` 直接 panic。
- 实测（RTX 4060 Ti / Vulkan，5 批中位数）：

  | 规模 | GPU 1 号 | GPU 2 号 | GPU 加速 | CPU 1 号 | CPU 2 号 | CPU 加速 |
  |---|---|---|---|---|---|---|
  | 1M | 33.5 ps/trit | 63.0 ps/元素 | 1.9× | 48.3 | 195.0 | 4.0× |
  | 16M | 5.06 | 36.8 | 7.3× | 51.9 | 528.9 | 10.2× |
  | 32M | 1.76 | 34.4 | 19.5× | 53.4 | 452.4 | 8.5× |
  | 64M | 1.24 | 33.9 | 27.4× | 55.8 | 499.8 | 9.0× |

  （CPU 2 号单位 ps/元素，其余同。）
- **读法**：16M 起三值工作集（0.5 B/trit）开始装进 32MB L2，而 f32（8 B/元素）装不进，
  所以 GPU 比值从 7× 涨到 27×，**超过 16× 的字节比**——这是缓存层级效应，不是指令数变少；
  1M 时两者都在片上、延迟主导，只有 1.9×。

### Added（对比基线规则 + 高压测试门）

- `docs/TERNARY-BOUNDARIES.md` 新增两条硬规则：
  - **§6 任何对比都必须同时给二进制基线**：性能要同机 `f32`/`f64`（或 1-bit 二值）读数，
    精度要同比特预算的二进制上界；**没有基线不算结论**，且必须写明省的是**带宽/存储**还是**指令数**。
  - **§7 高压测试**：位运算 / 编解码 / 浮点边界必须有跨字边界、极端向量
    （全 `+1` / 全 `-1` / 单点 / 交替）与大规模随机的不变量测试；只测小样例不算验证。
- 新增 `tests/stress.rs`：
  - `trit-dot`：极端向量 × 长度 `1/31/32/33/63/64/65/1000`、bipolar 尾部补位、
    3 个种子 × 1M 随机、块级稀疏，以及**二进制基线自检**（朴素 f32 与四累加器互相一致）。
  - `trit-codec`：0..=256 全长往返、10 万 trit 大载荷、256 个字节值分类、2000 个随机 `i64`、
    200 组随机消息 × 4 位置 × 2 幅度的单符号纠错。
  - `trit-float`：6 个精度 × 1000 个值（指数 -40..40）不越 `3^{1-p}`；乘积误差 ≤ 2.5× 上界。
- CI 新增 `stress (high pressure invariants)` job（release 模式跑上述套件）。
- **修掉一个真实竞态**：CLI 集成测试的夹具用 `OnceLock` 的 `get` + `set`，这两步不是原子的——
  两个并发测试会同时看到 `None` 并各自写同一个临时文件，子进程可能读到截断的 JSON。
  改用 `get_or_init`（原子初始化）。这正是此前「ubuntu test 步 exit 101、日志取不到、复跑即过」
  那次 flake 的根因；修复后 CLI 测试连跑 5 轮、每轮 91 条全绿。

### Added（GPU 后端 `trit-gpu`：WGSL `countOneBits`）

- 新 crate `crates/trit-gpu`（`publish = false`）。**GPU 代码在 `gpu` feature 后面**，默认构建不拉
  `wgpu`，CI 的三平台 / MSRV job 不受影响；本地用 `--features gpu` 构建。
- `src/ternary.wgsl`：每 32 个 trit 打成 `u32`（双导轨），`countOneBits` 四组计数累加；
  `src/float.wgsl`：**同一个设备**上的 f32 点积（workgroup 归约），作为公平对照。
- CPU 位并行 `dot_words` 与 GPU 内核逐位一致；测试覆盖 1/31/32/33/100 跨字边界、四种符号组合、
  正负导轨不对称；GPU 测试在没有 adapter 时自动跳过。
- 实测（RTX 4060 Ti / Vulkan，5 批取中位数）：

  | 规模 | GPU 三值 | 同卡 f32 | CPU 位并行 | 加速 |
  |---|---|---|---|---|
  | 16M trit | **2.68 ps/trit** | 35.6 ps/元素 | 50.7 ps/trit | **13.3×** |
  | 1M trit | 40.6 ps/trit | 58.0 ps/元素 | 54.6 ps/trit | 1.4× |

  与内存带宽一致：三值 0.5 B/trit（4 个位平面 / 32 trit）vs f32 8 B/元素 = **16× 字节比**；
  1M 时数据进缓存、dispatch 延迟占主导，GPU 优势消失。
- 测试 4 条 + 1 条 GPU 测试；变异门 163 → **166**（少减一项、`+1` 写错导轨、标量参考改加法）。
- 边界：只测这一个 kernel，不是完整推理；没有与 int8/triton 内核对比；GPU 计时抖动大（已取中位数）。

### Added（发布图门 `scripts/publish_graph.py`）

- 新门：**任何 crate 都不许依赖未发布（`publish = false`）的 crate**。
  - 可发布 → 实验层：发布链断（红）；
  - 实验层 → 实验层：`cargo package --workspace` 会把实验层也一起打包，路径依赖没有版本 ⇒ 同样失败（红）；
  - 实验层 → 可发布：允许（绿）。
- 推论：**实验层必须自包含**；共享代码只能下沉到可发布 crate。`trit-net::format` 里 5-trit 打包
  之所以内联而不是复用 `trit-codec`，就是这条约束。
- 顺带检查可发布 crate 的 crates.io 必填字段（`description` / `license` / `repository`），
  并按依赖打印**发布顺序**（当前：17 个 crate / 11 个可发布）。
- `--self-test` 5 条断言：2 正例（可发布→可发布、实验层→可发布）+ 3 反证（可发布→实验层、
  实验层→实验层、缺 `license`），夹具是真实的两 crate workspace，跑的是真 `cargo metadata`。
- 接入 CI：新 job `publish graph (no dep on unpublished crates)`。
- 由来：本会话真实踩过两次（`trit-net → trit-codec` 让 `cargo package` 失败），现在由门强制。

### Added（三值点积的位并行实现 `trit-dot`）

- 新 crate `crates/trit-dot`（`publish = false`，不属于稳定 API）：
  - **二值（±1）点积**：`dot = n - 2·popcount(a XOR b)`——这才是「XNOR + POPC」能算的东西，
    每个 64 位块 **2 条指令**。
  - **三值点积**：`{-1,0,+1}` 一个 bit 装不下，必须双导轨（2 bit/trit）：
    `dot = Σa⁺b⁺ - Σa⁺b⁻ - Σa⁻b⁺ + Σa⁻b⁻`，每块 **4 AND + 4 POPC = 8 条指令**。
  - **块级稀疏跳过**：整块为 0 才跳过；**元素级稀疏救不了**——64 位块里几乎每块都有非零。
- 实测（1M 元素、同机、20 次平均）：

  | 方式 | 时间 | ns/元素 |
  |---|---|---|
  | 三值位并行 | 22.8 µs | 0.0217 |
  | 二值 XNOR+POPC | 6.4 µs | 0.0061 |
  | f32 朴素（单累加器） | 451 µs | 0.430 |
  | f32 四累加器（公平对照） | 113 µs | 0.108 |
  | 三值 + 块级 90% 稀疏 | 13.8 µs | 0.0132 |
  | 三值 + 元素级 90% 稀疏 | 43.4 µs | 0.0414（**更慢**） |

  三值位并行相对**公平**的 f32 约 **5×**（相对朴素 f32 是 19.8×；「16×」只在拿朴素基线时成立）。
- 结论：**「XNOR+POPC 算三值点积」是误传**。准确说法是「二值点积 2 条指令/64 元素，
  三值 8 条」；GPU 晶体管不是三值的，三值始终是 2 bit 模拟（B 类）。
- 测试 6 条库 + 3 条 CLI；变异门 160 → **163**（二值公式丢系数、三值少减一项、稀疏漏负负项）。

### Added（三值浮点实验层 `trit-float`）

- 新 crate `crates/trit-float`（`publish = false`，不属于稳定 API）：`value = mantissa · 3^exponent`，
  尾数是平衡三进制整数、规格化后最高位 `±1`、按 `precision` 位舍入；实现 `+ - * neg` 与 `f64` 互转。
- 舍入：丢掉最低位时余数只有 `±1`（不到半个 ulp），所以平衡三进制的**最近舍入就是截断**。
- 实测（`trit-float stats`，2000 次随机）：位数 6/12/18/24 的最大相对误差都低于理论上界 `3^{1-p}`；
  **同比特预算下三值的误差上界恒为二进制的 3 倍**（`3^{1-p}` vs `2^{-p·log2 3}`）。
  即三值浮点**没有精度红利**，相同存储白丢约 **1.58 bit**。
- 要匹配 `f64` 的 53 bit 精度，三值需要 **35 位 ≈ 55.47 bit（多 2.47 bit）**。
- 测试 7 条库 + 3 条 CLI；变异门 157 → **160**（舍入改截断、加法恒丢弃小项、乘法指数相减）。
- 边界：浮点精度是**基数无关的 D 类**问题；三值的优势只在整数的基数经济性（`3/ln3 ≈ 2.73` 最小），
  不在浮点精度。未实现次正规数、溢出标志、NaN / ±Inf 的三值表示。

### Added（三值注意力 / 快权重记忆实验层 `trit-attn`）

- 新 crate `crates/trit-attn`（`publish = false`，不属于稳定 API）：
  - **注意力 = 快权重 FFN**：`attention` 与 `ffn_view` 是同一计算的两个写法——第一层权重是 `K`、
    第二层是 `V`，都由输入现场生成，激活是 `exp`，**隐层宽度 = 序列长度**。
    20 组随机用例上逐元素差 `<1e-6`，本机实测最大差 **0.0**。
  - **三值关联记忆（编码器 / 解码器）**：`TernaryMemory` 把「键 → 值」写成整数矩阵 `W += v kᵀ`，
    解码 `W q` 取符号，编码与解码全程没有浮点。
  - `recall_experiment`：32 条键值对、维度 16，**逐分量**召回率
    **三值整数记忆 0.760 / softmax 注意力 0.963 / f32 对照 0.725**。
- CLI：`trit-attn demo [--pairs N] [--dim D] [--seed N]`。
- 测试 7 条库 + 3 条 CLI；变异门 154 → **157**（softmax 不归一化、记忆编码符号写反、三值化不夹边界）。
- 边界：f32 对照**未做能量对齐**（三值键 `|k|²≈2d/3`，f32 均匀键只有 `d/3`），不能据此说三值更好；
  记忆容量随条目数线性退化，没有做归一化/正交化；这是离散玩具模型，不是 Transformer 实现。

### Added（`trit-net`：二进制模型格式 + STE 反向传播）

- **模型格式 `TRN1`**：`magic / version / 层数` + 每层维度 + 权重与偏置按 **5 trits/byte** 打包
  （`{-1,0,+1} ↔ F/X/T` 只是存储映射，不代表 0 是逻辑未知）。113 参数的模型 **26 B**，
  同模型 JSON 是几百字节；非法魔数 / 版本 / 截断 / 尾部多余 / `243..=255` 非法字节全部显式报错。
- **反向传播（直通估计器 STE）**：保留 f32 影子权重，前向用 `clamp(round(w), -1, 1)`，
  梯度直通写回影子权重，训练结束量化回 `{-1,0,+1}`。只支持 1 个隐层的 tanh MLP、`±1` 输入输出。
- CLI：`trit-net train [--epochs N] [--hidden N] [--lr F]`、`trit-net format encode|decode`。
- 读数：majority-of-5（32 样本、6 隐元）300 轮，损失 **0.5 → 0.0027**，
  **三值网络精度 1.00**（影子权重同样 1.00），导出模型 26 B。
- 测试：库 16（格式往返 / 坏输入 / 训练收敛 / 标签与层数校验）+ CLI 7；
  变异门 151 → **154**（不校验魔数、打包长度向下取整、梯度上升）。
- 边界：STE 是近似梯度；只支持单隐层；合成任务上的精度不代表真实数据集。

### Added（三值编解码实验层 `trit-codec`）

- 新 crate `crates/trit-codec`（`publish = false`，不属于稳定 API）：
  - **`pack`：5 trits/byte**。`3^5 = 243 ≤ 256`，密度 **1.6 bit/trit**；1000 trit 实测 200 B
    （双导轨 250 B，省 20%；文本 `0/1/X` 1000 B，省 80%）。`243..=255` 是非法字节，解码显式报错；
    尾部不足 5 个时按长度截断。
  - **`balanced`：平衡三进制整数**（`+0-`）。i64 全域需要 **41 位**
    （`(3^40-1)/2 < i64::MAX < (3^41-1)/2`）；往返覆盖 `i64::MIN/MAX`；空串 / 非法字符 / 溢出分别报错。
  - **`hamming`：GF(3) 上的 `[4,2,3]` 纠错码**。2 个消息 trit → 4 个码字 trit，
    校验位 `p = a + b`、`q = a - b`；穷举 9×4×2 = **72 种单符号错误全部纠正**，最小距离实测 3。
    2 个错误不保证（距离 3 的固有上限），文档写明。
- CLI：`trit-codec pack|unpack|balanced|hamming|stats`。
- 测试 13 条库 + 5 条 CLI；变异门 147 → **151**（打包丢位权、非法字节边界写成 `>`、
  第 4 列误差幅度取错、平衡三进制负号解成 +1）。
- 边界：打包**不是压缩**（固定 1.6 bit/trit，不随分布变化）；Hamming 只保证纠 1 个符号；
  平衡三进制只做整数，不含分数。

### Added（三值小部件实验层 `trit-machines`）

- 新 crate `crates/trit-machines`（`publish = false`，不属于稳定 API），三个独立小机器：
  - **`vm` 三值栈式解释器**：`Jz` 的条件是 `X` 时不猜，两条路都走；结论分成
    「所有路径同值 / 多个可能值 / 截断」，并给出每个输出的路径数。
  - **`ca` 三值一维元胞自动机**（半径 1、总istic）：`3^7 = 2187` 条规则可整片枚举。
  - **`snn` 三值权重脉冲神经元**：`{-1,0,+1}` 突触、整数膜电位、不应期；统计脉冲率与活跃比例。
- CLI：`trit-machines vm|ca|snn`，JSON 输出。
- 研究读数（本机）：CA 在 64 格 × 200 步下 2187 条规则 = 死 274 / 不动点 351 / 周期 690 /
  复杂 872，最大熵规则 #1031（1.5846 bit ≈ log2 3）；SNN 100 神经元时**全局同步驱动会让网络
  退化成一个神经元**（脉冲率只落在离散档位），无驱动时一次 kick 后即静默——要看到传播必须异质驱动。
- 测试 16 条库 + 4 条 CLI 集成；变异门 144 → **147**，三条新变异逐条实跑判红。
- 边界：VM 不做相关性分析（`X & ~X` 仍是 `X`，那是 #90 分析器的活）；CA 只做总istic；
  SNN 是离散时间玩具模型，不能当神经形态硬件的证据。

### Added（三值网络实验层 `trit-net`）

- 新 crate `crates/trit-net`（`publish = false`，**不属于稳定 API**）：三值权重 `{-1,0,+1}`
  的全连接网络，信号是逻辑三值 `{F,X,T}`。`analyze` 枚举 `X` 的所有补全，把每个输出判成
  **确定 / 未定（带两组见证）/ 超预算**，同时给出逐层区间（保守）结论用于对照。
- CLI：`trit-net infer MODEL.json --inputs 1,X,0 [--budget N] [--json]`。
- 研究读数（本机原型，12→8→1，单种子合成任务）：f32 94.75% → 三值权重 84.75%
  （非零输出 85.45%），参数 452 B → 28 B；输入含 `X` 时精确解析 k=1 覆盖 70.3%/确定时精度 90.7%、
  k=4 19.8%/94.9%、k=8 0.7%/100%；而逐层区间只敢确定 0–3%，**假 X 占 95.7–100%**
  （远高于电路的 28.7%，因为区间误差逐层放大）。
- 测试 8 条（取消相关性、见证可复算、预算边界、超预算不猜）+ 4 条 CLI 测试；
  变异门 141 → **144**，三条新变异逐条实跑判红。
- 边界：只做前向解析、不含训练；精确解析是 `2^k` 枚举（k > 12 直接报超预算）；
  权重三值是**量化**（B 类税），只有 `X` 语义是本仓的 A 类能力。

### Added（X-Pessimism 分析）

- `trit_circuit::circuit::analyze_x_pessimism`：把单遍传播给出的 `X` 分成**真 X**与
  **悲观 X**。真 X 给出一对见证赋值（一个补全得 0、一个得 1）；悲观 X 给出可解析的确定值
  （不可满足的那一边就是证明）。
- 用 SAT（`varisat`）查询「输出能否为 0 / 能否为 1」，把 `X` 主输入当自由变量；
  不再需要 `2^k` 次完整传播，`propagate_exact` 的 4096 预算不再限制验证规模。
- `support` = 结构扇入锥里的 `X` 主输入（保守可达集，不是最小反例）。
- `Const X` 无法用二值编码表达，返回 `CircuitError::NonBinaryConstant`。
- CLI：`trit circuit FILE --assign a=X,b=0 --pessimism [-f json]`；JSON 含 `kind`、
  `resolved`、两个 `*_witness`、`support` 与 `sat_calls`。
- 测试：7 条库测试（重汇聚互补、mux 数据相同、见证代入复算、锥隔离、已知输出不发 SAT、
  `Const X` 拒绝、60 组随机电路与 `propagate_exact` 逐输出对拍）+ 3 条 CLI 集成测试。
- 变异门 133 → **136**（单边可满足判成 1、跳过 X 判定、支持集不回溯扇入），三条逐条实跑判红。
- **修变异门的跨平台缺陷**：18 条锚点把 `file` 写成反斜杠路径，Linux 上（周常全量门）
  直接 `FileNotFoundError`。统一为正斜杠，并在跑测试前加 `preflight`：文件缺失、锚点出现
  次数 != 1、路径含反斜杠一次性报全并退出，不再跑一半才崩。
- 实测（研究用探针，未入库）：200 个随机电路 / 800 个输出里，单遍的 383 个 `X` 有
  **110 个（28.7%）是悲观 X**。

### Changed（去上帝模块 + 门禁加强）

- **`trit-schedule` 去上帝模块**：`checkpoint.rs`（1095 行 / 82 个函数）拆成
  `checkpoint/{mod,file,memory,index,lock}.rs` + `tests.rs`；`runtime.rs`（988 行）拆成
  `runtime/{mod,types,execute}.rs` + `tests.rs`。公开路径不变（`trit_schedule::checkpoint::*`、
  `trit_schedule::runtime::*` 与 crate 根重导出均保持），无新增公开 API。
- **新门：模块预算**（`scripts/module_budget.py`）：单文件 800 行预算；超预算必须登记在
  `scripts/module_budget_baseline.json`，且台账**只允许变短**——变胖、变瘦却没收紧、
  文件已降到预算内却没移除、台账里的文件已删除，四种都判红；`--update` 拒绝任何增长。
  当前登记 5 件：`map.rs` 842、`verilog/parse.rs` 861、`verilog/tests.rs` 1094、
  `cli/tests.rs` 1100、`bits.rs` 801。
- **门的自证**：`--self-test` 用脚本自己的 `LIMIT` 生成夹具，10 条断言（正例 5 / 反证 5），
  逐条覆盖上面四种红法与 `--update` 的两条路径。
- **CI**：新增 `module budget (god objects)` job（先跑自测再查台账）；`mutation` 新增 PR 触发，
  只跑改动过的 crate（`mutation_check.py --pkg`），周常全量保留。
- 变异门仍为 133 条：拆分只改路径不改语义，锚点逐条重定位并验证唯一命中。

### Added（0.12 静态三值调度）

- 新增 `trit-schedule`：静态 DAG 调度，支持 `True` / `False` / `Unknown` 依赖；
  `Unknown` 直接延迟并沿必需依赖传播；输出 worker、start/finish、makespan、关键路径。
- 多 worker 事件驱动列表调度；高优先级先排，确定性 tie-break。
- CLI `trit schedule tasks.json --workers N`（table/json/csv/markdown）；`sansheng` 重新导出。
- 测试：链、并行、优先级、未知依赖传播、可选依赖、环/重复/未知引用拒绝、JSON 输入。
- 变异门 96 → 100（未知依赖、延迟传播、关键路径、优先级排序）。
- 这是静态调度 MVP；runtime/抢占/取消/故障恢复不在本轮。

### Added（0.12 runtime）

- `trit_schedule::run_runtime`：依赖驱动的 worker runtime；`RuntimeTask` 闭包、
  `CancelToken` 协作式取消、失败/取消后停止派发、`completed/failed/cancelled/deferred` 报告。
- 不强制抢占运行中的任务；任务需自行轮询 token。
- 测试：依赖顺序、双 worker 重叠、join 等待全部前置、失败后不派发新任务、取消未启动任务、
  未知依赖、panic 转失败、环/零 worker 拒绝。
- 变异门 100 → 103（取消、依赖传播、失败停止）。

### Added（0.12 资源配额）

- `Resources { cpu, memory }` + `RuntimeConfig::with_capacity`；任务 `with_requirements`。
- 严格优先级准入：队首放不下时等待；运行中任务不抢占。
- 超过容量的单任务直接 `Unschedulable`；报告新增 `peak_usage`。
- 测试：6+6 超过 10 时串行、超容量拒绝、配额下高优先级先跑。
- 变异门 103 → 106（容量、占用累计、超容量拒绝）。

### Added（0.12 可恢复任务协议）

- 新增 `ResumableTask::step`：步骤边界 `Yield` + 任务对象内检查点状态。
- `run_resumable` 单线程步进式参考执行器：高优先任务在步骤之间抢占低优先任务，
  低优先任务放回 ready 但状态保留，之后继续；资源配额同样生效。
- 报告 `preemptions`、`steps`、`peak_usage`；取消/失败后不执行新 step。
- 测试：抢占后从原步骤继续、优先级顺序、取消、失败/未知依赖、环/超容量。
- 变异门 106 → 109（抢占状态、优先级过滤、失败清理）。

### Added（0.12 threaded resumable + metrics）

- `run_resumable_threaded`：worker 在 `Yield` 边界抢占，任务对象保留；资源/优先级/取消沿用协议。
- `RuntimeMetrics` 统一指标：`started`（按任务去重）/`completed`/`failed`/`cancelled`/
  `deferred`/`preemptions`/`steps`/`peak_usage`；闭包 runtime 与 resumable runtime 都有。
- 测试：线程抢占后状态续跑、双 worker 重叠、取消/指标、单线程参考仍通过。
- 变异门 109 → 112（线程抢占状态、优先级过滤、started 去重）。

### Added（0.12 metrics export）

- `RuntimeMetrics::to_prometheus()` / `to_prometheus_with(prefix)`：Prometheus/OpenMetrics
  文本导出；报告提供 `metrics_prometheus()` 便捷方法；空前缀回退到 `trit_schedule`。
- 导出字段与 `RuntimeMetrics` 一致；峰值拆为 `peak_cpu` / `peak_memory`。
- 测试 `prometheus_exports_all_fields`；变异门 112 → 114。

### Added（0.12 checkpoint protocol）

- `CheckpointTask::save_state` + `TaskRestorer::restore`：可序列化状态协议，编码格式由任务决定。
- `Checkpoint` 带 `version`，`CheckpointStore` 按任务 id 覆盖保存/读取/删除/恢复。
- `CheckpointedTask` 包装器：每次 `Yield`/`Done` 后写入共享存储，可直接用于现有
  单线程/线程 runtime。
- 测试：检查点往返、存储覆盖/删除、版本拒绝、runtime 每步落盘并恢复。
- 变异门 114 → 117（版本校验、空状态、恢复校验）。

### Added（0.12 checkpoint files）

- `FileCheckpointStore`：版本化目录 `<root>/v1/<hex(task_id)>/<generation>.json`；
  临时文件 + rename 到新代文件（不覆盖），加载取最新代；`prune`/`remove`/`len`。
- 任务 id 十六进制转义，路径分隔符/unicode 不落到文件名；临时文件不参与加载。
- 测试：往返/代号递增、id 转义、prune/remove、版本拒绝；crate 33 → 36。
- 变异门 117 → 120（保存版本校验、加载最新代、prune 保留）。

### Added（0.12 checkpoint index）

- `CheckpointIndex` / `CheckpointEntry`：扫描 `v1/<hex>/`，列出 task id、最新代、状态字节数；
  按 task id 排序，只读元数据，不加载状态。
- `FileCheckpointStore::index()` / `task_ids()` / `index_json()`；`CheckpointIndex::get()`。
- 测试：多任务排序、unicode/slash id、最新代、空索引；crate 36 → 37。
- 变异门 120 → 123（id 解码、排序、state_bytes）。

### Added（0.12 checkpoint lock）

- `CheckpointLock`：`try_lock()` 原子创建 `v1/.lock` 目录，`Drop` 自动释放；
  `lock_timeout()` / `lock()`（默认 5 秒）带超时，占用时返回 `LockBusy`。
- 不自动包裹 `save`/`load`；调用方在临界区显式持有，避免改变现有单写者调用路径。
- 测试：排他、超时、释放后可重新获取；crate 37 → 38。
- 变异门 123 → 125（已存在分支、Drop 释放）。

### Added（0.12 checkpoint resume）

- `CheckpointSource`（内存/文件存储实现）+ `TaskFactory` + `ResumableSpecDef`。
- `restore_specs(defs, source, restorer, factory)`：有检查点则恢复，没有则新建；
  返回值可直接交给 `run_resumable` / `run_resumable_threaded`。
- 测试：内存源从第 3 步恢复只再走 7 步、文件源恢复、无检查点时用 factory。
- 变异门 125 → 127（忽略检查点、无检查点不建任务）。

### Added（0.12 exact propagation）

- `propagate_exact`：相关性感知的精确 X 传播。把 `X` 当未知 0/1，枚举完成并合并可能值；
  `a & ~a`（`a=X`）得到 `0`，修正单遍 Kleene 的假 X。
- 支持部分赋值（缺省主输入按 `X`）；未知输入 `k` 个需要 `2^k` 次完成。
- 超预算时回退到单遍保守结果，`exact=false`、`completions=0`；`DEFAULT_EXACT_BUDGET=4096`。
- 测试：重汇聚相关性、部分赋值、遮蔽、超预算、全已知输入；circuit 166 → 171。
- 变异门 127 → 130（冲突合并、预算、缺省 X）。

### Added（0.12 tri-state / ternary topology）

- `TriState`（0/1/Z）与 `TriStateConflict`：`Z` 是总线单位元，0/1 同时驱动报冲突；
  `TriStateBus` 逐位解析并校位宽；`to_ternary()` 把 `Z -> Unknown`。
- `TernaryTopology`：边带 True/Unknown/False 的有向图三值可达性；路径取 min、节点取 max，
  False 边断开；固定点迭代处理环。
- 测试：解析表/冲突、字符与 Ternary 转换、总线逐位/位宽、True 链、Unknown 传播、False/环；
  `trit-core` 67 → 73。
- 变异门 130 → 133（Z 单位元、总线宽度、拓扑 min/max）。

## [0.11.0] - 2026-10-23

0.10 与 0.11 合并发布：并行/共享 CNF、SDF per-instance、Verilog 参数化、CLI 硬超时，
以及 0.10 的桥接 SAT、映射面积、STA I/O 约束、层级/向量 Verilog。

### Changed（发布工程）

- workspace 版本 0.9.0 → **0.11.0**；新增每周 `quality` 工作流：`cargo audit` +
  8 个 fuzz 目标各 500 次 smoke。

### Added（SDF per-instance）

- **门实例元数据**：`Gate.instance`（`u1` / `u1.u2`），层级展开时写入；JSON 可选字段，
  顶层门为 `None`。
- **SDF 实例对齐**：`parse_sdf_instances` / `timing_model_from_sdf_for`；按 `INSTANCE` 匹配
  `Gate::instance`（容忍 `top.u1` 前缀），实例最坏 IOPATH 优先于 celltype；匹配不到回退 celltype。
- **STA**：`TimingModel::instance_delays` + `arc_delay_for_gate` / `output_arrival_for_gate`；
  同一实例的 rise/fall/条件弧仍合并为一个最坏值，不做引脚级差异。
- 测试：两实例 2/5 周期分别到达 y1/y2；SDF 前缀匹配；CLI `timing --sdf`。
- 变异门 94 → 96（不按实例覆盖、实例路径只做全等）。

### Added（0.11 硬超时）

- **全局 `--timeout-ms N`**：CLI 重新执行自身为子进程，超时 `kill` 并退出 **124**；
  子进程通过 `TRIT_TIMEOUT_CHILD=1` 跳过包装，避免递归。可兜住 ATPG/验证/REPL 等任意子命令。
- 这是进程级硬超时，不是 varisat 单故障取消；库 API 仍无中断句柄。
- 测试：`repl` 挂起时 200 ms 超时，断言退出码 124 且 5 s 内结束。

### Added（0.11 Verilog 参数）

- **参数化模块**：模块头 `#(parameter W = 8, ...)`、实例覆盖 `#(.W(4))` / `#(4)`；
  参数表达式支持 `+ - * / %`、括号与前置负号，用于位宽/下标/实例连接。
- 解析前做源码特化：每个参数组合一个唯一模块副本（上限 256），再由现有解析/层级展开处理；
  根模块保留原名，子模块名带 `__pN`。
- 明确限制：不支持模块体 `parameter`/`localparam`、`signed`/实数/字符串参数、参数化端口类型。
- 测试：默认值、命名/位置覆盖、父参数下传、同模块多参数实例、算术表达式、未知覆盖/体参数拒绝。
- 变异门 90 → 94（忽略覆盖、不替换、不归一化位宽、位置覆盖错位）。

### Added（0.11 共享 CNF）

- **`atpg --sat --shared-cnf`**：每个 worker 只建一份 good/faulty CNF，每个故障一个 selector；
  mux 链注入引脚常量，顺序 at-most-one 保证一次只激活一个故障。按 512 个故障切块，
  避免单 solver selector 过多导致的病态慢例。
- 默认逐故障路径不变（见证稳定）；共享路径覆盖/可测性一致，但见证向量可能不同。
- 10k 门 / 40k 故障、8 线程：**2.6 s**；3000 门 0.26 s；800 门 0.08 s（本机 24 线程）。
- 测试：共享路径与逐故障在随机小电路上对拍覆盖率/可测性/可观测性，并用 `verify_sat_patterns`
  验证测试向量真正检出故障；CLI `--shared-cnf` 覆盖率对拍。
- 变异门 87 → 90（候选不注入、mux 选反、不假设 selector）。

### Added（0.11 SDF-lite 互操作）

- **SDF 读取**：`trit_circuit::sdf::{parse_sdf, timing_model_from_sdf}`；`trit timing` /
  `trit check` 增加 `--sdf FILE`（与 `--library` 互斥）。只取 `CELL/CELLTYPE/DELAY/ABSOLUTE/
  IOPATH`，rise/fall 与 `min:typ:max` 取最坏值、`COND` 合并、向上取整；按 celltype 聚合，
  `TIMESCALE` 不换算，未知 celltype 忽略，无可识别延迟时报错。
- 测试：语法/缺延迟/负值拒绝、跨实例与条件弧最坏值、`min:typ:max`、CLI `timing`/`check`。
- 变异门 83 → 87（新增：取最小值、floor、跨实例取最小、放行负值）。

### Added（0.11 并行 SAT）

- **并行 ATPG / 桥接 SAT**：`generate_tests_sat_with_jobs` 与
  `generate_bridge_tests_sat_with_jobs`；CLI `atpg --sat --jobs N`、`bridge --sat --jobs N`
  （`0` = 自动，`1` = 串行）。按故障/桥接对分块，`std::thread::scope` 独立 CNF/solver，
  按原顺序合并，输出与串行逐字段一致；wasm32 强制串行。
- **策略断言并行**：`trit-core::parallel` 下沉为公共并发原语；`Policy::verify_with_bdd_config_and_jobs`、
  CLI `verify --jobs N`。前提只检查一次，断言顺序不变。
- **减少重复工作**：SAT CNF 复用调用方算好的拓扑序，`Cnf::new_with_order`；不再每个故障重跑
  `validate` / `topological_order`。
- 测速（本机 24 线程）：800 门串行 6.6 s → 2.5 s（拓扑序复用），`jobs=8` 0.37 s（并行 6.97×）；
  3000 门 34.4 s → 4.68 s（7.36×）；2000 条策略断言 0.19 s → 0.07 s。
- 测试：并行/串行逐字段对拍（ATPG、桥接、策略、CLI JSON）、顺序与错误传播。

### Added（0.10 STA I/O 约束）

- **时序 I/O 约束**：`TimingConstraints { input_arrival, output_required }`（JSON，拒绝未知字段/名字）；
  `analyze_timing_with_constraints` 与 `critical_paths_with_constraints`；`trit timing` / `trit check`
  增加 `--constraints FILE`，`--paths` 从输入到达时间起算。未列出的输入按 0、输出按 clock
  （省略 clock 时按关键路径），保持旧行为。
- 测试：到达/required/slack、逐输出 required、未知名字拒绝、JSON 往返、CLI `timing`/`check`。
- 变异门 80 → 83（新增：忽略输入到达、忽略逐输出 required、`--paths` 忽略输入到达）。

### Added（0.10 桥接 SAT + 映射面积）

- **桥接故障 SAT**：`trit bridge --sat` 对每个桥接构造 good / faulty CNF，要求两端在 good 下
  取值不同且至少一个主输出不同；SAT 给见证，UNSAT 证明不可测。支持 **≤64 主输入**；
  默认枚举路径仍为 ≤10。`BridgeReport` 新增公开字段 `sat`；不做 X 压缩。
- **映射面积**：`CellTiming` 支持 `{delay, area}` 与 `{delay, arcs, area}`；
  `CellLibrary::area`；`--area-first` 改为按 area 选择（省略按 1，旧单元库行为等价于单元数最少）。
- 测试：桥接 SAT 与枚举逐桥接对拍、20 输入、CLI `--sat`；映射 area 选择与 JSON 往返。
- 变异门 76 → 80（SAT 锚点迁移到共享 CNF，新增桥接 3 条 + 映射 1 条）。

### Added（0.10 SAT ATPG）

- **SAT-based ATPG**：`trit atpg --sat` 对每个门引脚故障构造 good / faulty CNF，
  要求至少一个主输出不同；SAT 给出测试向量，UNSAT 证明不可测（`UntestableReason::Proven`）。
  上限 64 个主输入（`Fault.witness` 仍为 `u64` 掩码）；`GateKind::Xor` 按代数编码
  （Ł3→OR，K3/B3→AND）。不做 X don't-care 压缩，也不证明测试集最小；未设单故障求解超时。
- `AtpgReport` 新增公开字段 `sat`（外部结构体字面量需补 `..Default::default()` 或改用构造器）；
  `CircuitError` 新增 `Sat` / `NonBinaryConstant`；`trit-circuit` 增加 `varisat` 依赖。
- 测试覆盖：随机小电路与枚举逐故障对拍（三种代数）、20 输入、恒 1 门的 UNSAT、
  常量 X 拒绝、65 输入上限、CLI `--sat`。
- 变异门 72 → 76（新增 4 条 SAT 规则）。

### Added（0.10 层级）

- **Verilog 多模块实例**：`parse_verilog` 现在解析一个文件里的多个 `module` 并做层级展开。
  连接支持 `.port(signal)` 与按端口顺序的位置连接；端口必须全部连接，重复连接报错；
  未知模块、递归、多顶层、重复实例名都返回错误。
- **层级模式支持 `[N:0]` 向量端口**：整条 bus 按位展开；同方向时子模块 `p[i]` 对应父模块 `a[i]`，
  跨方向按位序映射；宽度不匹配、把向量端口接到单比特、把标量端口接到整条 bus 都明确报错。
  单模块且无实例时保留原有完整向量行为。
- **`[0:N]` 升序向量范围**：解析、导出与 JSON 都保留方向；Verilog 位序（左界=MSB）贯通
  `assign`、拼接与层级跨方向连接。其它范围（非零下界）明确拒绝。
  `Bus` 新增公开字段 `ascending`（外部结构体字面量需补 `..Default::default()` 或改用构造器）；
  网表 JSON `format` 1 → 2，旧 `format: 1` 仍接受并按降序解释，更高版本拒绝。
  内部 `wire [N:0]` / `[0:N]` 声明也写入 `Circuit.buses`（此前只记录端口），导出按声明宽度与方向重组。
- 展开后的内部信号名加 `实例名__` 前缀；与用户信号冲突时显式报错，不静默复用。
  门数上限 10 万、嵌套 256 层，均在展开过程中检查。
- `hierarchy` 模块新增 17 条回归，另加 6 条 `[0:N]` 升序回归（解析/导出/JSON、位序、
  跨方向赋值、字面量、内部 wire、层级跨方向）。
- 变异门 62 → 72（新增 10 条：hierarchy 6 条 + 升序位序 4 条）。

### Docs

- **三进制四类边界**：新增 [docs/TERNARY-BOUNDARIES.md](docs/TERNARY-BOUNDARIES.md)，
  把原生三进制红利（A）/ 二进制模拟税（B）/ 生态路径依赖（C）/ 基数无关复杂度（D）
  分开；README「范围」与 DESIGN「非目标」已引用。后续结论、评审与路线图必须先归类。

## [0.9.0] - 2026-10-23

结构模型与 Verilog 前端增量：向量元数据、尺寸常量、JSON 版本；含 `Circuit` 新字段
（minor 允许破坏性调整）。

### Added（0.9 结构模型）

- **向量元数据**：`Circuit.buses` + `Bus { name, width }` 保留 `[width-1:0]` 声明；
  Verilog 解析填充，导出优先按声明宽度重组（允许只建模部分位），JSON 往返保留；
  空映射不序列化，旧 JSON 可直接读取。`CircuitError` 新增 `InvalidBus` / `BusNameConflict`。
  注：`Circuit` 新增公开字段，外部结构体字面量需补 `..Default::default()`。
- **Verilog 尺寸常量**：`<width>'<b|o|d|h><digits>`，支持 `_` 分隔与 b/o/h 的 `x`/`z`，
  不足高位补 0、超出截断高位；十进制常量限 128 位。尺寸常量可参与拼接与按位运算。
  变异门 59 → 62。
- **网表 JSON 版本字段**：`to_json` 写入 `format: 1`；旧 JSON（无 `format`）按 0 接受，
  更高版本明确拒绝。

## [0.8.2] - 2026-10-23

硬化补丁：修掉 Circuit 的静默常量默认，补齐结构校验，收紧 CI 与模块边界；无新增功能。

### Changed（0.8.2 硬化）

- **Circuit 结构不变量**：`validate` 拒绝常量门缺 `const_value`、非常量门带 `const_value`、
  空信号名与重复主输出；`Circuit::validate` 公开。`CircuitError` 新增四个变体
  （`ConstValueMissing` / `ConstValueUnexpected` / `EmptySignalName` / `DuplicateOutput`）。
- **`trit-bdd` 去上帝模块**：`lib.rs` 1059 行拆成 `encode` / `order` / `equiv` / `quantify`；
  公开路径不变。
- 所有 crate 启用 `#![forbid(unsafe_code)]`（当前全仓无 `unsafe`）。
- CI 新增 `cargo package --workspace --no-verify --locked` 可发布性门。
- 变异门 53 → 56（新增 Const 取值/空信号名规则）。

## [0.8.1] - 2026-10-23

评审修复收尾 + 三项芯片侧能力 + 按模块拆分上帝文件；三值语义不变。
本版含 **API 变更**（错误类型与若干返回 `Result` 的入口），升级前请读下面的 Changed 段。

### Changed（评审修复第二批，含 API 变更）

- **Verilog 错误结构化**：`VerilogError` 从 `Message(String)` 改为
  `{ kind, line, message }`，`kind ∈ Syntax / Unsupported / Limit / Export`；
  body 语句错误带 1 起行号（注释剥离保留换行），导出错误归 `Export`。
- **拆掉 circuit 模块环**：`validate` 独立到 `circuit/validate.rs`，`sim` 不再依赖 `analyze`；
  `sim → validate → graph`、`analyze → sim/validate/graph` 单向依赖。
- **ATPG 去上帝模块**：`atpg.rs`（1042 行）拆成 `atpg/{mod,compact}.rs` + `tests.rs`；
  `generate_tests` / `AtpgReport` / `CoverageStat` / `TestPattern` 公开路径不变。
- **Circuit 去上帝模块**：`circuit.rs`（1493 行）拆成 `circuit/{types,sim,analyze,graph}.rs`
  + `tests/test_support`；`Circuit`/`CircuitError`/`analyze`/`propagate` 等公开路径不变。
- **CLI 去上帝模块**：`trit-cli/src/main.rs`（2091 行）拆成 `commands/` 下 15 个命令模块，
  main 只保留 CLI 定义、分发与共享助手（约 590 行）；命令行为与参数不变。
- **Verilog 去上帝模块**：`verilog.rs`（1537 行）拆成 `verilog/{mod,expr,parse,export}.rs`
  + `verilog/tests.rs`；`parse_verilog` / `to_verilog` / `VerilogError` 公开路径不变。
- **设计去重**：ATPG 与桥接故障的精确/贪心集合覆盖抽到私有 `coverage` 模块，
  `TestPattern` 抽到 `pattern` 模块；`bridge` 不再直接调用 `atpg` 的私有实现，
  两套覆盖策略不再各写一份。公开 API 路径保持不变（`atpg::TestPattern` / crate 根导出）。
- 解析器在构造期扣预算：右结合蕴含链改为迭代收集再右折，扁平二元链按实际 AST 深度拒绝；
  新增输入字节上限（1 MiB）与节点预算错误；`Parser::parse_with_limits` 可指定预算。
- `Evaluator::assignment_count` / `chunk_planes` / `evaluate` 改为可失败（`Result`），
  `3^n` 用 checked 计算，新增 `Limits::max_rows` 与 `evaluate_within`。
- `trit_eval::{truth_table, equivalent}`、`trit_belnap::{table, equivalent}`、
  `Circuit::combination_count` 改为返回 `Result`；`analyze` 在 `3^n` 循环前拒绝 >16 个主输入。
- `TriVec` 的 `PartialEq/Eq` 改为按值比较（全零 unknown 平面等同 `None`）；写入清除最后一个
  `Unknown` 时规范化平面；新增 `try_from_binary` / `try_get` / `try_set` / `try_word`。
- `BitPlane` 的 `with_bit` / `bit` 明确为环绕语义，新增 `checked_with_bit` / `checked_bit` /
  `TriBit::checked_get`。
- `Ternary::{is_false,is_true}` 文档改为精确匹配（`X` 两者皆假）；新增 `Expr::try_var` 拒绝
  保留常量 token/非法标识符，`Display` 不再暗示任意变量可无损往返。
- 赋值编号：新增 `try_assignment_index` / `try_decode_assignment`（重复变量、超出 `3^n` 域、
  u64 溢出可区分）；编码不再因末位多余的 `3^n` 计算而过度拒绝。
- 策略：展开时按节点/深度预算（`ExpandBudget` / `DefinitionDepth`），完整定义图（含未引用环）
  先预检；`binary` 只允许原始变量，定义名拒绝保留 token，重复 binary 报错；
  上层错误携带 `LimitError` / `SymbolicError` / `BddError`，不再压成字符串。
- BDD 预算明确为软限额（每次运算返回后检查，峰值可超）；新增 `Quantified::try_is_valid` /
  `try_counterexample`；README 说明硬隔离需进程/worker。
- 策略反例文档改为“完整赋值模型，不做最小化”。
- `CellLibrary.cells` 的值类型改为 `CellTiming`（`Delay(u32)` 或 `Arcs { delay, arcs }`）；
  `TimingModel::from_json` 返回 `TimingModelError`，并做键归一化与弧数校验。

### Fixed

- 评审修复（第一批）：Verilog 括号倒置不再 panic；多输入 NAND/NOR 改为“先合并再反相”；
  XNOR 不再双驱动；临时信号名全模块唯一；强加 `Xor` 导出 Verilog 明确拒绝；
  `check` 无法执行覆盖率检查且用户指定阈值时 FAIL；单元库键规范化并拒绝重复；
  时序/映射累计改用 u64 检查，溢出返回错误；ATPG X 位改为联合填充验证；
  `minimum_cover` 预算耗尽不再宣称最优；`decode_binary` 超过 64 输入返回错误。

### Added

- ATPG X 合并压缩：两个向量若存在一种填充能覆盖“删掉它们后会失去”的唯一故障，就合成一个；
  `inputs` 仍给出具体 0/1 填充，CSV 导出即用该填充。
- `AtpgReport::x_coverage`（含 X 向量覆盖的可测故障比例）与 `merged`（压缩省下的向量数）；
  表格、JSON、WASM 同步输出。
- 多比特 Verilog 子集：`input/output [N:0]` 向量端口按位展开为 `a[3]`… 单比特网；
  支持位选 `a[3]`、拼接 `{a, b}`、等宽逐位向量赋值 `assign y = a & b;`，
  导出时按连续位重新分组为向量端口。只支持 `[N:0]` 组合逻辑；拒绝 `[0:N]`、复制语法、
  位宽不匹配、非连续向量位。CLI `netlist --from verilog --to verilog` 可往返。
- 桥接故障 ATPG：`BridgePair` / `BridgeMode`（wired-AND、wired-OR）→ `generate_bridge_tests`，
  在二进制向量上判定短路可测性并给出覆盖向量；CLI `trit bridge` 支持 `--pairs`/`--max-pairs`/JSON。
  空列表与未知网络直接报错，规模超限拒绝；逻辑级模型，不建模模拟/布局效应。
- Liberty-lite 引脚弧：`TimingModel.arcs`（按 `A1`/`A2` → `Y`）与 `CellLibrary` 的
  `{"delay": N, "arcs": [...]}` 写法；`trit timing` / `check` / `map` 按输入引脚弧计算到达时间，
  慢弧会改变映射选实现。键大小写归一化；未知门、规范化重复、弧数与元数不符都报错。
- 复审测试缺口（§3.3）：`trit-core/tests/structure.rs` 用 proptest 生成不同形状的 AST，
  验证 `to_infix → parse` 的结构/深度/节点数守恒、扁平链与右结合蕴含链的深度边界、
  任意文本不 panic；`trit-policy` 新增展开预算精确性/深度单测与稳定 smoke。
- fuzz 目标补到 8 个：新增 `expr`（解析往返）、`policy`（小策略验证）、`netlist_json`
  （JSON 往返 + 坏电路导出）；`fuzz/Cargo.toml` 的 `trit-circuit` 依赖路径修正为
  `../crates/trit-circuit`。
- ATPG 端到端回归：用 `generate_tests` 实际产出的覆盖向量验证 don't-care 的每个联合填充
  仍检出故障，补上报告 §3.1 指出的“只证明 helper”缺口。
- AIGER ASCII 子集读写：`parse_aiger` / `to_aiger`（`L = 0` 组合逻辑，AND + 取反编码）；
  CLI `netlist --from/--to aiger`；0/1 激励上的往返等价。
- BLIF 子集读写：`parse_blif` / `to_blif`（`.model/.inputs/.outputs/.names`，拒绝 `.latch`）；
  CLI `netlist --from blif` / `--to blif`；往返等价在 0/1 激励上验证。
- Verilog `assign` 支持 `^` / `~^` / `^~`、括号与常量字面量，按 Verilog 优先级解析；
  `^` 映射为经典异或（`XorClassic`），不是强加三值 `Xor`。
- 负载感知时序：`TimingModel.load_delay`（每个扇出追加延迟）进入 `analyze_timing` / `critical_paths`；
  `to_cells_with_load` 在选实现时按扇出计负载；CLI `timing` / `check` / `map --load-delay`。
- fuzz：`fuzz/` 下 cargo-fuzz 目标（netlist / Verilog / ATPG）+ CI 可跑的稳定 smoke；
  `scripts/bench.sh` 与 `trit-circuit` 的 `core` bench（解析/时序/映射/ATPG）。

## [0.8.0] - 2026-10-22

芯片侧能力的一批新入口：结构化 Verilog、STA-lite、技术映射（NAND-only 与时序驱动）、
设计检查流水线、X-aware ATPG；三值语义不变。

### Added

- 时序驱动技术映射：`CellLibrary` + `to_cells`，每门在直接单元与 NAND 展开间按到达时间选实现
  （最长路径 DP，关键路径最优）；`MapStrategy::Area` 优先单元数。CLI `map --cells` / `--area-first`。
- `trit atpg --format csv`：`pattern,<输入…>,covered`，便于导出给外部仿真器。
- `trit_circuit::netlist`：`parse_netlist` / `to_netlist` 从 CLI 下沉到库（可往返）。
- WASM `atpg_js(netlist, algebra)`：浏览器里直接跑 ATPG，返回 JSON。
- `satisfiable_bdd_with_order` / `premise_counterexample_bdd_with_order`，策略验证 `--bdd-order`。
- 网表 JSON 互转：`trit_circuit::netlist::{to_json, from_json}` 与 CLI `trit netlist`。
- `analyze_faults`：ATPG 不再做 `3^n` 输出域枚举；全向量搜索收紧到 ≤8 个主输入。
- 索引化仿真器（`Simulator`）：故障分析/覆盖矩阵不再逐向量建 `BTreeMap`。
- 结构化 Verilog 子集：`parse_verilog` / `to_verilog`，CLI `netlist --from verilog --to verilog`。
- 新增 `GateKind::XorClassic`（真实布尔异或，X 传播）：网表 `XOR2`，Verilog `xor` 原语，NAND 四门映射。
- `trit map`：降到 NAND + 常量；三值强加 `Xor` 因缺 X 检测单元明确拒绝。
- Verilog `assign` 里的 `^` 仍拒绝（请用 `xor` 原语）。
- `trit check`：结构 + 时序 + X 传播 + ATPG 覆盖率，`--clock`/`--min-coverage` 失败即退出 1。
- ATPG X don't-care：每个向量标出可任意填 0/1 的输入位（`dont_care` / `dont_care_bits`）。
- STA-lite：`analyze_timing` / `critical_paths` 与 `trit timing`（required/slack、违例、top-N 路径）；
  `TimingModel` 支持按门类型延迟与 JSON 库。
- `circuit` / `atpg` 支持 `--from text|json|verilog`。
- `BddRequest`（顺序 + 预算）与 `*_with` 系列 API；CLI `equiv/quantify/verify --bdd-max-nodes`。
- `verify --bdd-order best`：四种排序取断言 BDD 节点总数最少者。
- ATPG 随机电路属性测试：每个向量必须真的检出它声称覆盖的故障。

## [0.7.0] - 2026-10-22

ATPG 与 BDD 的一批新能力；`Fault` 结构体有破坏性变更。

### Added

- `trit atpg <netlist>` 与 `trit_circuit::generate_tests`：门引脚 stuck-at 的测试向量生成
  （贪心集合覆盖）+ 覆盖率报告（table / JSON）。
- `AtpgReport::by_gate_kind`：覆盖率按门类型拆分；`exhaustive` 标记是否做了全 `2^n` 搜索；
  `optimal` 标记是否为精确最小覆盖（故障 ≤ 64、向量 ≤ 256 时用 DFS + 下界剪枝）。
- `Fault::witness`（首个二进制区分向量）、`Fault::reason`（`redundant` / `unobserved` / `masked`）
  与 `Circuit::decode_binary`；`trit circuit` 同步输出原因。
- `VarOrder`（appearance / reverse / frequency / frequency-asc）、`order_variables`、
  `equivalent_bdd_with_order`、`equivalent_bdd_best`、`quantify_bdd_with_order`、`quantify_bdd_best`；
  CLI `equiv --bdd-order` 与 `quantify --bdd-order`。
- `BddLimits`（默认 100 万节点/单个 BDD）与 `equivalent_bdd_with_limits`；超限返回
  `BddError::BudgetExceeded`，所有 BDD 构造点都做 O(1) 检查。

### Changed（破坏性）

- `Fault` 新增 `witness` 与 `reason` 字段：用结构体字面量构造 `Fault` 的代码需要补字段。
- stuck-at 可测性判定改用 `2^n` 二进制测试向量；`X` 输入不是物理测试向量，`analyze` 的
  `observable` 与 ATPG 口径一致。
- 主输入超过 16 时故障分析返回 `CircuitError::TooManyInputs`。

## [0.6.0] - 2026-10-22

### Added

- `trit-core::Limits` + `check_expr` / `check_vars`：库级统一预算（深度 / 节点 / 枚举变量数）。
- `trit-eval`：`eval_within` / `truth_table_within` / `equivalent_within`。
- SAT、BDD、策略入口先做深度/节点检查，超限返回 `Limit` 错误；CLI 与 WASM 共用同一套预算。
- `scripts/mutation_check.py`：12 条关键规则的变异门（12/12 被抓）；新增每周 CI。

### Fixed

- 变异门发现 `strong_and` / `strong_add` 的 K3 断言过弱（只测了 `X⊗X` 与 `1⊗1`），
  补上 `1⊗0 = 0`、`0⊗1 = 0` 的固定用例。

## [0.5.0] - 2026-10-22

### Changed（破坏性）

- `CircuitReport::untestable_faults` 替换为 `faults: Vec<Fault>`：逐**门引脚**、逐极性（SA-0/SA-1）。
  `trit circuit` 的 JSON 字段同步为 `faults`（含 `input_index` / `stuck_at` / `observable`）。

### Added

- `trit-core`：解析嵌套上限 256（`ParseError::TooDeep`）。
- CLI 入口规模护栏：`table` / REPL 最多 10 个变量，网表分析最多 10 个主输入。

### Fixed

- 故障注入从整条 net 改为 gate-pin：多扇出不再串扰，SA-0 / SA-1 分开报告。
- `TriVec::from_ternary` / `and_assign` / `or_assign` 的纯二值路径不再分配 Unknown 临时平面。
- GitHub Action 全部固定到 commit SHA（checkout / rust-toolchain / rust-cache / artifact / gh-release），
  MSRV 1.88 同样固定。
- 文档：DESIGN 路线补 v0.5；CONTRIBUTING 的 `tla-core` 路径改为 `trit-core` / `trit-eval`；
  `bulk_bench` 把缓冲区分配移出 move-only 计时；README 明确双导轨只是进程内表示，不是字节协议。

## [0.4.2] - 2026-10-22

### Fixed

- `trit-circuit`：`analyze` / `propagate` / 故障模拟改用真正的拓扑序（此前把全部门输出预置为已解析，
  等于按声明顺序求值）；主输出必须有驱动。
- `trit-eval`：修正三条不保真化简（`a | (a | b)`、K3 `a ⊕ a`、K3/B3 `a ≡ a`）；
  `semantically_equal` 改用真实 `3^n` 等价，不再比较变量顺序敏感的 64 位指纹。
- `trit-bdd`：`is_valid` 相对合法自由变量域判定；`satisfiable_bdd` 与合法域相交，不再把非法导轨 (1,1)
  当成见证；新增 `premise_counterexample_bdd`。
- `trit-policy`：`assumptions` 按“前提确定为 True”过滤（SAT/BDD 新增 `premise_counterexample*`），
  不再用三值 `premise ⇒ assertion` 恒真替代；schema 拒绝未知字段；空断言不再静默通过。
- `trit-core`：`TriVec::from_binary` 规范化多余字与尾部 padding；`TriBit` 的 B3 或不再产生禁止的 `(1,1)`。
- `trit-cli`：CSV/Markdown 每个变量独立成列（此前整行拼成一格、表头拆两行）；REPL `:algebra` 真正切换语义；
  `simplify` 若结果不等价则非零退出，不再 fail-open。
- `action`：`policy` / `engine` 经 env 传入并校验 engine 白名单，不再把输入拼进 shell 脚本。

## [0.4.1] - 2026-10-22

### Fixed

- `trit-sat` / `trit-bdd` 的二值快速路径把 `⊕` 编成经典异或，与解释器不一致：K3/B3 的 `⊕` 在 `0/1` 上是 `∧`，
  Ł3 上是 `∨`。常量（`1 ^ 1`）与 `--binary` 变量都会触发，例如 `trit equiv --symbolic "1 ^ 1" "1"` 误报不等价。
  SAT 新增 `binary_strong_add`，BDD 的二值分支按代数选 `∧`/`∨`，并加回归测试。
- 新增 `trit-sat` 属性测试：随机公式上 SAT 与 `3^n` 穷举逐例一致，且 SAT 给出的反例必须在解释器下区分两边。

## [0.4.0] - 2026-10-22

### Added

- `trit-wasm` + `playground/`：浏览器里跑求值、三值真值表、等价与 Belnap 四值（wasm-bindgen，`--target web`）。
- `trit-belnap`：Belnap 四值 `T/F/B/N` 的 opt-in 语义（双轨 `(t,f)`、`∧/∨/¬/⇒/⇔`、
  4^n 真值表与逐点等价）；`trit belnap eval|table|equiv`；`sansheng` 再导出。
- 网表指令 `const <0|1|X> <out>` 与 `buf <in> <out>`；`examples/x_mask.net` 演示 `X & 0` / `X | 1` 遮蔽。
- 网表校验：同一信号多驱动（含“既是主输入又被门驱动”）与重复主输入分别报
  `MultipleDrivers` / `DuplicateInput`。
- `trit_circuit::propagate` 与 `trit circuit --assign`：给定一条含 `X` 的主输入向量，单遍拓扑传播，
  报告每根信号取值、主输出上的 `X` 来源，以及被遮蔽（输入含 `X`、输出确定）的门；不枚举 `3^n`。
- `trit_bdd::quantify_bdd` 与 `trit quantify`：三值存在/全称量化（`exists` / `forall`），
  量化域是 trit（`{0,1,X}`），可用 `--binary` 收窄到 `{0,1}`；提供自由变量上的见证/反例与 `--json`。
- `sansheng` 再导出 `Quantifier` / `Quantified` / `quantify_bdd`。

### Fixed

- `Gate::constant` 不再把自己的输出当输入，避免 Const 门被判成组合环。

## [0.3.2] - 2026-10-22

### Added

- `trit verify` 策略支持 `assumptions`：先检查前提可满足，再检查 `assumptions ⇒ assertion` 恒真；
  前提不一致返回 `InconsistentAssumptions`。
- `trit_sat::satisfiable` / `trit_bdd::satisfiable_bdd`：返回可满足见证。

## [0.3.1] - 2026-10-22

### Changed

- Release 产物扩到三平台：`trit-linux-x86_64.tar.gz`、`trit-windows-x86_64.zip`、
  `trit-macos-aarch64.tar.gz`；发布改为 build 矩阵 + 单次 publish，避免并发建 release。

## [0.3.0] - 2026-10-22

### Added

- `trit-core`：`TriVec` 位切片三值向量与 `TriBit::from_binary` / `as_binary`；二进制是
  `unknowns` 缺失的零拷贝视图（1M 元素打包 0.65 ms、K3 AND 0.033 ms）。
- `trit-sat` 二值快速路径：`--binary` / `--binary-all`，纯二值子图走经典 CNF
  （100 变量链子句 3866→598，0.028→0.0147 s）；`--stats` 打印编码规模。
- 充分性反例最小化：`minimal_counterexample` / `--minimize`，对偶 `a==b` 检查保证未列出的变量任取仍区分。
- `trit-bdd`：canonical BDD 等价引擎与 CLI `--bdd`；`--stats` 打印差集节点数。
- `trit-policy` + `trit verify`：JSON 决策表、定义展开、全合法赋值断言检查；失败退出码 1 并给最小反例。
- `action/action.yml`：可复用的 `trit verify` GitHub Action。

### Changed

- 仓库由 `ternary-logic-analyzer` 更名 `trit`，拆成 6 个 crate 的 workspace。
- `EquivalenceResult` 新增 `counterexample_assignment`；编号超出 `u64` 时 `counterexample=None`
  （100 变量不再溢出）。

### Fixed

- Release 工作流打包的二进制名由 `tla` 修正为 `trit`。
