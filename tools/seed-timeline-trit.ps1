#requires -Version 6.0
# 这是一个版本闸，不是一个偏好。Windows PowerShell 5.1 会把 UTF-8 当 ANSI 解码，
# 读和写都坏；它的 ConvertTo-Json 也没有转义选项。让它在入口处报错，比让它安静地
# 产出坏数据便宜得多。bash 入口是默认路径：tools/*.sh
# seed-timeline-trit: the boundaries we know about, each with a date to look again.
#
# A boundary recorded without a review date is a law. A boundary with one is a claim.
# These are the five that currently limit what this toolchain can promise.

param([Parameter(Mandatory = $true)][string]$Root)

$ErrorActionPreference = 'Stop'
$tl = Join-Path $PSScriptRoot 'timeline.ps1'

$items = @(
    @{ kind = 'boundary'; title = 'Rust 受影响分析只到包级，改一个模块要跑整包'
       detail = 'crates 内 21 个模块不区分，obligation.rs 一改就 -p trit-orch 全跑，减集对单包等于没减。'
       evidence = 'agent-verify -Changed crates/trit-orch/src/obligation.rs | cargo test -p trit-orch 10.05s'
       days = 14 },
    @{ kind = 'boundary'; title = 'policy 是命令分类器，不是内核沙箱'
       detail = '改名 curl.exe、脚本二次拼接、别名都可能绕过；Windows 上没有 Seatbelt/bubblewrap 等价物。'
       evidence = 'selftest F1-F9 | 仅验证关键字与路径边界，未验证改名绕过'
       days = 30 },
    @{ kind = 'limitation'; title = 'journal 只覆盖 Root 之下的树'
       detail = '工作区外写入不在快照范围内，只能靠 policy 事前拒绝。'
       evidence = 'journal begin -- 1627 files, 0 changed | 范围限于 -Root'
       days = 30 },
    @{ kind = 'limitation'; title = 'Node 适配器只在 fixture 上验证过'
       detail = 'dsh-orch 之前跑不起来是因为依赖了未构建的 Rust CLI 产物，属环境问题；适配器本身未在真项目通过。'
       evidence = 'costprobe -Path node-proj | exit 0 | 仅 fixture'
       days = 7 },
    @{ kind = 'risk'; title = '跨语言调用边未建模，只能靠 -AssumeIsolated 声明'
       detail = 'Rust 调 Python、Node 调 Rust 的边不在图里；多语言仓默认回退全量，但显式声明后就不再保护。'
       evidence = 'selftest A1-A3 | proven=false 时回退全量'
       days = 14 }
)

$ok = 0; $skip = 0
foreach ($i in $items) {
    & pwsh -NoProfile -File $tl -Mode open -Root $Root -Kind $i.kind -Title $i.title -Detail $i.detail -Evidence $i.evidence -ReviewAfterDays $i.days | Out-Null
    if ($LASTEXITCODE -eq 0) { $ok++ }
    elseif ($LASTEXITCODE -eq 3) { $skip++ }
    else { "seed-timeline: FAILED -> $($i.title) (exit $LASTEXITCODE)" }
}
"seed-timeline: $ok opened, $skip already present"
