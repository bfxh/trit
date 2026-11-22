# AGENT-SANDBOX：子智能体命令的 workspace 沙盒

> 定位：给“子智能体要跑命令、但不想让它悄悄改宿主”的场景。
> 一句话：**默认 cwd 在宿主外的空 workdir，只复制声明输入，policy 先拒，caps 兜底，
> 跑完用宿主指纹证明有没有写坏；需要真隔离时显式走 `-Isolation wsl` 的 WSL+bwrap 内核命名空间。**
> 边界：默认模式不是内核沙盒；`-Isolation wsl` 只接受 Linux 命令。见 §4。
> 状态：**已跑通**（`tools/sandbox_run.py --self-test` 41/41；`sandbox_matrix.py --cases 40` 40/40；
> `hostwrite` 独立宿主 100 例 100/100；WSL+bwrap 隔离回归 **11/11**）。红队已补的洞见
> `AGENT-REDTEAM.md`：workdir junction、硬链接输入、git-ignored 宿主写、
> 根内 junction 复制、`.agent/.git` 排除目录、`.git/index` 与构建缓存漏检、
> 大小写/尾点路径绕过、同 size+恢复 mtime、`.git/index.lock` 后置残留。

## 1. 为什么不是“有 policy 就够了”

`tools/policy.py` 的 docstring 已经写明：它是命令分类器 + 树 journal，不是 kernel sandbox；
`run_caps.py` 是资源帽，不是安全边界。两者合起来仍有一个空档：

- 已放行的 `python script.py` 可以在脚本里 `open("宿主外的绝对路径/evil.txt","w")`，policy 看不见；
- 子智能体在仓库根直接跑，相对路径写会落到宿主；
- 跑完以后，没有人证明“宿主到底被没被写”。

workspace 沙盒补的就是这个空档：把 cwd 移出宿主、只给声明输入、跑完对比宿主指纹。
它不阻止蓄意逃逸，但让“悄悄改宿主”从默认可能变成默认被拒 + 有证据。

## 2. 一次 run 的六步

1. 在宿主根**之外**建空 workdir（默认 `%TEMP%\trit-sandbox-*`）；`-Work` 指到宿主内直接拒绝。
2. `-Copy` 只接受宿主内相对路径：拒绝绝对路径、`..`、符号链接/junction 等
   任何 reparse point、硬链接（`nlink>1`）、`.git/.agent/node_modules/target/.qoder`
   组件；组件比较做大小写 + Windows 尾点归一；默认上限 64 MB / 5000 文件。
   `-Work` 也先 `realpath`，junction/符号链接指向别处直接拒绝。
3. `policy.py -Profile <profile> -Root <workdir> -Command <命令>` 预分类；policy 拒绝就**不运行**。
4. `run_caps.py` 套内核资源帽（mem/procs/seconds）；stdin 接 DEVNULL；环境只留白名单，
   密钥类变量摘掉（回执只记变量名，不记值）。显式 `-CapacityLease <id>` 时，
   `run_caps --lease` 会先校验租约仍存活、租约内存是上限，再执行；过期/缺失/超额
   fail-closed。
4b. 显式 `-Isolation wsl` 时，命令改由 `wsl.exe` 里的 `bwrap` 执行：
   `--unshare-all`（user/pid/net/ipc/uts/cgroup）、空根只读 bind `/usr /bin /lib
   /lib64 /sbin /etc /usr/local`、只把 workdir bind 到 `/work`、`--cap-drop ALL`、
   `--clearenv`。Windows `.exe` 与 Windows 绝对路径参数直接拒绝，不回退到默认模式。
   默认仍是 `none`；不做静默降级。
5. 运行前后各取一次宿主指纹：git 状态（HEAD + 过滤后的 status + `git diff HEAD` +
   未跟踪内容）**加**全部文件 size+mtime manifest；`.git/.agent/.qoder` 不再排除，
   `.git/index` 按**暂存条目语义哈希**（`git ls-files --stage -z`，排除并发读
   git 时 stat cache 重写造成的假变化；坏索引回退到原始内容哈希），
   `node_modules/target` 只跳过文件遍历但保留目录与 name/size/mtime 指纹；
   文件在 64 MiB 总预算内都按 sha256 内容哈希（超过预算判 `host-unprovable`），
   封住“同 size 覆盖 + `os.utime` 恢复 mtime”；构建缓存小文件仍只做 name/size/mtime；
   `.git/index.lock` 不进 manifest，运行后若从无到有、且经过 0.35 秒仍存在，才判
   `host-mutated`（并发 `git status` 的瞬时锁不算）；
   超过 200000 文件判 `host-unprovable`，不静默降级。
6. 宿主变了且没有 `-AllowHostWrite` → `host-mutated`，exit 3；成功默认清掉 workdir，
   `-Keep` 留现场。

## 3. 命令与判据

```powershell
# 让子智能体只读一份规范，写自己的草稿到 workdir
python tools/sandbox_run.py -Root . -Copy docs/AGENT-TASK-STANDARD.md `
  -Role docs -Model deepseek-chat -CapsSeconds 60 -- python tools/some_writer.py

# 允许把结果写回宿主（显式风险开关）
python tools/sandbox_run.py -Root . -Copy draft.md -AllowHostWrite `
  -- python tools/publish.py

# 真内核隔离：只在 WSL 里跑 Linux 命令，绝对路径读/网络/子进程都关在 bwrap 命名空间
python tools/sandbox_run.py -Root . -Copy script.py -Isolation wsl `
  -WslDistro Ubuntu-24.04 -- python3 script.py
python tools/sandbox_run.py --self-test-wsl -WslDistro Ubuntu-24.04

# 接父智能体批准的容量租约：sandbox_run 与 run_caps 共用同一份 lease
python tools/sandbox_run.py -Root . -CapacityLease cap-... -CapacityDir <state> `
  -CapsMemMb 1024 -- python tool.py

# 批量对抗：40 个独立沙盒并发
python tools/sandbox_matrix.py --cases 40 --parallel 8 --json

# 单独压 host-write 检出：100 个独立宿主，每个都写绝对路径
python tools/sandbox_matrix.py --cases 100 --parallel 12 --scenario hostwrite --json
```

回执 `verdict` 是有限集，不是“成功/失败”二分：

| verdict | 含义 | exit |
|---|---|---|
| `ok` | policy 放行、run 成功、宿主指纹未变 | 0 |
| `policy-denied` | policy 在运行前拒绝（网络/删除/越界写等） | 1 |
| `host-mutated` | 跑完发现宿主指纹变了，且没开 `-AllowHostWrite` | 3 |
| `host-unprovable` | 宿主指纹读不出（不是“干净”） | 3 |
| `run-timeout` | run_caps 的秒帽触发（124） | 124 |
| `run-failed` | 命令自身非零退出 | 原值 |
| `bad-invocation` | 参数/输入复制/workdir 不合法 | 2 |

## 4. 边界（诚实）

- **默认模式不是内核隔离**：绝对路径、网络 socket、子进程、`ctypes` 都能逃出 workdir；
  本工具默认 cwd + policy + 事后指纹，防的是“无意/顺手写坏”，不是“蓄意逃逸”。
- **`-Isolation wsl` 是另一条路**：WSL2 内的 bwrap `--unshare-all` 把 user/pid/net/ipc/uts/cgroup
  都切开，根是空目录，只把 workdir bind 到 `/work`；实测绝对路径读、绝对路径写、直连
  `1.1.1.1:80`、`subprocess cat` 都被阻断（`--self-test-wsl` 11/11）。它只接受 Linux
  命令（`python3`/`bash` 等），Windows `.exe` 和 Windows 绝对路径参数直接拒绝；这不是
  “Windows 任意命令沙盒”，也不是默认模式。
- 宿主指纹能发现内容变化；64 MiB 总预算内的宿主文件（含大于 1 MiB 的）都按内容哈希，
  超出预算判 `host-unprovable`（fail-closed），不再静默退化成 metadata-only。构建缓存
  （`node_modules/target`）仍只做目录/name/size/mtime，主要是遍历成本无界；它也不证明
  “读”没发生：默认模式下绝对路径读仍可读宿主 secret，需要读隔离时用 `-Isolation wsl`。
- `.git/index.lock` 只做“运行后新留锁且 0.35 秒稳定存在”的检查；并发 git 进程自己的
  短暂锁不会被当成命令逃逸，但也不能证明锁从未被创建过。恶意命令若故意留锁又在窗口内
  被别的进程清掉，仍可逃过这一项。
- `-Copy` 拒绝硬链接是因为根内相对硬链接可以指向根外 inode；`nlink>1` 也意味着同一
  文件在别处被链接，拒绝比“猜链接目标”更安全。
- 默认模式的“不联网”指 policy 的 network 轴；`-AllowNetwork` 只让 policy 放行，运行期没有
  内核网络隔离。WSL 模式直接 `--unshare-all`，因此 `-AllowNetwork` 与之不兼容，直接拒绝。
- `run_caps` 的 `procs` 帽在 POSIX 没有等价物（见其 docstring），本工具不伪造。
- workdir 里生成的文件默认随成功清理；要取产物必须 `-Keep` 或显式从 workdir 复制回来，
  不存在“沙盒自动把产物落回宿主”的隐式路径。
- 不替代 `journal.py` 的撤销证据；沙盒的宿主指纹只回答“变没变”，不回答“怎么撤”。

## 5. 测试

- `sandbox_run.py --self-test` 37 条：相对写、成功清理、复制输入、宿主写检测/显式允许、
  超时 124、内存帽、密钥环境摘除、PATH 保留、非零退出、命令缺失 127、输出截断、stdin EOF、
  stderr 分离、网络/删除 policy 拒绝、复制穿越/字节帽、硬链接拒绝、workdir junction 拒绝、
  git-ignored 宿主写检测、嵌套复制、keep、workdir 在宿主内拒绝、并发隔离、非 git 宿主回退、
  `.agent`/`.git` 忽略目录写、`.git/index` 改写、缓存新建/覆盖、同 size+恢复 mtime、
  **大于 1 MiB 文件同 size+恢复 mtime**、`.git/index.lock` 后置残留。
- `sandbox_run.py --self-test-wsl -WslDistro Ubuntu-24.04`：WSL+bwrap 11/11，
  workdir 可读、绝对路径读/写、网络、子进程四项全阻断，`.exe`/Windows 绝对路径参数拒绝。
- `sandbox_matrix.py --cases 40 --parallel 8`：40 个独立 workdir 并发，40/40 通过，
  distinct workdirs=40；本机墙钟 **4.5 s**（内容哈希加深后）。矩阵判据同时检查 host 未被写、超时/策略/退出码/截断的语义。
  加压到 `--cases 100 --parallel 12` 仍 **100/100**（混跑 6.21 s），用于确认驱动没有在第 40 例后退化。
- `sandbox_matrix.py` 新增 `hostwrite` 场景：每个 case 在**独立宿主**上用绝对路径写文件，
  必须判 `host-mutated` 且文件出现；共享宿主会把“别人写变了”误判成本 case 逃逸，故每个 case
  单独复制宿主。40 例矩阵中 **5/5 检出**；`--scenario hostwrite --cases 100 --parallel 12`
  为 **100/100**（本机 **3.22 s**，workdir 100 个互不相同）。
- 未覆盖：Windows 原生 AppContainer/WFP、Linux seccomp 细粒度规则、超大量文件
  （>5000）性能、沙盒产物自动回收策略；WSL+bwrap 后端已覆盖 namespace/文件/网络/子进程。

## 6. 还没落地

1. Windows 原生内核隔离后端（AppContainer/Job + WFP）尚未落地；Linux 侧已用
   WSL2+bwrap 提供 `-Isolation wsl`，并做了“后端不可用就拒绝”的能力探测。
2. 默认模式的网络 egress 仍只有 policy 的命令级拒绝；要真断网用 `-Isolation wsl`。
3. 沙盒产物清单与哈希化回收（`-Keep` 后由主智能体决定哪些落回宿主）。
4. 把 `sandbox_run` 接进 `agent_verify` 的 run 段（现在它是独立入口，回执格式未合并）。
