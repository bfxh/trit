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

## 三、收录流程（已自动化到两条命令）

1. **本地双形态验证**：
   ```text
   cargo test                     # full 形态：全部 221（默认）
   cargo test --no-default-features   # 内核形态：146（上游将收到的）
   ```
   内核形态编译通过 = 工具层与内核零反向依赖（当前已解耦：json_escape 归
   json.rs、export_dag_tsv 归 journal.rs、adoption 归工具层）。
2. **PR 同步**（`git read-tree` 树级叠加 + 三补丁，见仓库记忆/脚本化待办）：
   fetch 本仓库 master → 清 index/clean → read-tree 叠加 → 打 compat
   补丁（lib.rs `pub mod compat` + Cargo.toml `trit-core` 依赖）→
   `cargo test -p trit-immune`。

## 四、给新贡献者的检验清单

- [ ] 新模块落在哪层？（口诀：删掉它内核还完整吗）
- [ ] 内核形态编译过吗？（`cargo test --no-default-features`）
- [ ] 新依赖是零依赖的吗？（内核不得引入第三方 crate）
- [ ] 性价比检验过了吗？（换进制还成立吗？成立就更省的那个）
- [ ] 测试是"任何输入都对"（性质）还是"这个输入对"（例子）？
      内核模块至少一条性质测试。
