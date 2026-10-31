# 上游收录规则（bfxh/trit）

> **定位一句话**：trit-immune = **裁决账本内核 + 人体工程工具集**。
> 内核是给 trit 生态的（上游收录）；工具集是给本工具用户的（不随上游走）。
> 你（维护者）不需要每次人工判断——`cargo build --no-default-features`
> 一条命令即得上游收录形态，编译通过 = 边界干净。

## 一、收录三分类（机器可查）

上游形态 = `--no-default-features` 构建留下的模块清单（feature 门控钉死）：

| 层 | 模块 | 收录立场 |
|---|---|---|
| **内核（必收）** | trit / delta / journal / hlc / snapshot / migrate（账本）；dag / verdict / sandbox / propagate / policy / cull（裁决）；sync / transport（同步）；bitparallel / auth / kani_proofs（计算验证）；lifecycle / scheduler / design / vocab（生产线）；scan / diag / json / net（裁决源与守卫）；supply / vex / attest（供应链与出口） | ✅ 随 PR 收 |
| **工具层（不收）** | recovery / import（git 桥）/ template / server + FleetServer / daemon / lsp / taskmd / intake / fleet（含 attack_chains）/ harvest / adoption / `trit` CLI | ❌ 本工具专属 |
| **特例** | `tls`（本来就可选 feature）、`compat`（仅 PR 分支，trit-core 互操作——**上游合并时归内核**） | 按上下文 |

规则一句话：**凡是"多个工具共用"的收，凡是"这个工具的人体工程"不收**。
判断口诀：删掉它，内核还完整吗？完整 → 工具层；不完整 → 内核层。

## 二、选型哲学：性价比中性，进制是参数不是信仰

trit 生态不搞三进制原教旨主义：

- **哪个进制好用用哪个**：3-trit（K3 裁决）、5-trit（pack 1.6 bit/trit）、
  4-trit（2 bit/trit = 双导轨）、甚至二进制位平面——都是**参数**。
  `TriWord`（双导轨，位并行快）与 `TriPack`（5/byte，体积省 20%）并存
  就是这条哲学的实例：快 vs 省，按性价比选，不是按信仰选。
- **二进制也一样**：HLC 戳是二进制混合进制的（物理毫秒+计数+写者），
  K3 AND 是"坏消息支配"——我们保留它是因为**语义恰好咬合**（证伪支配、
  未知冻结、确定性收敛），不是因为"三就是好"。
- 给上游贡献技术时的检验问题：**"这个设计换成二进制/四进制还成立吗？**
  成立且更省 → 用别的进制，别为了三而三；不成立（三值语义必需）→ 这才是
  三进制的真正地盘"。
- **性价比要看未来，不能只盯着现在**：
  `性价比 = 当前收益 + 未来可演进空间 − 未来迁移成本`。
  - 现在省 20% 但把数据锁进封闭格式 → 未来迁移成本吃掉全部收益 → 慎选；
  - 现在稍贵但与生态对齐（如 TriPack 与上游 trit-codec::pack 同构互认）
    → 未来收益计入后更划算；
  - 检验问题追加一问：**"三年后换掉它要付多少？"** 答案是"几乎免费"的
    设计（可逆、可并存、可导出）永远优先。

## 三、收录流程（已自动化到两条命令）

1. **本地双形态验证**：
   ```text
   cargo test                     # full 形态：全部 221（默认）
   cargo test --no-default-features   # 内核形态：146（上游将收到的）
   ```
   内核形态编译通过 = 工具层与内核零反向依赖（当前已解耦：json_escape 归
   json.rs、export_dag_tsv 归 journal.rs、adoption 归工具层）。
2. **一键同步**（`scripts/sync-upstream.sh`）：
   ```text
   scripts/sync-upstream.sh <trit-immune 仓库> <trit 上游 clone> [分支]
   ```

## 三点五、同步过程纪律（底层长任务别让智能体卡死）

同步是对上游工作区的破坏性重建（read-tree 清空 prefix）——纪律四条，
全部钉在脚本里：

1. **补丁文件化**：上游专属内容（compat、trit-core 依赖）的真源是
   `upstream/patches/*.patch`（git 管理），工作区永远可以重建——
   read-tree 清掉的东西没有一个是孤本；
2. **幂等**：每个补丁先 `-R --check`（已应用则跳过），脚本中断后**重跑即
   恢复**，没有"半同步"死状态；
3. **分层验证**：`cargo check`（秒级）内建在脚本里先跑；`cargo test`
   （分钟级）**不自动跑**——智能体任务别让底层长任务卡死，长验证由人
   决定时机；
4. **外向动作人工确认**：脚本只做本地同步，commit/push 打印成建议由人
   执行——push 是 outward-facing，不进自动化。

## 四、给新贡献者的检验清单

- [ ] 新模块落在哪层？（口诀：删掉它内核还完整吗）
- [ ] 内核形态编译过吗？（`cargo test --no-default-features`）
- [ ] 新依赖是零依赖的吗？（内核不得引入第三方 crate）
- [ ] 性价比检验过了吗？（换进制还成立吗？成立就更省的那个）
- [ ] 测试是"任何输入都对"（性质）还是"这个输入对"（例子）？
      内核模块至少一条性质测试。
