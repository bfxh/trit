# 工作区记忆

项目：D:\KF\trit-inv（共 43 条记录）

## 已知坑（别踩第二次）
- 验证检查是否有效时，先确认注入点落在被检查的区段内
- 真值表行号随变量出现顺序变，直接比 values 会误判等价
- 对照实验要喂同一份输入
- 静默失效族：判定/边界/缓存失效都不报错，只给错结果
- 根边界用 startswith 比较等于没边界：兄弟目录被判成根内

## 已定决策
- 问题即检查：台账条目必须绑定可执行检查，L56 起纯文字条目硬红
- 测试扩面按符号级命中率排优先级，四个 crate 补到 528 条
- trit-orch 测试扩到 296 条：先符号级盘点再补静默失效面
- 执行面必须是 Git Bash
- policy 迁到 python+bash，20 例一致，另修两个边界漏判

## 验证过的命令
- 全工作区 cargo test 约 59s，单包约 10s
- agent-verify 在 trit-inv 上端到端：proven subset，10.05s，changed=0
- pwsh tools/selftest.ps1 → 142 条全绿（bash 入口已为主路径）

> 详细内容用 -Mode recall -Query <词> 取，别把整本日志灌进上下文。
