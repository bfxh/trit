# 活动声明（多智能体重叠提醒）

机器读写：`scripts/agent_claims.py`。人可读，收工请 release。


| id | who | repo | paths | claimed_at | ttl_min | note | renewed |
| --- | --- | --- | --- | --- | --- | --- |
| c2 | zodER(ZCode) | bfxh/trit | scripts/mutation_check.py,crates/trit-net,docs/SECURITY-NET.md,CHANGELOG.md | 1793647344 | 240 | netsec M2/M3（#215/#217 已合），也改变异登记表 | 0 |
| c3 | Qoder(sess 179ea139) | bfxh/trit | crates/trit-net/src/netsec,crates/trit-ops/src,crates/trit-train/src,crates/trit-wasm/src,crates/trit-immune/src,crates/trit-immune/scripts,crates/trit-circuit/src,crates/trit-net/src,docs/SECURITY-NET.md,scripts/gitflow.py,scripts/snapshot.py,scripts/agent_claims.py,scripts/mutation_check.py,scripts/mutants/scripts__gitflow.py.json,scripts/mutants/scripts__snapshot.py.json,scripts/mutants/scripts__agent_claims.py.json,scripts/mutants/scripts__mutation_check.py.json,scripts/mutants/crates__trit-immune__scripts__sync-upstream.sh.json,.github | 1793678060 | 90 | #233/#236/#237 等 CI；新增：变异门把「未点名」从算抓住改成判红 | 0 |
| c6 | Qoder(sess dc34ac09) | bfxh/trit | scripts/mutation_sweep.py,scripts/mutants/scripts__mutation_sweep.py.json,.github/workflows/ci.yml,scripts/test_surface_baseline.json | 1793678632 | 120 | A309 后半：把全表 sweep 挂进现有 job mutation gate (self-test) 的 step；不新建检查名、不动 required-checks.txt；门本体 main()/classify 已让给 179ea139。注：对方 claim 仍挂 .github 整目录 ⇒ 本条仍判拦，CI 那半步等对方收窄再动 | 0 |
| c1 | qoder:f5e9c21f | bfxh/trit | crates/trit-parts/tests,crates/trit-parts/src/model,crates/trit-count/src,crates/trit-count/examples,docs/STATUS.md,docs/ROADMAP.md,CHANGELOG.md,scripts/claims_evidence.py,scripts/claims_evidence_baseline.json,scripts/mutants/scripts__claims_evidence.py.json,scripts/tech_collide.py | 1793919980 | 240 | DPLL 分配口径探针 + 残基子句表扁平化（µs/节点 5.0x 上升的归因） | 0 |
| c5 | qoder:f1c14d91 | bfxh/trit | crates/trit-re/src/sigscan.rs,crates/trit-re/src/sigscan/scan.rs,crates/trit-re/src/sigscan/pattern.rs,crates/trit-re/src/sigscan/regex.rs,crates/trit-re/src/yaraparse.rs,crates/trit-re/src/yaraparse/lex.rs,crates/trit-re/tests/yara.rs,crates/trit-re/tests/yara_regex.rs,docs/CALIBRATION.md,docs/SECURITY-RE.md,scripts/module_budget_baseline.json | 1794438679 | 90 | YARA 正则串 Pike VM 收尾：代码+预算台账+SECURITY-RE/CALIBRATION（CHANGELOG 暂缓，c4 占用） | 0 |
| c7 | human:38455807607746050 | bfxh/trit | tools/handoff.py,tools/agent_verify.py,tools/_run.py,tools/bench_orchestration.py,docs/engineering/AGENT-COORDINATION.md,docs/ERRORS.md | 1795393509 | 90 |  | 0 |
| c4 | codex:deepseek-flash-20261122 | bfxh/trit | crates/trit-wspace/src,crates/trit-wspace/examples/bert6,crates/trit-wspace/tests,docs/research/WSP-BERT6-RESULTS.md,docs/results/LESSONS.md,CHANGELOG.md,scripts/module_budget_baseline.json,scripts/ledger-snapshot | 1795394048 | 90 | 低秩残差侧车：R=W-s*T 的 top-r SVD，全量 1379 对复验（PR #618） | 0 |
