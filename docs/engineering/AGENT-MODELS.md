# AGENT-MODELS：子智能体的按角色模型路由

> 定位：给“一个主智能体带多个子智能体”的场景选模型。不同角色（写文档、改代码、评审、
> 沙盒实验）可以落到不同模型，不必整条链共用一个模型。
> 边界：路由器只解决“选谁 + 记下选谁”，不解决“谁更聪明”；后者必须靠评测与回执。
> 状态：**已跑通**（`tools/model_router.py --self-test` 16/16；派发器
> `tools/subagent_dispatch.py --self-test` 25/25）。本机 `probe` 只打印
> `env* env-present`（证明环境变量存在，不是鉴权）：2026-11-22 的真实 `deepseek-chat`
> 调用返回 **HTTP 401 invalid api key**，所以真实外部调用尚未成立，不能把 `probe`
> 当成鉴权/额度/模型名的证明。

## 1. 为什么单独做路由

- 写文档和改代码的最优模型经常不是同一个：长上下文/文风、工具调用、代码正确性、
  成本四条轴不同，用一个模型全包会让每条轴都被平均掉。
- 子智能体的提示词、工具权限、沙盒策略也不同；模型选择必须和角色绑定，才有可复现的
  “这个角色用这个模型”的口径。
- “能连上”不等于“质量好”。路由器只探测可用性（命令在不在、环境变量在不在），
  不在路由阶段发网络请求，也不拿字符数冒充 token。

## 2. 配置

默认读 `<仓库>/.agent/models.json`（本机配置，已 gitignore）；模板见
`tools/models.example.json`。只写环境变量名，**不写密钥字面量**：

```json
{
  "version": 1,
  "models": [
    {
      "id": "deepseek-chat",
      "provider": "openai-chat",
      "base_url": "https://api.deepseek.com/v1",
      "model": "deepseek-chat",
      "api_key_env": "DEEPSEEK_API_KEY",
      "roles": ["docs", "code", "sandbox"],
      "priority": 10,
      "context": 65536,
      "cost": "paid",
      "capabilities": ["text"],
      "enabled": true
    },
    {
      "id": "deepseek-reasoner",
      "provider": "openai-chat",
      "base_url": "https://api.deepseek.com/v1",
      "model": "deepseek-reasoner",
      "api_key_env": "DEEPSEEK_API_KEY",
      "roles": ["review", "docs"],
      "priority": 20,
      "context": 65536,
      "cost": "paid",
      "capabilities": ["text"],
      "enabled": true
    }
  ],
  "routes": {
    "default": ["deepseek-chat"],
    "docs": ["deepseek-chat"],
    "code": ["deepseek-chat"],
    "review": ["deepseek-reasoner"],
    "sandbox": ["deepseek-chat"]
  }
}
```

- `provider=openai-chat`：POST `<base_url>/chat/completions`（若 base_url 已以
  `/chat/completions` 结尾则不重复拼）；`api_key_env` 指向环境变量。
- `provider=command`：本机 CLI，stdin 收提示词，或把 `{prompt}` / `{prompt_file}` /
  `{role}` / `{model}` 占位在 argv 里。适合已经装好的 codex/claude 类 CLI；命令继承
  当前环境（不清洗密钥），不可信提示词请套 `tools/sandbox_run.py`。
- `routes` 是**人的策略**，按列表顺序尝试；`--prefer cost|priority|context` 只对
  可用候选重排，不改变“没可用就拒绝”的规则。

## 3. 命令

```powershell
python tools/model_router.py validate --json
python tools/model_router.py probe
python tools/model_router.py route --role docs --json
python tools/model_router.py route --role review --require text --prefer cost
python tools/model_router.py call --role docs --prompt-file task.md `
  --out draft.md --receipt .agent/model-receipt-docs.json
```

- `route` 选不到可用模型时 exit 2，并列出每个候选不可用的原因；不静默回退到某个付费模型。
- `call` 的输出写到 `--out`，回执写 `--receipt`：`model`、`provider`、`prompt_sha256`、
  `output_sha256`、`seconds`、`usage`（API 有就记，没有记 null）、`error`。
- `call` 的 HTTP 失败/命令失败/超时都 exit 2，回执 `ok=false`；不把错误页当模型输出。

## 4. 子智能体派发（真正并行，不只是路由）

`tools/subagent_dispatch.py` 是“按角色分模型”的执行面，不是只打印 `route` 的元数据：

```powershell
python tools/subagent_dispatch.py --plan tasks.json --root . --parallel 3 `
  --receipt .agent/subagent-dispatch.json
```

```json
{
  "version": 1,
  "tasks": [
    {"id": "doc-readme", "role": "docs", "prompt_file": "tasks/doc.md", "out": "docs/README.md"},
    {"id": "code-fix", "role": "code", "prompt": "修这个 bug", "out": "src/fix.patch", "max_tokens": 2048}
  ]
}
```

- 每个任务走同一份 `model_router` 配置选模型；选不到就只失败该任务（`no-model`），
  不静默换默认模型。不同任务可以落到不同模型，`--parallel N` 真并行。
- 高并发前先过容量准入：默认 `--capacity auto` 在 `--parallel > 2` 时向
  `tools/capacity_lease.py` 申请 worker/内存租约，失败整批拒绝；`--capacity enforce`
  对任意并行数都申请。批准后把 worker/内存/deadline 写成 CAPACITY CONTRACT 注入 prompt。
  详见 `AGENT-CAPACITY.md`。
- 每个任务在调用 provider 前默认过一次 `tools/context_guard.py`（`--guard enforce`）：
  prompt 被判 `quarantine` 时任务在调用前变成 `prompt-quarantined`，不落 `out`；
  `--guard warn` 只记录 `prompt_guard` 并继续调用；`--guard off` 关闭。回执带
  `guard` 与逐任务 `prompt_guard`（verdict/max_severity/findings/input_sha256），
  不记录 prompt 原文。详见 `AGENT-TEXT-GUARD.md`。
- 模型产物也按不可信数据处理：默认 `enforce` 下，产物被判 `quarantine` 或
  扫描失败时改名为 `<out>.quarantine[.N]`，原路径留空，任务 `output-quarantined`；
  `warn` 下留在原路径并标 `output-flagged`；非 UTF-8 产物先抽可打印段与
  zip/tar 文本成员再扫，完全抽不到文本才记 `opaque`，不冒充已扫。
  逐任务回执带 `output_guard`（verdict/max_severity/findings/input_sha256/bytes/
  binary/binary_view）。
- 输出路径、任务 id 必须唯一；`out` 拒绝绝对路径、`..`、`.git/.agent/node_modules/target`，
  已存在默认拒绝，`--overwrite` 或任务内 `overwrite: true` 才覆盖。
- 预算是**声明制**：`budget_tokens<=0` 调用前拒绝；声明了预算但 provider 不报 `usage`
  （例如本机 CLI）=> `budget-unmetered` 失败；报了 `usage` 才事后核对，超了标
  `budget-exceeded`。prompt 侧 token 调用前不可测，这一点不装成“已强制”。
- `--dry-run` 只路由、不调用、不写输出；回执顶层含 `plan_sha256/config_sha256/models`
  和逐任务 `model/status/usage/output_sha256`。`--dry-run` 下的容量准入只模拟，不落租约。
- 边界：派发器不替代 `agent_verify` 的验证链，也不自动套 `sandbox_run`；`command`
  provider 是本机 CLI，可信度由接入者负责（不可信提示词请外包裹沙盒）。容量租约
  是探针估算 + 协调文件，不是 cgroup/Job Object；绕过派发器仍可超。

## 5. 证据与边界

- 自测覆盖：配置校验（重复 id、悬空路由、密钥字面量、非 http base_url）、按角色选、
  `--prefer cost`、能力缺失拒绝、未知角色回退 default、HTTP stub 调用与 usage、HTTP 500
  失败留痕、命令 provider、命令超时、probe 可用性；共 16 条，全部本地夹具，不发真实 API 请求。
- 本机 `probe`：`deepseek-chat`、`deepseek-reasoner` 均可用（只证明环境变量存在，不证明
  网络/额度/模型名有效；真实调用失败会由 `call` 的 exit 2 + 回执暴露）。
- 不是质量评测器，不做自动“便宜就降级”；`call` 会真实计费，测试用 stub 或 `--model` 选择
  免费本地 provider。
- 配置里出现 `api_key/token/secret` 字段或 `sk-...` 形状的值会被 `validate` 拒绝；
  但环境变量本身的值不在本工具审计范围。

## 6. 还没落地

1. 模型质量/成本评测矩阵（同一任务多模型对拍，按角色给读数）。
2. 失败重试与显式降级链：现在失败即拒绝，避免静默换模型；若要做，必须由计划显式列出 fallback。
3. 真实外部模型连通性：本机 DeepSeek 凭据返回 401；需要一个有效凭据或一个已验证的
   `command` provider（`codex exec` 存在但本轮调用挂起，未配置成功）。
4. 密钥托管（现在只认环境变量；没有 keychain/secret manager 接入）。
5. 派发器自动套 `sandbox_run`：现在 command provider 的隔离由调用者显式包裹，尚未内联。
