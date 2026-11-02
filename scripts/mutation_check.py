#!/usr/bin/env python3
"""变异门：对关键规则做单点文本变异，跑对应回归；测试必须变红。

用法：python scripts/mutation_check.py [--pkg <目标>]...
目标 = crate 名，或带自测面的脚本路径（`scripts/*.py`、`crates/*/scripts/*.sh`）。
每个变异都会在 finally 里**按字节**恢复原文件；退出码 1 表示有 mutant 存活。

语言口径见 RUNNERS：
  cargo  —— `cargo test -p <crate> <测试名> --locked`
  python —— `<python> <脚本> --self-test`
  bash   —— `bash <脚本> --self-test`
登记表里没写 runner 时按 pkg 后缀推断（.py / .sh），避免脚本被当成 crate 去跑 cargo。
"""

from __future__ import annotations

import contextlib
import io
import pathlib
import re
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]

MUTANTS = [
    {
        "name": "core: K3 strong_add 用 OR 代替 AND",
        "file": "crates/trit-core/src/algebra.rs",
        "old": "    pub const fn strong_and(self, a: Ternary, b: Ternary) -> Ternary {\n        match self {\n            Self::Lukasiewicz => {\n                let s = a.as_f64() + b.as_f64() - 1.0;\n                if s <= 0.0 {\n                    Ternary::False\n                } else if s >= 1.0 {\n                    Ternary::True\n                } else {\n                    Ternary::Unknown\n                }\n            }\n            Self::Kleene | Self::Bochvar => {\n                if a.is_unknown() || b.is_unknown() {\n                    Ternary::Unknown\n                } else if a.is_true() && b.is_true() {",
        "new": "    pub const fn strong_and(self, a: Ternary, b: Ternary) -> Ternary {\n        match self {\n            Self::Lukasiewicz => {\n                let s = a.as_f64() + b.as_f64() - 1.0;\n                if s <= 0.0 {\n                    Ternary::False\n                } else if s >= 1.0 {\n                    Ternary::True\n                } else {\n                    Ternary::Unknown\n                }\n            }\n            Self::Kleene | Self::Bochvar => {\n                if a.is_unknown() || b.is_unknown() {\n                    Ternary::Unknown\n                } else if a.is_true() || b.is_true() {",
        "pkg": "trit-core",
        "filter": "strong_and_lukasiewicz_matches_formula"
    },
    {
        "name": "core: B3 或忘记扣掉未知位",
        "file": "crates/trit-core/src/bits.rs",
        "old": "let res1 = (a1 | b1) & !resx;",
        "new": "let res1 = a1 | b1;",
        "pkg": "trit-core",
        "filter": "ops_never_produce_illegal_rails"
    },
    {
        "name": "core: from_binary 不截断多余字",
        "file": "crates/trit-core/src/bulk.rs",
        "old": "        ones.truncate(need);",
        "new": "        let _ = need;",
        "pkg": "trit-core",
        "filter": "from_binary_normalizes"
    },
    {
        "name": "eval: OR 吸收误用 AND 形状",
        "file": "crates/trit-eval/src/simplify.rs",
        "old": "    if let Expr::And(p, q) = y",
        "new": "    if let Expr::Or(p, q) = y",
        "pkg": "trit-eval",
        "filter": "absorption_only_removes_matching_operator"
    },
    {
        "name": "eval: K3 a^a 不再折回 a",
        "file": "crates/trit-eval/src/simplify.rs",
        "old": "    if x == y && alg != Algebra::Lukasiewicz {",
        "new": "    if x == y && alg == Algebra::Lukasiewicz {",
        "pkg": "trit-eval",
        "filter": "xor_and_equiv_self_respect_unknown"
    },
    {
        "name": "eval: 自等价对所有代数折叠成 1",
        "file": "crates/trit-eval/src/simplify.rs",
        "old": "    if x == y && alg == Algebra::Lukasiewicz {",
        "new": "    if x == y {",
        "pkg": "trit-eval",
        "filter": "xor_and_equiv_self_respect_unknown"
    },
    {
        "name": "sat: 二值 strong_add 的 Ł3 分支用 AND",
        "file": "crates/trit-sat/src/symbolic.rs",
        "old": "            Algebra::Lukasiewicz => self.binary_or(a, b),",
        "new": "            Algebra::Lukasiewicz => self.binary_and(a, b),",
        "pkg": "trit-sat",
        "filter": "binary_fast_path_uses_strong_add_not_xor"
    },
    {
        "name": "circuit: 拓扑排序退回声明序",
        "file": "crates//trit-circuit//src//circuit//graph.rs",
        "old": "            if deps.iter().all(|d| resolved.contains(d)) {",
        "new": "            if true {",
        "pkg": "trit-circuit",
        "filter": "evaluation_is_independent_of_gate_declaration_order"
    },
    {
        "name": "verilog: 拼接嵌套不计深度",
        "file": "crates/trit-circuit/src/verilog/expr.rs",
        "old": "                self.enter()?;" + "\n" + "                let parsed = self.parse_concat_body();" + "\n" + "                self.leave();" + "\n" + "                parsed",
        "new": "                let parsed = self.parse_concat_body();" + "\n" + "                parsed",
        "pkg": "trit-circuit",
        "filter": "concat_depth_just_over_limit_returns_limit_error",
    },
    {
        "name": "verilog: 二元运算链不封顶",
        "file": "crates/trit-circuit/src/verilog/expr.rs",
        "old": "        self.ops += 1;" + "\n" + "        if self.ops > MAX_RHS_OPS {",
        "new": "        self.ops += 1;" + "\n" + "        if self.ops > MAX_RHS_OPS * 40 {",
        "pkg": "trit-circuit",
        "filter": "long_operator_chain_returns_limit_not_stack_overflow"
    },
    {
        "name": "sdf: 括号深度护栏失效",
        "file": "crates/trit-circuit/src/sdf.rs",
        "old": "                self.depth += 1;" + "\n" + "                if self.depth > MAX_SDF_DEPTH {",
        "new": "                self.depth += 1;" + "\n" + "                if false {",
        "pkg": "trit-circuit",
        "filter": "sdf_depth_just_over_limit_returns_limit_error",
    },
    {
        "name": "sdf: 输入字节预算失效",
        "file": "crates/trit-circuit/src/sdf.rs",
        "old": "    if text.len() > MAX_SDF_BYTES {",
        "new": "    if false {",
        "pkg": "trit-circuit",
        "filter": "budget_rejects_deep_nesting_and_oversized_input"
    },
    {
        "name": "params: 表达式嵌套护栏失效",
        "file": "crates/trit-circuit/src/verilog/params.rs",
        "old": "        self.depth += 1;" + "\n" + "        if self.depth > MAX_PARAM_DEPTH {",
        "new": "        self.depth += 1;" + "\n" + "        if false {",
        "pkg": "trit-circuit",
        "filter": "parameter_depth_just_over_limit_returns_limit_error",
    },
    {
        "name": "aiger: 文本字节预算失效",
        "file": "crates/trit-circuit/src/aiger.rs",
        "old": "    if text.len() > MAX_AIGER_BYTES {",
        "new": "    if false {",
        "pkg": "trit-circuit",
        "filter": "aiger_budget_rejects_huge_headers_and_oversized_input"
    },
    {
        "name": "aiger: 计数对行数不校验",
        "file": "crates/trit-circuit/src/aiger.rs",
        "old": "    if declared > lines.len() {",
        "new": "    if false {",
        "pkg": "trit-circuit",
        "filter": "aiger_budget_rejects_huge_headers_and_oversized_input"
    },
    {
        "name": "schedule: 未知传播回退全量扫描",
        "file": "crates/trit-schedule/src/lib.rs",
        "old": "    let mut worklist: Vec<usize> = (0..n).filter(|&index| deferred[index].is_some()).collect();",
        "new": "    let mut worklist: Vec<usize> = Vec::new();",
        "pkg": "trit-schedule",
        "filter": "reverse_chain_deferral_is_linear_not_quadratic"
    },
    {
        "name": "schedule: 资源溢出改记空消息",
        "file": "crates/trit-schedule/src/runtime/execute.rs",
        "old": '                    message: "资源累计溢出".to_string(),',
        "new": "                    message: String::new(),",
        "pkg": "trit-schedule",
        "filter": "resource_overflow_without_capacity_fails_instead_of_panicking"
    },
    {
        "name": "schedule: step panic 记成完成",
        "file": "crates/trit-schedule/src/resumable_threaded.rs",
        "old": '            let progress = catch_unwind(AssertUnwindSafe(|| task.step(&ctx)))\n                .unwrap_or_else(|_| Progress::Failed("任务 panic".to_string()));',
        "new": '            let progress = catch_unwind(AssertUnwindSafe(|| task.step(&ctx)))\n                .unwrap_or_else(|_| Progress::Done);',
        "pkg": "trit-schedule",
        "filter": "task_panic_fails_the_run_instead_of_hanging"
    },
    {
        "name": "texture: trit 打包换基 2",
        "file": "crates/trit-texture/src/lib.rs",
        "old": "        out[byte] += (trit_digit(*value) * 3u32.pow(slot as u32)) as u8;",
        "new": "        out[byte] += (trit_digit(*value) * 2u32.pow(slot as u32)) as u8;",
        "pkg": "trit-texture",
        "filter": "pack_roundtrips_all_five_trit_values"
    },
    {
        "name": "texture: 调色板中点写成 1/3",
        "file": "crates/trit-texture/src/endpoints.rs",
        "old": "    let mid = |a: u8, b: u8| ((u16::from(a) + u16::from(b)) / 2) as u8;",
        "new": "    let mid = |a: u8, b: u8| ((u16::from(a) + u16::from(b)) / 3) as u8;",
        "pkg": "trit-texture",
        "filter": "block_roundtrips_endpoint_colors_exactly"
    },
    {
        "name": "texture: 最近色并列取大下标",
        "file": "crates/trit-texture/src/endpoints.rs",
        "old": "        if dist < best_dist {",
        "new": "        if dist <= best_dist {",
        "pkg": "trit-texture",
        "filter": "nearest_color_breaks_ties_toward_the_lower_index"
    },
    {
        "name": "texture: 基线位打包丢移位",
        "file": "crates/trit-texture/src/lib.rs",
        "old": "        payload[bit / 8] |= ((palette_index << (bit % 8)) & 0xff) as u8;",
        "new": "        payload[bit / 8] |= (palette_index & 0xff) as u8;",
        "pkg": "trit-texture",
        "filter": "baseline_roundtrips_endpoint_colors_exactly"
    },
        {
                "name": "texture: into 解码 alpha 写 0",
                "file": "crates/trit-texture/src/lib.rs",
                "old": "                Ternary::True => 2,\n            }];\n            out[offset] = r;\n            out[offset + 1] = g;\n            out[offset + 2] = b;\n            out[offset + 3] = 255;",
                "new": "                Ternary::True => 2,\n            }];\n            out[offset] = r;\n            out[offset + 1] = g;\n            out[offset + 2] = b;\n            out[offset + 3] = 0;",
                "pkg": "trit-texture",
                "filter": "decode_into_matches_allocating_and_rejects_short_buffers"
    },
    {
        "name": "ttx2: mip 允许同级",
        "file": "crates/trit-texture/src/ttx2/mod.rs",
        "old": "            if !(w <= prev_w && h <= prev_h && (w < prev_w || h < prev_h)) {",
        "new": "            if !(w <= prev_w && h <= prev_h && (w <= prev_w || h <= prev_h)) {",
        "pkg": "trit-texture",
        "filter": "mip_chain_rejects_equal_misaligned_and_empty"
    },
    {
        "name": "ttx2: 基 3 打包换基 2",
        "file": "crates/trit-texture/src/ttx2/model.rs",
        "old": "            (value_digit(*value) * 3u32.pow((index % TRITS_PER_PACKED_BYTE) as u32)) as u8;",
        "new": "            (value_digit(*value) * 2u32.pow((index % TRITS_PER_PACKED_BYTE) as u32)) as u8;",
        "pkg": "trit-texture",
        "filter": "model_container_bytes_are_pinned"
    },
    {
        "name": "ttx2: digit 映射偏移",
        "file": "crates/trit-texture/src/ttx2/model.rs",
        "old": "    (value + 1) as u32",
        "new": "    (value + 2) as u32",
        "pkg": "trit-texture",
        "filter": "model_container_bytes_are_pinned"
    },
    {
        "name": "ttx2: 层间衔接放行",
        "file": "crates/trit-texture/src/ttx2/model.rs",
        "old": "            if index > 0 && layer.inputs != layers[index - 1].outputs {",
        "new": "            if false && index > 0 && layer.inputs != layers[index - 1].outputs {",
        "pkg": "trit-texture",
        "filter": "chain_dimensions_are_enforced_like_trit_net"
    },
    {
        "name": "ttx2: CRC 生成常数换轨",
        "file": "crates/trit-texture/src/ttx.rs",
        "old": "pub(crate) const GEN16: [i32; 17] = [2, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1];",
        "new": "pub(crate) const GEN16: [i32; 17] = [2, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 1];",
        "pkg": "trit-texture",
        "filter": "model_container_bytes_are_pinned"
    },
    {
        "name": "ttx2: 双轨行界错位（平面下标）",
        "file": "crates/trit-texture/src/ttx2/model.rs",
        "old": "                    1 => pos[row * words + col / TRITS_PER_WORD] |= bit,",
        "new": "                    1 => pos[(row * self.inputs + col) / TRITS_PER_WORD] |= bit,",
        "pkg": "trit-texture",
        "filter": "dual_rail_conversion_matches_the_gpu_layout"
    },
    {
        "name": "convert: 降级放行 mip 链",
        "file": "crates/trit-texture/src/convert.rs",
        "old": "    if levels.len() != 1 {",
        "new": "    if levels.len() > 2 {",
        "pkg": "trit-texture",
        "filter": "downgrade_rejects_mip_chains_and_model_kind"
    },
    {
        "name": "convert: v2 解码取末级",
        "file": "crates/trit-texture/src/convert.rs",
        "old": "    let texture = levels.first().ok_or(TextureError::TtxEmpty)?;",
        "new": "    let texture = levels.last().ok_or(TextureError::TtxEmpty)?;",
        "pkg": "trit-texture",
        "filter": "downgrade_rejects_mip_chains_and_model_kind"
    },
    {
        "name": "convert: 盒式滤波只取首像素",
        "file": "crates/trit-texture/src/convert.rs",
        "old": "            for (dy, dx) in [(0usize, 0usize), (0, 1), (1, 0), (1, 1)] {",
        "new": "            for (dy, dx) in [(0usize, 0usize)] {",
        "pkg": "trit-texture",
        "filter": "mip_chain_dimensions_and_box_filter_are_exact"
    },
    {
        "name": "circuit: 引脚故障不注入",
        "file": "crates//trit-circuit//src//circuit//sim.rs",
        "old": "                ins[pin] = value;",
        "new": "                let _ = value;",
        "pkg": "trit-circuit",
        "filter": "fault_polarities_are_reported_separately"
    },
    {
        "name": "sat: 前提过滤不再要求 premise.t=1",
        "file": "crates/trit-sat/src/symbolic.rs",
        "old": "    enc.clause(&[Lit::from_var(p.t, true)]);",
        "new": "    enc.clause(&[Lit::from_var(p.t, false)]);",
        "pkg": "trit-policy",
        "filter": "counterexample_satisfies_premise"
    },
    {
        "name": "atpg: 全向量矩阵枚举为空",
        "file": "crates//trit-circuit//src//atpg//mod.rs",
        "old": "    let total = 1u64 << circuit.input_count();",
        "new": "    let total = 0;",
        "pkg": "trit-circuit",
        "filter": "atpg"
    },
    {
        "name": "circuit: 不可观测误判为被遮蔽",
        "file": "crates//trit-circuit//src//circuit//analyze.rs",
        "old": "                } else if reachable.contains(gate.output.name()) {",
        "new": "                } else if true {",
        "pkg": "trit-circuit",
        "filter": "untestable_reasons"
    },
    {
        "name": "circuit: Const 缺值不再报错",
        "file": "crates//trit-circuit//src//circuit//validate.rs",
        "old": "            (GateKind::Const, None) => {",
        "new": "            (GateKind::Const, None) if false => {",
        "pkg": "trit-circuit",
        "filter": "const_gate_without_value_is_rejected"
    },
    {
        "name": "circuit: 非 Const 带值不再报错",
        "file": "crates//trit-circuit//src//circuit//validate.rs",
        "old": "            (kind, Some(_)) if kind != GateKind::Const => {",
        "new": "            (_, Some(_)) if false => {",
        "pkg": "trit-circuit",
        "filter": "non_const_gate_with_value_is_rejected"
    },
    {
        "name": "circuit: 空信号名不再报错",
        "file": "crates//trit-circuit//src//circuit//validate.rs",
        "old": "        if sig.name().is_empty() {",
        "new": "        if false {",
        "pkg": "trit-circuit",
        "filter": "empty_signal_name_is_rejected"
    },
    {
        "name": "verilog: 向量声明记成 1 位",
        "file": "crates/trit-circuit/src/verilog/decl.rs",
        "old": "                let bus = if range.ascending {\n                    Bus::new_ascending(name.clone(), range.width)\n                } else {\n                    Bus::new(name.clone(), range.width)\n                };\n",
        "new": "                let bus = Bus::new(name.clone(), 1);\n",
        "pkg": "trit-circuit",
        "filter": "vector_declarations_are_recorded_as_buses"
    },
    {
        "name": "verilog: 导出忽略 bus 宽度",
        "file": "crates/trit-circuit/src/verilog/export.rs",
        "old": "        let (max, vector, ascending) = if let Some(bus) = buses.get(&base) {",
        "new": "        let (max, vector, ascending) = if let Some(bus) = buses.get(&base).filter(|_| false) {",
        "pkg": "trit-circuit",
        "filter": "export_uses_declared_bus_width_for_partial_wire"
    },
    {
        "name": "circuit: 零宽 bus 不再报错",
        "file": "crates/trit-circuit/src/circuit/validate.rs",
        "old": "            || bus.width == 0",
        "new": "            || false",
        "pkg": "trit-circuit",
        "filter": "zero_width_bus_is_rejected"
    },
    {
        "name": "verilog: 尺寸常量位序反转",
        "file": "crates/trit-circuit/src/verilog/expr.rs",
        "old": "        'b' => {\n            for ch in digits.chars().rev() {",
        "new": "        'b' => {\n            for ch in digits.chars() {",
        "pkg": "trit-circuit",
        "filter": "sized_literals_preserve_bit_order"
    },
    {
        "name": "verilog: 尺寸常量零扩展成 X",
        "file": "crates/trit-circuit/src/verilog/expr.rs",
        "old": "    bits.resize(width, Ternary::False);",
        "new": "    bits.resize(width, Ternary::Unknown);",
        "pkg": "trit-circuit",
        "filter": "sized_literal_zero_extends_to_wider_vector"
    },
    {
        "name": "netlist: JSON 新版本不拒绝",
        "file": "crates/trit-circuit/src/netlist.rs",
        "old": "    if parsed.format > JSON_FORMAT {",
        "new": "    if false {",
        "pkg": "trit-circuit",
        "filter": "json_format_is_versioned_and_rejects_newer"
    },
    {
        "name": "netlist: gate 参数计数失效",
        "file": "crates/trit-circuit/src/netlist.rs",
        "old": "                if parts.len() != 5 {",
        "new": "                if false {",
        "pkg": "trit-circuit",
        "filter": "reports_line_numbers"
    },
    {
        "name": "atpg: 精确覆盖递归错误清除已覆盖位",
        "file": "crates/trit-circuit/src/coverage.rs",
        "old": "        search_cover(\n            uncovered & !faults,",
        "new": "        search_cover(\n            uncovered & faults,",
        "pkg": "trit-circuit",
        "filter": "budget_exhaustion_with_unexplored_siblings_is_not_optimal",
    },
    {
        "name": "bdd: 节点预算永不触发",
        "file": "crates/trit-bdd/src/encode.rs",
        "old": "    pub(crate) fn guard(&self, bdd: Bdd) -> Result<Bdd, BddError> {\n        if bdd.size() > self.limits.max_nodes {",
        "new": "    pub(crate) fn guard(&self, bdd: Bdd) -> Result<Bdd, BddError> {\n        if false {",
        "pkg": "trit-bdd",
        "filter": "budget_rejects"
    },
    {
        "name": "bdd: 频率降序写反",
        "file": "crates/trit-bdd/src/order.rs",
        "old": "                    cr.cmp(&cl)",
        "new": "                    cl.cmp(&cr)",
        "pkg": "trit-bdd",
        "filter": "frequency_order"
    },
    {
        "name": "bdd: is_valid 退回布尔全集",
        "file": "crates/trit-bdd/src/quantify.rs",
        "old": "        self.bdd.not().and(&self.valid_free).is_false()",
        "new": "        self.bdd.is_true()",
        "pkg": "trit-bdd",
        "filter": "forall_with_free_ternary_variable_is_judged_on_valid_domain"
    },
    {
        "name": "bdd: satisfiable 不限制合法域",
        "file": "crates/trit-bdd/src/equiv.rs",
        "old": "    let sat = enc.guard(enc.valid()?.and(&node.t).and(&node.x.not()))?;",
        "new": "    let sat = enc.guard(node.t.and(&node.x.not()))?;",
        "pkg": "trit-bdd",
        "filter": "satisfiable_requires_a_legal_valuation"
    },
    {
        "name": "verilog: 多输入 NAND/NOR 忘记最终反相",
        "file": "crates//trit-circuit//src//verilog//parse.rs",
        "old": "                        if invert {",
        "new": "                        if false {",
        "pkg": "trit-circuit",
        "filter": "multi_input_nand_nor_are_not_left_folded"
    },
    {
        "name": "atpg: X 位只验证一种填充",
        "file": "crates//trit-circuit//src//atpg//mod.rs",
        "old": "    for fill in 0..(1u64 << xbits.len()) {",
        "new": "    for fill in 0..1 {",
        "pkg": "trit-circuit",
        "filter": "dont_care_bits_are_jointly_fillable"
    },
    {
        "name": "check: 覆盖率无法执行时仍然 PASS",
        "file": "crates//trit-cli//src//commands//check.rs",
        "old": "    if atpg.is_none() && min_coverage > 0.0 {",
        "new": "    if false {",
        "pkg": "trit-cli",
        "filter": "check_fails_closed_when_coverage_cannot_run"
    },
    {
        "name": "aiger: 忽略时序锁存器",
        "file": "crates/trit-circuit/src/aiger.rs",
        "old": "    if latches > 0 {",
        "new": "    if false {",
        "pkg": "trit-circuit",
        "filter": "rejects_latches_strong_xor_and_bad_header"
    },
    {
        "name": "blif: 把 '-' 当成 0 字面量",
        "file": "crates/trit-circuit/src/blif.rs",
        "old": "                        \"-\" => None,",
        "new": "                        \"-\" => Some(false),",
        "pkg": "trit-circuit",
        "filter": "constants_and_dont_care_rows"
    },
    {
        "name": "verilog: assign 的 ^ 退化成强加三值 Xor",
        "file": "crates/trit-circuit/src/verilog/build.rs",
        "old": "                    VExpr::Or(..) => GateKind::Or,\n                    _ => GateKind::XorClassic,",
        "new": "                    VExpr::Or(..) => GateKind::Or,\n                    _ => GateKind::Xor,",
        "pkg": "trit-circuit",
        "filter": "assign_xor_is_not_lukasiewicz_strong_sum"
    },
    {
        "name": "timing: 负载延迟不参与到达时间",
        "file": "crates/trit-circuit/src/timing.rs",
        "old": "        start + u64::from(self.load_delay) * fanout as u64\n    }\n\n    /// 某类门在给定扇出下的延迟",
        "new": "        start\n    }\n\n    /// 某类门在给定扇出下的延迟",
        "pkg": "trit-circuit",
        "filter": "load_delay_accumulates_with_fanout"
    },
    {
        "name": "map: 直接单元到达时间漏加单元延迟",
        "file": "crates/trit-circuit/src/map.rs",
        "old": "            let total = start + u64::from(load_delay) * fanout;",
        "new": "            let total = u64::from(load_delay) * fanout;",
        "pkg": "trit-circuit",
        "filter": "timing_driven_picks_lower_arrival_implementation"
    },
    {
        "name": "map: 面积策略按延迟排序",
        "file": "crates/trit-circuit/src/map.rs",
        "old": "                MapStrategy::Area => (candidate.area as u64, candidate.arrival as u64),",
        "new": "                MapStrategy::Area => (candidate.arrival as u64, candidate.area as u64),",
        "pkg": "trit-circuit",
        "filter": "timing_driven_picks_lower_arrival_implementation"
    },
    {
        "name": "parser: 二元链不累计深度",
        "file": "crates/trit-core/src/parser.rs",
        "old": "        let depth = 1 + left.1.max(right.1);\n",
        "new": "        let depth = 1;\n",
        "pkg": "trit-core",
        "filter": "rejects_long_flat_binary_chain"
    },
    {
        "name": "parser: 解析期节点预算失效",
        "file": "crates/trit-core/src/parser.rs",
        "old": "        if self.nodes > self.limits.max_nodes {\n",
        "new": "        if false {\n",
        "pkg": "trit-core",
        "filter": "node_budget_is_enforced_while_parsing"
    },
    {
        "name": "parser: 输入字节上限失效",
        "file": "crates/trit-core/src/parser.rs",
        "old": "        if input.len() > MAX_INPUT_BYTES {\n",
        "new": "        if false {\n",
        "pkg": "trit-core",
        "filter": "oversized_input_is_rejected_before_collecting_chars"
    },
    {
        "name": "core: TriVec 相等忽略 unknown 平面",
        "file": "crates/trit-core/src/bulk.rs",
        "old": "            if *ones != other.ones[w] || left != right {\n",
        "new": "            if *ones != other.ones[w] {\n",
        "pkg": "trit-core",
        "filter": "eq_compares_values_not_storage"
    },
    {
        "name": "core: TriVec 位平面不足不报错",
        "file": "crates/trit-core/src/bulk.rs",
        "old": "        if ones.len() < need {\n",
        "new": "        if false {\n",
        "pkg": "trit-core",
        "filter": "fallible_constructors_and_accessors_report_bounds"
    },
    {
        "name": "core: 清空最后一个 Unknown 不规范化平面",
        "file": "crates/trit-core/src/bulk.rs",
        "old": "        if cleared_last_unknown {\n            self.unknowns = None;\n        }\n",
        "new": "        if false {\n            self.unknowns = None;\n        }\n",
        "pkg": "trit-core",
        "filter": "eq_compares_values_not_storage"
    },
    {
        "name": "core: 枚举变量数预算失效",
        "file": "crates/trit-core/src/bits.rs",
        "old": "        if self.variables.len() > limits.max_vars {\n",
        "new": "        if false {\n",
        "pkg": "trit-core",
        "filter": "evaluate_within_enforces_var_budget_directly"
    },
    {
        "name": "eval: 解码忽略剩余高位",
        "file": "crates/trit-eval/src/eval.rs",
        "old": "    if t != 0 {\n        return Err(AssignmentCodecError::OutOfDomain {\n",
        "new": "    if false {\n        return Err(AssignmentCodecError::OutOfDomain {\n",
        "pkg": "trit-eval",
        "filter": "checked_codec_rejects_out_of_domain_and_duplicates"
    },
    {
        "name": "eval: 编码对高位零也报溢出",
        "file": "crates/trit-eval/src/eval.rs",
        "old": "        if digit == 0 {\n            continue;\n        }\n",
        "new": "        if digit == 0 {\n        }\n",
        "pkg": "trit-eval",
        "filter": "assignment_index_does_not_over_reject_high_zero_digits"
    },
    {
        "name": "policy: 未引用环不报错",
        "file": "crates/trit-policy/src/lib.rs",
        "old": "                        return Err(PolicyError::Cycle(cycle.join(\" -> \")));\n",
        "new": "                        let _ = cycle;\n",
        "pkg": "trit-policy",
        "filter": "unused_definition_cycle_is_rejected_before_expansion"
    },
    {
        "name": "policy: binary 允许派生定义",
        "file": "crates/trit-policy/src/lib.rs",
        "old": "        if definitions.contains_key(name) {\n",
        "new": "        if false && definitions.contains_key(name) {\n",
        "pkg": "trit-policy",
        "filter": "binary_names_must_be_original_variables"
    },
    {
        "name": "circuit: 组合数溢出后饱和",
        "file": "crates//trit-circuit//src//circuit//types.rs",
        "old": "        checked_pow3(self.input_count())\n",
        "new": "        Ok(checked_pow3(self.input_count()).unwrap_or(u64::MAX))\n",
        "pkg": "trit-circuit",
        "filter": "analyze_rejects_too_many_inputs_before_enumeration"
    },
    {
        "name": "timing: 到达时间忽略引脚弧",
        "file": "crates/trit-circuit/src/timing.rs",
        "old": "            .map(|(index, arrival)| {\n                u64::from(*arrival) + u64::from(self.arc_delay_for_gate(gate, index))\n            })",
        "new": "            .map(|(_index, arrival)| u64::from(*arrival))",
        "pkg": "trit-circuit",
        "filter": "pin_arcs_change_arrival_and_critical_path"
    },
    {
        "name": "timing: 关键路径忽略引脚弧",
        "file": "crates/trit-circuit/src/timing.rs",
        "old": "                    let arc = u64::from(model.arc_delay_for_gate(gate, input_index));\n",
        "new": "                    let arc = 0u64;\n",
        "pkg": "trit-circuit",
        "filter": "pin_arcs_change_arrival_and_critical_path"
    },
    {
        "name": "map: 直接单元忽略引脚弧",
        "file": "crates/trit-circuit/src/map.rs",
        "old": "                let at = u64::from(arrival.get(signal.name()).copied().unwrap_or(0))\n                    + u64::from(library.arc_delay(gate.kind, input_index).unwrap_or(0));\n",
        "new": "                let at = u64::from(arrival.get(signal.name()).copied().unwrap_or(0));\n",
        "pkg": "trit-circuit",
        "filter": "pin_arcs_flip_implementation_choice"
    },
    {
        "name": "map: 弧数校验放宽",
        "file": "crates/trit-circuit/src/map.rs",
        "old": "                && arcs.len() != kind.arity()\n",
        "new": "                && arcs.len() > kind.arity()\n",
        "pkg": "trit-circuit",
        "filter": "cell_library_accepts_arc_form_and_validates"
    },
    {
        "name": "atpg: X 合并忽略唯一覆盖（保留全部故障）",
        "file": "crates//trit-circuit//src//atpg//compact.rs",
        "old": "                        need.retain(|fault| !detected.contains(fault));",
        "new": "                        need.retain(|_fault| true);",
        "pkg": "trit-circuit",
        "filter": "compaction_merges_witness_patterns"
    },
    {
        "name": "bridge: wired-OR 被当成 wired-AND",
        "file": "crates/trit-circuit/src/bridge.rs",
        "old": "            Self::Or => Ternary::True,\n",
        "new": "            Self::Or => Ternary::False,\n",
        "pkg": "trit-circuit",
        "filter": "and_bridge_is_detected_only_for_wired_or"
    },
    {
        "name": "bridge: 两端值恒等导致全部不可测",
        "file": "crates/trit-circuit/src/bridge.rs",
        "old": "                .map(|&(a, b, _)| (good[a], good[b]))\n",
        "new": "                .map(|&(a, _b, _)| (good[a], good[a]))\n",
        "pkg": "trit-circuit",
        "filter": "and_bridge_is_detected_only_for_wired_or"
    },
    {
        "name": "bridge: 空列表不失败",
        "file": "crates/trit-circuit/src/bridge.rs",
        "old": "    if pairs.is_empty() {\n        return Err(BridgeError::NoPairs);\n    }\n    let inputs = circuit.input_count();\n    if inputs > 16 {",
        "new": "    if false {\n        return Err(BridgeError::NoPairs);\n    }\n    let inputs = circuit.input_count();\n    if inputs > 16 {",
        "pkg": "trit-circuit",
        "filter": "validation_rejects_empty_unknown_and_same_pairs"
    },
    {
        "name": "bridge: 同一网络不报错",
        "file": "crates/trit-circuit/src/bridge.rs",
        "old": "        if a == b {\n",
        "new": "        if false {\n",
        "pkg": "trit-circuit",
        "filter": "validation_rejects_empty_unknown_and_same_pairs"
    },
    {
        "name": "verilog: 向量位选择恒取 bit 0",
        "file": "crates//trit-circuit//src//verilog//expr.rs",
        "old": "                    return Ok(VExpr::Var(format!(\"{name}[{index}]\")));",
        "new": "                    return Ok(VExpr::Var(format!(\"{name}[0]\")));",
        "pkg": "trit-circuit",
        "filter": "parses_bit_selects_and_concatenation"
    },
    {
        "name": "verilog: 拼接按 MSB-first 误解",
        "file": "crates//trit-circuit//src//verilog//expr.rs",
        "old": "            for item in items.iter().rev() {\n",
        "new": "            for item in items.iter() {\n",
        "pkg": "trit-circuit",
        "filter": "parses_bit_selects_and_concatenation"
    },
    {
        "name": "verilog: 向量端口只展开 bit 0",
        "file": "crates/trit-circuit/src/verilog/decl.rs",
        "old": "                for index in order {",
        "new": "                for index in [0] {",
        "pkg": "trit-circuit",
        "filter": "parses_vector_ports_and_vectorized_bitwise_assign"
    },
    {
        "name": "verilog: 导出不分组向量端口",
        "file": "crates//trit-circuit//src//verilog//export.rs",
        "old": "            (max, true, bus.ascending)",
        "new": "            (max, false, bus.ascending)",
        "pkg": "trit-circuit",
        "filter": "vector_round_trip_regroups_ports"
    },
    {
        "name": "verilog: 右值深度预算失效",
        "file": "crates//trit-circuit//src//verilog//expr.rs",
        "old": "        if self.depth > MAX_RHS_DEPTH {\n",
        "new": "        if false {\n",
        "pkg": "trit-circuit",
        "filter": "deep_rhs_is_rejected"
    },
    {
        "name": "hierarchy: 所有模块都当作顶层",
        "file": "crates/trit-circuit/src/verilog/hierarchy.rs",
        "old": "        .filter(|def| !instantiated.contains(def.name.as_str()))\n",
        "new": "        .filter(|_def| true)\n",
        "pkg": "trit-circuit",
        "filter": "hierarchical_named_connections_can_be_reordered"
    },
    {
        "name": "hierarchy: 位置连接按逆序绑定",
        "file": "crates/trit-circuit/src/verilog/hierarchy.rs",
        "old": "                    let Some(port) = child.ports.get(index) else {\n",
        "new": "                    let Some(port) = child.ports.get(child.ports.len().saturating_sub(1 + index)) else {\n",
        "pkg": "trit-circuit",
        "filter": "hierarchical_positional_connections_bind_by_port_order"
    },
    {
        "name": "hierarchy: 生成名冲突不报错",
        "file": "crates/trit-circuit/src/verilog/hierarchy.rs",
        "old": "    if used.contains(&renamed) {\n",
        "new": "    if false && used.contains(&renamed) {\n",
        "pkg": "trit-circuit",
        "filter": "hierarchical_generated_names_do_not_collide_with_user_signals"
    },
    {
        "name": "hierarchy: 向量端口宽度不校验",
        "file": "crates/trit-circuit/src/verilog/hierarchy.rs",
        "old": "    if parent_width != child_width {\n",
        "new": "    if false && parent_width != child_width {\n",
        "pkg": "trit-circuit",
        "filter": "hierarchical_vector_width_mismatch_is_rejected"
    },
    {
        "name": "hierarchy: 向量端口接受单比特连接",
        "file": "crates/trit-circuit/src/verilog/hierarchy.rs",
        "old": "    if index.is_some() {\n",
        "new": "    if false && index.is_some() {\n",
        "pkg": "trit-circuit",
        "filter": "hierarchical_vector_port_rejects_single_bit"
    },
    {
        "name": "hierarchy: 向量 bus 逆序连线",
        "file": "crates/trit-circuit/src/verilog/hierarchy.rs",
        "old": "            let parent_index = if parent_asc {\n",
        "new": "            let parent_index = if true || parent_asc {\n",
        "pkg": "trit-circuit",
        "filter": "hierarchical_vector_bit_order_is_preserved"
    },
    {
        "name": "verilog: 升序向量按降序取位",
        "file": "crates/trit-circuit/src/verilog/expr.rs",
        "old": "                let declared = if ascending.contains(name) {\n",
        "new": "                let declared = if false && ascending.contains(name) {\n",
        "pkg": "trit-circuit",
        "filter": "ascending_vector_bit_order_matches_verilog"
    },
    {
        "name": "verilog: 升序 LHS 不按位序落位",
        "file": "crates/trit-circuit/src/verilog/parse.rs",
        "old": "                                    let lhs_index = if ascending.contains(&lhs_base) {\n",
        "new": "                                    let lhs_index = if false && ascending.contains(&lhs_base) {\n",
        "pkg": "trit-circuit",
        "filter": "literal_assignment_to_ascending_vector_keeps_verilog_order"
    },
    {
        "name": "verilog: 导出升序向量写成降序",
        "file": "crates/trit-circuit/src/verilog/export.rs",
        "old": "        if group.ascending {\n",
        "new": "        if false {\n",
        "pkg": "trit-circuit",
        "filter": "ascending_ranges_parse_export_and_json_round_trip"
    },
    {
        "name": "hierarchy: 跨方向 bus 不按子端口位序",
        "file": "crates/trit-circuit/src/verilog/hierarchy.rs",
        "old": "            let child_index = if child_asc {\n",
        "new": "            let child_index = if false && child_asc {\n",
        "pkg": "trit-circuit",
        "filter": "hierarchical_cross_direction_bus_maps_by_position"
    },
    {
        "name": "sat: 常量 false 被强制成真",
        "file": "crates/trit-circuit/src/sat_cnf.rs",
        "old": "        solver.add_clause(&[Lit::from_var(false_var, false)]);\n        let mut encoded = Self {",
        "new": "        solver.add_clause(&[Lit::from_var(false_var, true)]);\n        let mut encoded = Self {",
        "pkg": "trit-circuit",
        "filter": "sat_matches_exhaustive_on_random_small_circuits"
    },
    {
        "name": "sat: 不要求输出不同",
        "file": "crates/trit-circuit/src/sat_cnf.rs",
        "old": "        self.solver.add_clause(&differences);\n",
        "new": "        if false { self.solver.add_clause(&differences); }\n",
        "pkg": "trit-circuit",
        "filter": "sat_matches_exhaustive_on_random_small_circuits"
    },
    {
        "name": "sat: 故障引脚不注入",
        "file": "crates/trit-circuit/src/sat_cnf.rs",
        "old": "            let lit = if faulty && inject.is_some_and(|(pin, _)| pin == index) {\n",
        "new": "            let lit = if false && faulty && inject.is_some_and(|(pin, _)| pin == index) {\n",
        "pkg": "trit-circuit",
        "filter": "sat_matches_exhaustive_on_random_small_circuits"
    },
    {
        "name": "sat: Xor 一律按经典 XOR",
        "file": "crates/trit-circuit/src/sat_cnf.rs",
        "old": "                    GateKind::Xor => match self.alg {\n                        // Ł3 强加法在 0/1 上是 OR；K3/B3 是 AND。\n                        Algebra::Lukasiewicz => self.or(y, a, b),\n                        Algebra::Kleene | Algebra::Bochvar => self.and(y, a, b),\n                    },\n",
        "new": "                    GateKind::Xor => self.xor(y, a, b),\n",
        "pkg": "trit-circuit",
        "filter": "sat_encodes_strong_xor_by_algebra"
    },
    {
        "name": "bridge: 不要求两端取值不同",
        "file": "crates/trit-circuit/src/bridge.rs",
        "old": "    cnf.require_good_difference(&pair.a, &pair.b)?;",
        "new": "    if false { cnf.require_good_difference(&pair.a, &pair.b)?; }",
        "pkg": "trit-circuit",
        "filter": "sat_bridge_matches_exhaustive_for_small_circuits"
    },
    {
        "name": "bridge: wired 值反转",
        "file": "crates/trit-circuit/src/bridge.rs",
        "old": "    let value = matches!(pair.mode, BridgeMode::Or);",
        "new": "    let value = !matches!(pair.mode, BridgeMode::Or);",
        "pkg": "trit-circuit",
        "filter": "sat_bridge_matches_exhaustive_for_small_circuits"
    },
    {
        "name": "bridge: 不要求输出不同",
        "file": "crates/trit-circuit/src/bridge.rs",
        "old": "    cnf.require_output_difference()?;",
        "new": "    if false { cnf.require_output_difference()?; }",
        "pkg": "trit-circuit",
        "filter": "sat_bridge_matches_exhaustive_for_small_circuits"
    },
    {
        "name": "map: area-first 忽略 area",
        "file": "crates/trit-circuit/src/map.rs",
        "old": "                MapStrategy::Area => (candidate.area as u64, candidate.arrival as u64),\n",
        "new": "                MapStrategy::Area => (candidate.cells as u64, candidate.arrival as u64),\n",
        "pkg": "trit-circuit",
        "filter": "area_first_uses_declared_area"
    },
    {
        "name": "timing: 忽略输入到达约束",
        "file": "crates/trit-circuit/src/timing.rs",
        "old": "            let arrival = constraints\n                .input_arrival\n                .get(signal.name())\n                .copied()\n                .unwrap_or(0);\n",
        "new": "            let arrival = 0;\n",
        "pkg": "trit-circuit",
        "filter": "input_arrival_and_output_required_constraints"
    },
    {
        "name": "timing: 忽略逐输出 required",
        "file": "crates/trit-circuit/src/timing.rs",
        "old": "            let required = constraints\n                .output_required\n                .get(signal.name())\n                .copied()\n                .unwrap_or(clock);\n",
        "new": "            let required = clock;\n",
        "pkg": "trit-circuit",
        "filter": "per_output_required_times_are_independent"
    },
    {
        "name": "timing: --paths 忽略输入到达",
        "file": "crates/trit-circuit/src/timing.rs",
        "old": "        let arrival = constraints\n            .input_arrival\n            .get(input.name())\n            .copied()\n            .unwrap_or(0);\n",
        "new": "        let arrival = 0;\n",
        "pkg": "trit-circuit",
        "filter": "input_arrival_and_output_required_constraints"
    },
    {
        "name": "sdf: 取最小延迟",
        "file": "crates/trit-circuit/src/sdf.rs",
        "old": "                    worst = worst.max(delay);\n",
        "new": "                    worst = worst.min(delay);\n",
        "pkg": "trit-circuit",
        "filter": "sdf::tests::parses_and_rounds_up_the_worst_arc"
    },
    {
        "name": "sdf: 向下取整",
        "file": "crates/trit-circuit/src/sdf.rs",
        "old": "    let rounded = value.ceil();",
        "new": "    let rounded = value.floor();",
        "pkg": "trit-circuit",
        "filter": "sdf::tests::parses_and_rounds_up_the_worst_arc"
    },
    {
        "name": "sdf: 跨实例取最小",
        "file": "crates/trit-circuit/src/sdf.rs",
        "old": "                out.entry(cell.to_string())\n                    .and_modify(|current| *current = current.max(worst))\n                    .or_insert(worst);",
        "new": "                out.entry(cell.to_string())\n                    .and_modify(|current| *current = current.min(worst))\n                    .or_insert(worst);",
        "pkg": "trit-circuit",
        "filter": "sdf::tests::takes_the_max_across_instances_and_conditional_arcs"
    },
    {
        "name": "sdf: 放行负值",
        "file": "crates/trit-circuit/src/sdf.rs",
        "old": "        if !value.is_finite() || value < 0.0 {\n",
        "new": "        if !value.is_finite() {\n",
        "pkg": "trit-circuit",
        "filter": "sdf::tests::rejects_bad_syntax_and_missing_delays"
    },
    {
        "name": "shared: 候选不注入常量",
        "file": "crates/trit-circuit/src/sat_cnf.rs",
        "old": "                    injected[pin] = enc.const_lit(stuck);\n",
        "new": "                    injected[pin] = faulty_inputs[pin];\n",
        "pkg": "trit-circuit",
        "filter": "shared_cnf_matches_per_fault_semantics"
    },
    {
        "name": "shared: mux 选反",
        "file": "crates/trit-circuit/src/sat_cnf.rs",
        "old": "                        Lit::from_var(candidate, true),\n                        Lit::from_var(current, true),\n",
        "new": "                        Lit::from_var(current, true),\n                        Lit::from_var(current, true),\n",
        "pkg": "trit-circuit",
        "filter": "shared_cnf_matches_per_fault_semantics"
    },
    {
        "name": "shared: 不假设故障 selector",
        "file": "crates/trit-circuit/src/sat_cnf.rs",
        "old": "        self.enc.solver.assume(&[selector]);\n",
        "new": "        let _ = selector;\n",
        "pkg": "trit-circuit",
        "filter": "shared_cnf_matches_per_fault_semantics"
    },
    {
        "name": "params: 忽略实例覆盖",
        "file": "crates/trit-circuit/src/verilog/params.rs",
        "old": "            Some(value) => *value,\n",
        "new": "            Some(_) => eval_expr(&param.default, &params)?,\n",
        "pkg": "trit-circuit",
        "filter": "named_parameter_override_changes_width"
    },
    {
        "name": "params: 不替换参数",
        "file": "crates/trit-circuit/src/verilog/params.rs",
        "old": "            if let Some(value) = params.get(&name) {\n",
        "new": "            if let Some(value) = params.get(\"__never__\") {\n",
        "pkg": "trit-circuit",
        "filter": "parses_parameterized_module_defaults"
    },
    {
        "name": "params: 不归一化位宽",
        "file": "crates/trit-circuit/src/verilog/params.rs",
        "old": "            let left = eval_expr(left, &empty)?;\n            out.push('[');\n            out.push_str(&left.to_string());\n            if let Some(right) = right {\n                let right = eval_expr(right, &empty)?;\n                out.push(':');\n                out.push_str(&right.to_string());\n            }\n            out.push(']');\n",
        "new": "            let left = eval_expr(left, &empty)?;\n            out.push('[');\n            out.push_str(&left.to_string());\n            out.push(']');\n",
        "pkg": "trit-circuit",
        "filter": "parses_parameterized_module_defaults"
    },
    {
        "name": "params: 位置覆盖错位",
        "file": "crates/trit-circuit/src/verilog/params.rs",
        "old": "            let Some(param) = child.params.get(positional) else {\n",
        "new": "            let Some(param) = child.params.get(positional + 1) else {\n",
        "pkg": "trit-circuit",
        "filter": "parent_parameter_flows_to_positional_child_override"
    },
    {
        "name": "sdf: 不按实例覆盖",
        "file": "crates/trit-circuit/src/sdf.rs",
        "old": "        if let Some(delay) = best {\n            model\n                .instance_delays\n                .insert(gate.output.name().to_string(), delay);\n        }\n",
        "new": "        if let Some(_delay) = best {}\n",
        "pkg": "trit-circuit",
        "filter": "instance_delays_override_celltype_in_sta"
    },
    {
        "name": "sdf: 实例路径只做全等",
        "file": "crates/trit-circuit/src/sdf.rs",
        "old": "    if sdf == gate {\n        return true;\n    }\n    sdf.strip_suffix(gate)\n        .is_some_and(|prefix| prefix.ends_with('.') || prefix.ends_with('/'))\n",
        "new": "    sdf == gate\n",
        "pkg": "trit-circuit",
        "filter": "instance_path_accepts_a_design_prefix"
    },
    {
        "name": "schedule: 忽略 Unknown 依赖",
        "file": "crates/trit-schedule/src/lib.rs",
        "old": "                Ternary::Unknown => {\n                    if direct_unknown[position].is_none() {\n                        direct_unknown[position] = Some(dependency.task.clone());\n                    }\n                }\n",
        "new": "                Ternary::Unknown => {}\n",
        "pkg": "trit-schedule",
        "filter": "unknown_dependency_defers_transitively"
    },
    {
        "name": "schedule: 不传播延迟",
        "file": "crates/trit-schedule/src/lib.rs",
        "old": "        for &predecessor in predecessors {\n            dependents[predecessor].push(position);",
        "new": "        for &predecessor in predecessors {\n            let _ = (predecessor, position);\n",
        "pkg": "trit-schedule",
        "filter": "unknown_dependency_defers_transitively"
    },
    {
        "name": "schedule: 关键路径忽略前置",
        "file": "crates/trit-schedule/src/lib.rs",
        "old": "                start = start.max(longest[predecessor]);\n",
        "new": "                start = 0;\n",
        "pkg": "trit-schedule",
        "filter": "chain_uses_one_worker_and_full_critical_path"
    },
    {
        "name": "schedule: 优先级取反",
        "file": "crates/trit-schedule/src/lib.rs",
        "old": "    (task.priority, task.duration, task.id.clone(), position)\n",
        "new": "    (-task.priority, task.duration, task.id.clone(), position)\n",
        "pkg": "trit-schedule",
        "filter": "higher_priority_is_scheduled_first"
    },
    {
        "name": "runtime: 忽略取消",
        "file": "crates/trit-schedule/src/runtime/execute.rs",
        "old": "        if cancel.is_cancelled() && !state.stop {\n",
        "new": "        if false && !state.stop {\n",
        "pkg": "trit-schedule",
        "filter": "cancellation_prevents_unstarted_tasks"
    },
    {
        "name": "runtime: 依赖完成不传播",
        "file": "crates/trit-schedule/src/runtime/execute.rs",
        "old": "                            state.indegree[next] -= 1;\n",
        "new": "                            state.indegree[next] = 0;\n",
        "pkg": "trit-schedule",
        "filter": "join_waits_for_all_predecessors"
    },
    {
        "name": "runtime: 失败后继续调度",
        "file": "crates/trit-schedule/src/runtime/execute.rs",
        "old": "                Err(message) => {\n                    state.failed = Some(TaskFailure { task: id, message });\n                    state.stop = true;\n",
        "new": "                Err(message) => {\n                    state.failed = Some(TaskFailure { task: id, message });\n                    state.stop = false;\n",
        "pkg": "trit-schedule",
        "filter": "failure_does_not_start_new_work"
    },
    {
        "name": "runtime: 忽略资源配额",
        "file": "crates/trit-schedule/src/runtime/execute.rs",
        "old": "            state\n                .capacity\n                .is_none_or(|capacity| state.requirements[*position].fits_in(capacity, state.used))\n",
        "new": "            true\n",
        "pkg": "trit-schedule",
        "filter": "quota_serializes_tasks_that_do_not_fit_together"
    },
    {
        "name": "runtime: 资源占用不累计",
        "file": "crates/trit-schedule/src/runtime/execute.rs",
        "old": "            state.running += 1;\n            state.started += 1;\n            state.used = new_used;",
        "new": "            state.running += 1;\n            state.started += 1;\n            state.used = requirements;",
        "pkg": "trit-schedule",
        "filter": "quota_accumulates_across_tasks"
    },
    {
        "name": "runtime: 超容量不拒绝",
        "file": "crates/trit-schedule/src/runtime/execute.rs",
        "old": "            if !tasks[position]\n                .requirements\n                .fits_in(capacity, Resources::default())\n            {\n",
        "new": "            if false {\n",
        "pkg": "trit-schedule",
        "filter": "task_exceeding_capacity_is_rejected"
    },
    {
        "name": "resumable: 抢占丢失状态",
        "file": "crates/trit-schedule/src/resumable.rs",
        "old": "    tasks[victim] = Some(task);\n",
        "new": "    let _ = task;\n",
        "pkg": "trit-schedule",
        "filter": "preempted_task_resumes_from_its_own_state"
    },
    {
        "name": "resumable: 抢占优先级过滤反了",
        "file": "crates/trit-schedule/src/resumable.rs",
        "old": "        .filter(|position| priorities[*position] < high_priority)\n",
        "new": "        .filter(|position| priorities[*position] > high_priority)\n",
        "pkg": "trit-schedule",
        "filter": "preempted_task_resumes_from_its_own_state"
    },
    {
        "name": "resumable: 失败后不清理后续",
        "file": "crates/trit-schedule/src/resumable.rs",
        "old": "\n    cancel_pending(&mut ready, &mut running, &mut tasks, &mut cancelled, &ids);\n",
        "new": "\n    if failed.is_none() { cancel_pending(&mut ready, &mut running, &mut tasks, &mut cancelled, &ids); }\n",
        "pkg": "trit-schedule",
        "filter": "failure_and_unknown_dependencies_are_reported"
    },
    {
        "name": "threaded: 抢占丢失状态",
        "file": "crates/trit-schedule/src/resumable_threaded.rs",
        "old": "                        state.tasks[position] = Some(task);\n",
        "new": "                        let _ = task;\n",
        "pkg": "trit-schedule",
        "filter": "threaded_preemption_keeps_task_state"
    },
    {
        "name": "threaded: 抢占优先级过滤反了",
        "file": "crates/trit-schedule/src/resumable_threaded.rs",
        "old": "                        *p > priority && (state.idle == 0 || !might_fit(&state, *high))\n",
        "new": "                        *p < priority && (state.idle == 0 || !might_fit(&state, *high))\n",
        "pkg": "trit-schedule",
        "filter": "threaded_preemption_keeps_task_state"
    },
    {
        "name": "metrics: started 不唯一",
        "file": "crates/trit-schedule/src/resumable_threaded.rs",
        "old": "    if state.started_tasks.insert(position) {\n",
        "new": "    if true {\n",
        "pkg": "trit-schedule",
        "filter": "threaded_preemption_keeps_task_state"
    },
    {
        "name": "metrics: Prometheus completed 写成 started",
        "file": "crates/trit-schedule/src/runtime/types.rs",
        "old": "            (\"completed\", \"Tasks completed.\", self.completed as u64),\n",
        "new": "            (\"completed\", \"Tasks completed.\", self.started as u64),\n",
        "pkg": "trit-schedule",
        "filter": "prometheus_exports_all_fields"
    },
    {
        "name": "metrics: Prometheus 峰值内存写成 CPU",
        "file": "crates/trit-schedule/src/runtime/types.rs",
        "old": "                self.peak_usage.memory.into(),\n",
        "new": "                self.peak_usage.cpu.into(),\n",
        "pkg": "trit-schedule",
        "filter": "prometheus_exports_all_fields"
    },
    {
        "name": "checkpoint: 版本校验失效",
        "file": "crates/trit-schedule/src/checkpoint/mod.rs",
        "old": "        if self.version == CHECKPOINT_VERSION {\n",
        "new": "        if true {\n",
        "pkg": "trit-schedule",
        "filter": "rejects_unknown_version_and_unknown_task"
    },
    {
        "name": "checkpoint: 保存空状态",
        "file": "crates/trit-schedule/src/checkpoint/mod.rs",
        "old": "    Ok(Checkpoint::new(task_id, task.save_state()?))\n",
        "new": "    Ok(Checkpoint::new(task_id, Vec::new()))\n",
        "pkg": "trit-schedule",
        "filter": "checkpoint_round_trips_progress"
    },
    {
        "name": "checkpoint: 恢复不校验版本",
        "file": "crates/trit-schedule/src/checkpoint/mod.rs",
        "old": "    checkpoint.validate()?;\n    restorer.restore(&checkpoint.task_id, &checkpoint.state)\n",
        "new": "    restorer.restore(&checkpoint.task_id, &checkpoint.state)\n",
        "pkg": "trit-schedule",
        "filter": "rejects_unknown_version_and_unknown_task"
    },
    {
        "name": "file checkpoint: 保存不校验版本",
        "file": "crates/trit-schedule/src/checkpoint/file.rs",
        "old": "    pub fn save(&self, checkpoint: &Checkpoint) -> Result<u64, CheckpointError> {\n        checkpoint.validate()?;\n",
        "new": "    pub fn save(&self, checkpoint: &Checkpoint) -> Result<u64, CheckpointError> {\n",
        "pkg": "trit-schedule",
        "filter": "file_store_rejects_bad_version_without_writing"
    },
    {
        "name": "file checkpoint: 加载取最旧",
        "file": "crates/trit-schedule/src/checkpoint/file.rs",
        "old": "    files.sort();\n    Ok(files.pop())\n",
        "new": "    files.sort();\n    Ok(files.into_iter().next())\n",
        "pkg": "trit-schedule",
        "filter": "file_store_round_trips_versions_and_escapes_ids"
    },
    {
        "name": "file checkpoint: prune 保留最旧",
        "file": "crates/trit-schedule/src/checkpoint/file.rs",
        "old": "        for path in files.into_iter().take(remove) {\n",
        "new": "        for path in files.into_iter().skip(keep) {\n",
        "pkg": "trit-schedule",
        "filter": "file_store_prunes_and_removes"
    },
    {
        "name": "index: 不解码 task id",
        "file": "crates/trit-schedule/src/checkpoint/file.rs",
        "old": "                let Ok(task_id) = hex_decode(name) else {\n                    continue;\n                };",
        "new": "                let task_id = name.to_string();",
        "pkg": "trit-schedule",
        "filter": "index_lists_latest_generations_sorted"
    },
    {
        "name": "index: 倒序",
        "file": "crates/trit-schedule/src/checkpoint/file.rs",
        "old": "        entries.sort_by(|left, right| left.task_id.cmp(&right.task_id));\n",
        "new": "        entries.sort_by(|left, right| right.task_id.cmp(&left.task_id));\n",
        "pkg": "trit-schedule",
        "filter": "index_lists_latest_generations_sorted"
    },
    {
        "name": "index: state_bytes 取代号",
        "file": "crates/trit-schedule/src/checkpoint/file.rs",
        "old": "                let state_bytes = fs::metadata(&file).map_err(io_error)?.len();\n",
        "new": "                let state_bytes = generation;\n",
        "pkg": "trit-schedule",
        "filter": "index_lists_latest_generations_sorted"
    },
    {
        "name": "lock: 忽略已存在",
        "file": "crates/trit-schedule/src/checkpoint/file.rs",
        "old": "            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),\n",
        "new": "            Err(_) if false => Ok(None),\n",
        "pkg": "trit-schedule",
        "filter": "cross_process_lock_excludes_and_releases"
    },
    {
        "name": "lock: Drop 不释放",
        "file": "crates/trit-schedule/src/checkpoint/lock.rs",
        "old": "impl Drop for CheckpointLock {\n    fn drop(&mut self) {\n        // 只删仍是自己的那把：陈旧回收/人工删锁之后，同路径可能已是别人的锁目录。\n        if read_owner(&self.path).as_ref() == Some(&self.owner) {\n            let _ = fs::remove_dir_all(&self.path);\n        }\n    }\n}\n",
        "new": "impl Drop for CheckpointLock {\n    fn drop(&mut self) {\n        let _ = &self.path;\n    }\n}\n",
        "pkg": "trit-schedule",
        "filter": "cross_process_lock_excludes_and_releases"
    },
    {
        "name": "lock: Drop 删掉别人的锁",
        "file": "crates/trit-schedule/src/checkpoint/lock.rs",
        "old": "        if read_owner(&self.path).as_ref() == Some(&self.owner) {\n            let _ = fs::remove_dir_all(&self.path);\n        }",
        "new": "        let _ = fs::remove_dir_all(&self.path);",
        "pkg": "trit-schedule",
        "filter": "drop_does_not_delete_a_taken_over_lock"
    },
    {
        "name": "checkpoint: 放行空 task id",
        "file": "crates/trit-schedule/src/checkpoint/file.rs",
        "old": "    if task_id.is_empty() {",
        "new": "    if false {",
        "pkg": "trit-schedule",
        "filter": "empty_task_id_is_rejected_by_every_entry"
    },
    {
        "name": "restore: 忽略检查点",
        "file": "crates/trit-schedule/src/checkpoint/mod.rs",
        "old": "            let task = match source.checkpoint(&def.id)? {\n",
        "new": "            let task = match None::<Checkpoint> {\n",
        "pkg": "trit-schedule",
        "filter": "memory_source_resumes_mid_task"
    },
    {
        "name": "restore: 无检查点不建新任务",
        "file": "crates/trit-schedule/src/checkpoint/mod.rs",
        "old": "                None => factory.create(&def.id)?,\n",
        "new": "                None => return Err(CheckpointError::UnknownTask(def.id.clone())),\n",
        "pkg": "trit-schedule",
        "filter": "missing_checkpoint_uses_factory"
    },
    {
        "name": "tristate: Z 不吸收",
        "file": "crates/trit-core/src/tristate.rs",
        "old": "            (Self::HighZ, value) | (value, Self::HighZ) => Ok(value),\n",
        "new": "            (Self::HighZ, Self::HighZ) => Ok(Self::HighZ),\n",
        "pkg": "trit-core",
        "filter": "resolution_table_and_conflict"
    },
    {
        "name": "tristate: 总线宽度不校验",
        "file": "crates/trit-core/src/tristate.rs",
        "old": "        if self.width() != other.width() {\n",
        "new": "        if false {\n",
        "pkg": "trit-core",
        "filter": "bus_resolves_lane_by_lane_and_checks_widths"
    },
    {
        "name": "exact: 超预算仍枚举",
        "file": "crates/trit-circuit/src/circuit/sim.rs",
        "old": "    if total > max_completions {\n",
        "new": "    if false {\n",
        "pkg": "trit-circuit",
        "filter": "exact_propagation_falls_back_over_budget"
    },
    {
        "name": "exact: 缺失输入不补 X",
        "file": "crates/trit-circuit/src/circuit/sim.rs",
        "old": "        normalized\n            .entry(input.name().to_string())\n            .or_insert(Ternary::Unknown);\n",
        "new": "        let _ = input;\n",
        "pkg": "trit-circuit",
        "filter": "exact_propagation_accepts_partial_vectors_and_merges_values"
    },
    {
        "name": "xpessimism: 单边可满足判成 1",
        "file": "crates/trit-circuit/src/circuit/xpessimism.rs",
        "old": "            (Some(_), None) => XOutcome::Pessimistic {\n                resolved: Ternary::False,\n",
        "new": "            (Some(_), None) => XOutcome::Pessimistic {\n                resolved: Ternary::True,\n",
        "pkg": "trit-circuit",
        "filter": "reconvergent_complement_is_pessimistic_zero"
    },
    {
        "name": "xpessimism: 不做 X 判定直接当已知",
        "file": "crates/trit-circuit/src/circuit/xpessimism.rs",
        "old": "        if !value.is_unknown() {\n            outputs.insert(name.to_string(), XOutcome::Known(value));\n            continue;\n        }\n",
        "new": "        if true {\n            outputs.insert(name.to_string(), XOutcome::Known(value));\n            continue;\n        }\n",
        "pkg": "trit-circuit",
        "filter": "mux_with_equal_data_is_pessimistic_one"
    },
    {
        "name": "xpessimism: 支持集不回溯扇入",
        "file": "crates/trit-circuit/src/circuit/xpessimism.rs",
        "old": "            for input in &gate.inputs {\n                stack.push(input.name());\n            }\n",
        "new": "            let _ = gate;\n",
        "pkg": "trit-circuit",
        "filter": "support_only_covers_the_backward_cone"
    },
    {
        "name": "topology: True 传播也走 Unknown 边",
        "file": "crates/trit-core/src/topology.rs",
        "old": "                if value == Ternary::True && best[next] == Ternary::False {\n                    best[next] = Ternary::True;\n",
        "new": "                if value != Ternary::False && best[next] == Ternary::False {\n                    best[next] = Ternary::True;\n",
        "pkg": "trit-core",
        "filter": "unknown_edge_propagates_unknown_until_true_path_exists"
    },
    {
        "name": "topology: Unknown 传播只走 Unknown 边",
        "file": "crates/trit-core/src/topology.rs",
        "old": "                if value != Ternary::False && best[next] == Ternary::False {\n                    best[next] = Ternary::Unknown;\n",
        "new": "                if value == Ternary::Unknown && best[next] == Ternary::False {\n                    best[next] = Ternary::Unknown;\n",
        "pkg": "trit-core",
        "filter": "unknown_edge_propagates_unknown_until_true_path_exists"
    },
    {
        "name": "topology: 未定判成通过",
        "file": "crates/trit-core/src/topology.rs",
        "old": "            verdict: Verdict::Undetermined,",
        "new": "            verdict: Verdict::Proved,",
        "pkg": "trit-core",
        "filter": "explain_reports_unknown_edges_for_contingent_path"
    },
    {
        "name": "topology: 证书不收集 Unknown 边",
        "file": "crates/trit-core/src/topology.rs",
        "old": "            .filter(|&&node| weak_via[node] == Some(Ternary::Unknown))",
        "new": "            .filter(|&&node| weak_via[node] == Some(Ternary::True))",
        "pkg": "trit-core",
        "filter": "explain_reports_unknown_edges_for_contingent_path"
    },
    {
        "name": "exact: 完成之间不合并差异",
        "file": "crates/trit-circuit/src/circuit/sim.rs",
        "old": "if merged[slot] != *value {",
        "new": "if false {",
        "pkg": "trit-circuit",
        "filter": "exact_propagation_accepts_partial_vectors_and_merges_values"
    },
    {
        "name": "exact: 跨块合并只取第一块",
        "file": "crates/trit-circuit/src/circuit/sim.rs",
        "old": "if left == right {",
        "new": "if true {",
        "pkg": "trit-circuit",
        "filter": "parallel_really_splits_into_chunks"
    },
    {
        "name": "exact: 每块只枚举第一个完成",
        "file": "crates/trit-circuit/src/circuit/sim.rs",
        "old": "for mask in start..end {",
        "new": "for mask in start..end.min(start + 1) {",
        "pkg": "trit-circuit",
        "filter": "exact_propagation_accepts_partial_vectors_and_merges_values"
    },
    {
        "name": "net: 预算边界写成严格小于",
        "file": "crates/trit-net/src/infer.rs",
        "old": ".filter(|total| *total <= budget)",
        "new": ".filter(|total| *total < budget)",
        "pkg": "trit-net",
        "filter": "budget_is_inclusive"
    },
    {
        "name": "net: +1 见证记到另一端",
        "file": "crates/trit-net/src/infer.rs",
        "old": "pos_witness[output] = Some(witness.clone());",
        "new": "neg_witness[output] = Some(witness.clone());",
        "pkg": "trit-net",
        "filter": "ambiguous_output_carries_both_witnesses"
    },
    {
        "name": "net: 区间下界不取端点最小值",
        "file": "crates/trit-net/src/infer.rs",
        "old": "low += low_term.min(high_term);",
        "new": "low += low_term;",
        "pkg": "trit-net",
        "filter": "negative_weight_interval_spans_for_unknown_input"
    },
    {
        "name": "net: 符号函数把负数当零",
        "file": "crates/trit-net/src/infer.rs",
        "old": "    (value > 0) as i32 - (value < 0) as i32",
        "new": "    (value > 0) as i32",
        "pkg": "trit-net",
        "filter": "binary_forward_is_sign_of_weighted_sum"
    },
    {
        "name": "net: 区间压缩下界拿上界算",
        "file": "crates/trit-net/src/infer.rs",
        "old": "            next_lows.push(sign(low));",
        "new": "            next_lows.push(sign(high));",
        "pkg": "trit-net",
        "filter": "cancellation_is_exact_but_interval_is_blind"
    },
    {
        "name": "vm: X 分叉跳到错误位置",
        "file": "crates/trit-machines/src/vm.rs",
        "old": "queue.push((target, stack.clone()));",
        "new": "queue.push((pc + 1, stack.clone()));",
        "pkg": "trit-machines",
        "filter": "branch_on_unknown_with_different_outputs_is_ambiguous"
    },
    {
        "name": "ca: 规则表倒序查",
        "file": "crates/trit-machines/src/ca.rs",
        "old": "next.push(rule.0[sum]);",
        "new": "next.push(rule.0[6 - sum]);",
        "pkg": "trit-machines",
        "filter": "single_seed_spreads_to_the_whole_line"
    },
    {
        "name": "snn: 抑制阈值写成严格小于",
        "file": "crates/trit-machines/src/snn.rs",
        "old": "} else if potential <= -i64::from(network.threshold) {",
        "new": "} else if potential < -i64::from(network.threshold) {",
        "pkg": "trit-machines",
        "filter": "negative_drive_produces_inhibitory_spike"
    },
    {
        "name": "codec: 打包丢掉位权",
        "file": "crates/trit-codec/src/pack.rs",
        "old": "byte += digit(*value) * power;",
        "new": "byte += digit(*value);",
        "pkg": "trit-codec",
        "filter": "five_trits_fit_in_one_byte"
    },
    {
        "name": "codec: 非法字节边界写成大于",
        "file": "crates/trit-codec/src/pack.rs",
        "old": "if u16::from(*byte) >= COMBINATIONS {",
        "new": "if u16::from(*byte) > COMBINATIONS {",
        "pkg": "trit-codec",
        "filter": "invalid_byte_is_rejected"
    },
    {
        "name": "codec: 第 4 列误差幅度取错",
        "file": "crates/trit-codec/src/hamming.rs",
        "old": "    } else {\n        (3, s1 as u8)\n    }",
        "new": "    } else {\n        (3, s2 as u8)\n    }",
        "pkg": "trit-codec",
        "filter": "every_single_symbol_error_is_corrected"
    },
    {
        "name": "codec: 平衡三进制负号解成 +1",
        "file": "crates/trit-codec/src/balanced.rs",
        "old": "            '-' => -1,",
        "new": "            '-' => 1,",
        "pkg": "trit-codec",
        "filter": "small_values"
    },
    {
        "name": "balanced: range 饱和值写成 0",
        "file": "crates/trit-codec/src/balanced.rs",
        "old": "        .map_or(i128::MAX, |power| (power - 1) / 2)",
        "new": "        .map_or(0, |power| (power - 1) / 2)",
        "pkg": "trit-codec",
        "filter": "range_is_exact_up_to_eighty_then_saturates"
    },
    {
        "name": "balanced: 位数上限把 41 位也拒",
        "file": "crates/trit-codec/src/balanced.rs",
        "old": "    if trimmed.chars().count() > 41 {",
        "new": "    if trimmed.chars().count() >= 41 {",
        "pkg": "trit-codec",
        "filter": "decode_accepts_redundant_zeros_but_bounds_the_work"
    },
    {
        "name": "net-format: 不校验魔数",
        "file": "crates/trit-net/src/format.rs",
        "old": "    if magic != MAGIC {",
        "new": "    if false {",
        "pkg": "trit-net",
        "filter": "bad_magic_is_rejected"
    },
    {
        "name": "net-format: 打包长度向上取整改成向下",
        "file": "crates/trit-net/src/format.rs",
        "old": "        let bytes = self.take(trits.div_ceil(5))?;",
        "new": "        let bytes = self.take(trits / 5)?;",
        "pkg": "trit-net",
        "filter": "round_trip_preserves_weights"
    },
    {
        "name": "net-train: 梯度上升",
        "file": "crates/trit-net/src/train.rs",
        "old": "        let scale = options.learning_rate / count;",
        "new": "        let scale = -options.learning_rate / count;",
        "pkg": "trit-net",
        "filter": "majority_five_is_learned"
    },
    {
        "name": "attn: softmax 不归一化",
        "file": "crates/trit-attn/src/scores.rs",
        "old": "    let sum: f32 = exps.iter().sum();",
        "new": "    let sum: f32 = 1.0;",
        "pkg": "trit-attn",
        "filter": "weights_sum_to_one"
    },
    {
        "name": "attn: 记忆编码符号写反",
        "file": "crates/trit-attn/src/memory.rs",
        "old": "                    .checked_add(delta)",
        "new": "                    .checked_sub(delta)",
        "pkg": "trit-attn",
        "filter": "orthogonal_keys_recall_exactly"
    },
    {
        "name": "attn: 三值化不夹边界",
        "file": "crates/trit-attn/src/scores.rs",
        "old": "    value.round().clamp(-1.0, 1.0)",
        "new": "    value.round()",
        "pkg": "trit-attn",
        "filter": "quantize_clamps_to_ternary"
    },
    {
        "name": "learn: STE 饱和门失效",
        "file": "crates/trit-learn/src/quant.rs",
        "old": "                if ratio.abs() <= limit {",
        "new": "                if true {",
        "pkg": "trit-learn",
        "filter": "clipped_ste_zeroes_saturated_gradients"
    },
    {
        "name": "learn: softmax 不归一化",
        "file": "crates/trit-learn/src/loss.rs",
        "old": "    let sum: f64 = exps.iter().sum();",
        "new": "    let sum: f64 = 1.0;",
        "pkg": "trit-learn",
        "filter": "softmax_sums_to_one_and_is_stable"
    },
    {
        "name": "attn: 注意力丢 1/sqrt(d) 缩放",
        "file": "crates/trit-attn/src/block.rs",
        "old": "                        / scale",
        "new": "                        / 1.0",
        "pkg": "trit-attn",
        "filter": "forward_matches_independent_attention_reference"
    },
    {
        "name": "attn: 残差一丢输入 X",
        "file": "crates/trit-attn/src/block.rs",
        "old": "        let mut h = seq.to_vec();",
        "new": "        let mut h = vec![0.0; seq.len()];",
        "pkg": "trit-attn",
        "filter": "input_gradients_match_finite_differences"
    },
    {
        "name": "float: 平局取更小指数（背离约定）",
        "file": "crates/trit-float/src/lib.rs",
        "old": "                            RoundMode::HalfAway => dropped > best_dropped,",
        "new": "                            RoundMode::HalfAway => dropped < best_dropped,",
        "pkg": "trit-float",
        "filter": "precision_bounds_follow_balanced_digits"
    },
    {
        "name": "float: 加法恒丢弃小项",
        "file": "crates/trit-float/src/lib.rs",
        "old": "        if shift > 76 {",
        "new": "        if shift > 0 {",
        "pkg": "trit-float",
        "filter": "arithmetic_is_exact_when_digits_fit"
    },
    {
        "name": "float: 加法对齐溢出改回饱和",
        "file": "crates/trit-float/src/lib.rs",
        "old": "        let Some(boosted) = high.checked_mul(3i128.pow(shift as u32)) else {\n            return Self::new(high, high_exp, precision);\n        };",
        "new": "        let boosted = high.saturating_mul(3i128.pow(shift as u32));",
        "pkg": "trit-float",
        "filter": "adding_zero_returns_the_operand_unchanged"
    },
    {
        "name": "float: digit_count 上界差一",
        "file": "crates/trit-float/src/lib.rs",
        "old": "        if magnitude <= limit {",
        "new": "        if magnitude < limit {",
        "pkg": "trit-float",
        "filter": "rounding_is_nearest_over_legal_mantissas"
    },
    {
        "name": "float: 允许多一位有效位",
        "file": "crates/trit-float/src/lib.rs",
        "old": "    if digits > precision {",
        "new": "    if digits > precision + 1 {",
        "pkg": "trit-float",
        "filter": "precision_bounds_follow_balanced_digits"
    },
    {
        "name": "float: 相等把精度算进去",
        "file": "crates/trit-float/src/lib.rs",
        "old": "        self.mantissa == other.mantissa && self.exponent == other.exponent",
        "new": "        self.mantissa == other.mantissa\n            && self.exponent == other.exponent\n            && self.precision == other.precision",
        "pkg": "trit-float",
        "filter": "equality_and_order_are_numeric_not_representational"
    },
    {
        "name": "float: 对齐溢出判成更小",
        "file": "crates/trit-float/src/lib.rs",
        "old": "        None if high > 0 => std::cmp::Ordering::Greater,",
        "new": "        None if high > 0 => std::cmp::Ordering::Less,",
        "pkg": "trit-float",
        "filter": "equality_and_order_are_numeric_not_representational"
    },
    {
        "name": "float: 乘法指数相减",
        "file": "crates/trit-float/src/lib.rs",
        "old": "            self.exponent.saturating_add(other.exponent),",
        "new": "            self.exponent.max(other.exponent),",
        "pkg": "trit-float",
        "filter": "arithmetic_is_exact_when_digits_fit"
    },
    {
        "name": "dot: 二值公式丢掉系数 2",
        "file": "crates/trit-dot/src/lib.rs",
        "old": '        return len as i64 - 2 * mismatches as i64;',
        "new": '        return len as i64 - mismatches as i64;',
        "pkg": "trit-dot",
        "filter": "bipolar_dot_matches_scalar"
    },
    {
        "name": "dot: 三值点积少减一项",
        "file": "crates/trit-dot/src/lib.rs",
        "old": '            sums[lane] += i64::from(matches) - i64::from(mismatches);',
        "new": '            sums[lane] += i64::from(matches);',
        "pkg": "trit-dot",
        "filter": "ternary_dot_matches_scalar_across_block_boundaries"
    },
    {
        "name": "dot: 稀疏路径漏掉负负项",
        "file": "crates/trit-dot/src/lib.rs",
        "old": "            let nn = (self.neg[block] & other.neg[block]).count_ones();\n            total += i64::from(pp) - i64::from(pn) - i64::from(np) + i64::from(nn);\n        }\n        Ok(total)\n    }\n\n    fn check_length(",
        "new": "            let nn = (self.neg[block] & other.neg[block]).count_ones();\n            total += i64::from(pp) - i64::from(pn) - i64::from(np);\n        }\n        Ok(total)\n    }\n\n    fn check_length(",
        "pkg": "trit-dot",
        "filter": "ternary_dot_matches_scalar_across_block_boundaries"
    },
    {
        "name": 'dot: 展开体丢掉负负项',
        "file": 'crates/trit-dot/src/lib.rs',
        "old": '            let matches = ((pa[lane] & pb[lane]) | (na[lane] & nb[lane])).count_ones();',
        "new": '            let matches = (pa[lane] & pb[lane]).count_ones();',
        "pkg": 'trit-dot',
        "filter": 'ternary_dot_matches_scalar_across_block_boundaries'
    },
    {
        "name": 'dot: 展开体的尾巴不算不匹配',
        "file": 'crates/trit-dot/src/lib.rs',
        "old": '    for lane in 0..rest_a_pos.len() {\n        let matches = ((rest_a_pos[lane] & rest_b_pos[lane])\n            | (rest_a_neg[lane] & rest_b_neg[lane]))\n            .count_ones();\n        let mismatches = ((rest_a_pos[lane] & rest_b_neg[lane])\n            | (rest_a_neg[lane] & rest_b_pos[lane]))\n            .count_ones();\n        total += i64::from(matches) - i64::from(mismatches);\n    }\n    total',
        "new": '    for lane in 0..rest_a_pos.len() {\n        let matches = ((rest_a_pos[lane] & rest_b_pos[lane])\n            | (rest_a_neg[lane] & rest_b_neg[lane]))\n            .count_ones();\n        let mismatches = ((rest_a_pos[lane] & rest_b_neg[lane])\n            | (rest_a_neg[lane] & rest_b_pos[lane]))\n            .count_ones();\n        total += i64::from(matches);\n    }\n    total',
        "pkg": 'trit-dot',
        "filter": 'unrolled_matches_dense_on_all_tail_lengths'
    },
    {
        "name": 'sim: 热循环丢掉 bmask',
        "file": 'crates/trit-circuit/src/circuit/sim.rs',
        "old": '                gate.lut[ins[0].index() * gate.stride + (ins[1].index() & gate.bmask)]',
        "new": '                gate.lut[ins[0].index() * gate.stride + ins[1].index()]',
        "pkg": 'trit-circuit',
        "filter": 'lookups_match_gate_semantics_for_every_algebra'
    },
    {
        "name": 'sim: 一元步长抹平成二元',
        "file": 'crates/trit-circuit/src/circuit/sim.rs',
        "old": '                        *slot = gate.kind.eval(&[Ternary::from_index(a)], alg);\n                    }\n                    (1, 0)',
        "new": '                        *slot = gate.kind.eval(&[Ternary::from_index(a)], alg);\n                    }\n                    (3, 0)',
        "pkg": 'trit-circuit',
        "filter": 'lookups_match_gate_semantics_for_every_algebra'
    },
    {
        "name": 'sim: 二元 bmask 抹掉第二操作数',
        "file": 'crates/trit-circuit/src/circuit/sim.rs',
        "old": '                    }\n                    (3, 3)\n                }',
        "new": '                    }\n                    (3, 0)\n                }',
        "pkg": 'trit-circuit',
        "filter": 'lookups_match_gate_semantics_for_every_algebra'
    },
    {
        "name": 'sim: 常量槽位不写常量值',
        "file": 'crates/trit-circuit/src/circuit/sim.rs',
        "old": '                    lut[0] = gate.const_value.unwrap_or(Ternary::False);',
        "new": '                    lut[0] = Ternary::False;',
        "pkg": 'trit-circuit',
        "filter": 'lookups_match_gate_semantics_for_every_algebra'
    },
    {
        "name": 'sim: LUT 填充把二元下标按 2 进位',
        "file": 'crates/trit-circuit/src/circuit/sim.rs',
        "old": '                                Ternary::from_index(index / 3),',
        "new": '                                Ternary::from_index(index / 2),',
        "pkg": 'trit-circuit',
        "filter": 'lookups_match_gate_semantics_for_every_algebra'
    },
    {
        "name": 'core: 序号还原把 Unknown 当成 True',
        "file": 'crates/trit-core/src/value.rs',
        "old": '            1 => Self::Unknown,',
        "new": '            1 => Self::True,',
        "pkg": 'trit-core',
        "filter": 'index_round_trips_and_lut_stride_is_three_wide'
    },
    {
        "name": "gpu: 位并行少减一项",
        "file": "crates/trit-gpu/src/lib.rs",
        "old": "        total += i64::from(pp) - i64::from(pn) - i64::from(np) + i64::from(nn);",
        "new": "        total += i64::from(pp) - i64::from(pn) + i64::from(nn);",
        "pkg": "trit-gpu",
        "filter": "all_four_sign_products_are_counted"
    },
    {
        "name": "gpu: +1 写进负导轨",
        "file": "crates/trit-gpu/src/lib.rs",
        "old": "            1 => pos[index / TRITS_PER_WORD] |= bit,",
        "new": "            1 => neg[index / TRITS_PER_WORD] |= bit,",
        "pkg": "trit-gpu",
        "filter": "positive_vectors_do_not_land_on_the_negative_rail"
    },
    {
        "name": "gpu: 标量参考改成加法",
        "file": "crates/trit-gpu/src/lib.rs",
        "old": "        .map(|(x, y)| i64::from(*x) * i64::from(*y))",
        "new": "        .map(|(x, y)| i64::from(*x) + i64::from(*y))",
        "pkg": "trit-gpu",
        "filter": "packing_round_trips_across_word_boundaries"
    },
        {
            "name": "gpu: 列打包正负导轨互换",
            "file": "crates/trit-gpu/src/lib.rs",
            "old": "                1 => pos[col * words + row / TRITS_PER_WORD] |= bit,\n                -1 => neg[col * words + row / TRITS_PER_WORD] |= bit,",
            "new": "                1 => neg[col * words + row / TRITS_PER_WORD] |= bit,\n                -1 => pos[col * words + row / TRITS_PER_WORD] |= bit,",
            "pkg": "trit-gpu",
            "filter": "column_packing_and_matmul_match_scalar_reference"
    },
        {
            "name": "gpu: matmul 行基址按 n 步进",
            "file": "crates/trit-gpu/src/lib.rs",
            "old": "        let a_base = row * words;",
            "new": "        let a_base = row * n;",
            "pkg": "trit-gpu",
            "filter": "column_packing_and_matmul_match_scalar_reference"
    },
    {
        "name": "codec: 量化 scale 用最小值",
        "file": "crates/trit-codec/src/quantize.rs",
        "old": "    let scale = values.iter().map(|w| w.abs()).fold(0.0f32, f32::max);",
        "new": "    let scale = values.iter().map(|w| w.abs()).fold(0.0f32, f32::min);",
        "pkg": "trit-codec",
        "filter": "symmetric_distribution_maps_to_three_buckets"
    },
    {
        "name": "codec: 量化阈值边界改含等号",
        "file": "crates/trit-codec/src/quantize.rs",
        "old": "                if *w > threshold {",
        "new": "                if *w >= threshold {",
        "pkg": "trit-codec",
        "filter": "boundary_value_equal_to_threshold_is_unknown"
    },
    {
        "name": "codec: 稀疏下标允许重复",
        "file": "crates/trit-codec/src/sparse.rs",
        "old": "            && index <= p",
        "new": "            && index < p",
        "pkg": "trit-codec",
        "filter": "duplicate_index_is_rejected"
    },
    {
        "name": "codec: 交织行列读反",
        "file": "crates/trit-codec/src/interleave.rs",
        "old": "            let idx = r * cols + c;\n            if idx < n {",
        "new": "            let idx = c * cols + r;\n            if idx < n {",
        "pkg": "trit-codec",
        "filter": "hand_known_example"
    },
    {
        "name": "codec: verify 对截断码字放行",
        "file": "crates/trit-codec/src/checksum.rs",
        "old": "    if codeword.len() < CHECKSUM_LEN {\n        return false;\n    }",
        "new": "    if codeword.len() < CHECKSUM_LEN {\n        return true;\n    }",
        "pkg": "trit-codec",
        "filter": "truncated_codeword_is_rejected"
    },
    {
        "name": "core: 取反把 X 变成 0",
        "file": "crates/trit-core/src/value.rs",
        "old": "            Self::Unknown => Self::Unknown,\n        }\n    }\n}\n\n/// **Kleene** 合取",
        "new": "            Self::Unknown => Self::False,\n        }\n    }\n}\n\n/// **Kleene** 合取",
        "pkg": "trit-core",
        "filter": "operators_use_kleene_and_classic_xor"
    },
    {
        "name": "codec: 稀疏批量拷贝不先填 X",
        "file": "crates/trit-codec/src/plan.rs",
        "old": "                out.fill(Ternary::Unknown);",
        "new": "                out.fill(Ternary::False);",
        "pkg": "trit-codec",
        "filter": "bulk_copy_matches_decode"
    },
    {
        "name": "codec: sparse 视图放行越界下标",
        "file": "crates/trit-codec/src/plan.rs",
        "old": "                    if index as usize >= len {\n                        return Err(CodecError::SparseIndexOutOfRange { index, len });\n                    }",
        "new": "                    if index as usize > len {\n                        return Err(CodecError::SparseIndexOutOfRange { index, len });\n                    }",
        "pkg": "trit-codec",
        "filter": "sparse_view_rejects_out_of_range_unsorted_and_dirty_padding"
    },
    {
        "name": "codec: sparse 视图不查符号区填充位",
        "file": "crates/trit-codec/src/plan.rs",
        "old": "                if used_bits != 0 && bytes[needed - 1] >> used_bits != 0 {",
        "new": "                if false && used_bits != 0 && bytes[needed - 1] >> used_bits != 0 {",
        "pkg": "trit-codec",
        "filter": "sparse_view_rejects_out_of_range_unsorted_and_dirty_padding"
    },
    {
        "name": "net: forward 不校验手搓模型",
        "file": "crates/trit-net/src/infer.rs",
        "old": "pub fn forward(model: &Model, inputs: &[Ternary]) -> Result<Vec<Sign>, ModelError> {\n    model.validate()?;",
        "new": "pub fn forward(model: &Model, inputs: &[Ternary]) -> Result<Vec<Sign>, ModelError> {",
        "pkg": "trit-net",
        "filter": "hand_built_invalid_model_is_rejected_instead_of_panicking"
    },
    {
        "name": "net-train: 只校验第一条样本",
        "file": "crates/trit-net/src/train.rs",
        "old": "    for (index, sample) in inputs.iter().enumerate() {",
        "new": "    for (index, sample) in inputs.iter().take(1).enumerate() {",
        "pkg": "trit-net",
        "filter": "rejects_inconsistent_model_and_late_short_samples"
    },
    {
        "name": "net-train: 不校验模型就开训",
        "file": "crates/trit-net/src/train.rs",
        "old": "    model.validate()?;\n    for (index, sample) in inputs.iter().enumerate() {",
        "new": "    for (index, sample) in inputs.iter().enumerate() {",
        "pkg": "trit-net",
        "filter": "rejects_inconsistent_model_and_late_short_samples"
    },
    {
        "name": "sdf: XOR2 映射成三值强加门",
        "file": "crates/trit-circuit/src/sdf.rs",
        "old": "        (\"XOR\", GateKind::XorClassic),",
        "new": "        (\"XOR\", GateKind::Xor),",
        "pkg": "trit-circuit",
        "filter": "xor_celltypes_map_to_the_classic_gate"
    },
    {
        "name": "circuit: 预算耗尽仍宣称最优覆盖",
        "file": "crates/trit-circuit/src/coverage.rs",
        "old": "        if *budget == 0 && position + 1 < candidates.len() {\n            *exhausted = true;",
        "new": "        if *budget == 0 && position + 1 < candidates.len() {",
        "pkg": "trit-circuit",
        "filter": "budget_exhaustion_with_unexplored_siblings_is_not_optimal"
    },
    {
        "name": "schedule: 索引把锁目录当任务目录",
        "file": "crates/trit-schedule/src/checkpoint/file.rs",
        "old": "                let Ok(task_id) = hex_decode(name) else {\n                    continue;\n                };",
        "new": "                let task_id = hex_decode(name)?;",
        "pkg": "trit-schedule",
        "filter": "index_ignores_the_lock_directory_while_held"
    },
    {
        "name": "schedule: 死进程的陈旧锁不回收",
        "file": "crates/trit-schedule/src/checkpoint/file.rs",
        "old": "                owner.host == local_owner().host && matches!(process_alive(owner.pid), Some(false))",
        "new": "                owner.host == local_owner().host && matches!(process_alive(owner.pid), Some(true))",
        "pkg": "trit-schedule",
        "filter": "stale_lock_from_a_dead_process_is_recovered"
    },
    {
        "name": "schedule: 无主锁一律当残骸",
        "file": "crates/trit-schedule/src/checkpoint/file.rs",
        "old": "            None => dir_age(&path).is_some_and(|age| age >= OWNERLESS_STALE_AFTER),",
        "new": "            None => true,",
        "pkg": "trit-schedule",
        "filter": "fresh_ownerless_lock_is_not_stolen"
    },
    {
        "name": "aiger: aig 二进制头被当文本接受",
        "file": "crates/trit-circuit/src/aiger.rs",
        "old": "    if parts[0] == \"aig\" {\n        return Err(AigerError::BinaryFormat);\n    }",
        "new": "    if false {\n        return Err(AigerError::BinaryFormat);\n    }",
        "pkg": "trit-circuit",
        "filter": "ascii_header_is_aag_and_binary_aig_is_rejected"
    },
    {
        "name": "verilog: && 静默按逐位与",
        "file": "crates/trit-circuit/src/verilog/expr.rs",
        "old": "                if self.chars.get(self.pos + 1).copied() == Some('&') {",
        "new": "                if false {",
        "pkg": "trit-circuit",
        "filter": "logical_and_or_are_rejected_instead_of_bitwise"
    },
    {
        "name": "verilog: || 静默按逐位或",
        "file": "crates/trit-circuit/src/verilog/expr.rs",
        "old": "                if self.chars.get(self.pos + 1).copied() == Some('|') {",
        "new": "                if false {",
        "pkg": "trit-circuit",
        "filter": "logical_and_or_are_rejected_instead_of_bitwise"
    },
    {
        "name": "netlist: 文本字节预算失效",
        "file": "crates/trit-circuit/src/netlist.rs",
        "old": "    if text.len() > limits.bytes {",
        "new": "    if false {",
        "pkg": "trit-circuit",
        "filter": "budgets_reject_oversized_input"
    },
    {
        "name": "blif: 文本字节预算失效",
        "file": "crates/trit-circuit/src/blif.rs",
        "old": "    if text.len() > limits.bytes {",
        "new": "    if false {",
        "pkg": "trit-circuit",
        "filter": "budgets_reject_oversized_blif"
    },
    {
        "name": "classifier: 标签梯度符号反",
        "file": "crates/trit-graph/src/classifier.rs",
        "old": "        dlogits[index_of(sample.label)] -= 1.0;",
        "new": "        dlogits[index_of(sample.label)] += 1.0;",
        "pkg": "trit-graph",
        "filter": "gradients_match_finite_differences"
    },
    {
        "name": "classifier: 头梯度漏掉特征",
        "file": "crates/trit-graph/src/classifier.rs",
        "old": "            dhead[0][k] = dlogits[k] * feature;",
        "new": "            dhead[0][k] = dlogits[k];",
        "pkg": "trit-graph",
        "filter": "gradients_match_finite_differences"
    },
    {
        "name": "classifier: 聚合梯度少除入度",
        "file": "crates/trit-graph/src/classifier.rs",
        "old": "            let scale = dfeature / incoming.len() as f32;",
        "new": "            let scale = dfeature;",
        "pkg": "trit-graph",
        "filter": "gradients_match_finite_differences"
    },
    {
        "name": "mpnn: 反对边不取反",
        "file": "crates/trit-graph/src/mpnn.rs",
        "old": "                _ => -1.0,",
        "new": "                _ => 1.0,",
        "pkg": "trit-graph",
        "filter": "identity_table_votes_all_nine_pairs"
    },
    {
        "name": "mpnn: 死区边界含等号",
        "file": "crates/trit-graph/src/mpnn.rs",
        "old": "        if latent > DEAD_ZONE {",
        "new": "        if latent >= DEAD_ZONE {",
        "pkg": "trit-graph",
        "filter": "dead_zone_is_unknown"
    },
    {
        "name": "mpnn: 拟合梯度反向",
        "file": "crates/trit-graph/src/mpnn.rs",
        "old": "            let grad_q = value_of(target) - value_of(current);",
        "new": "            let grad_q = value_of(current) - value_of(target);",
        "pkg": "trit-graph",
        "filter": "fit_recovers_identity_table"
    },
    {
        "name": "mpnn: 消息取错端点状态",
        "file": "crates/trit-graph/src/mpnn.rs",
        "old": "                votes[target].push(self.table.vote(edge, state));",
        "new": "                votes[target].push(self.table.vote(edge, states[target]));",
        "pkg": "trit-graph",
        "filter": "propagates_along_support_edges"
    },
    {
        "name": "kernel: WL 核忽略边符号",
        "file": "crates/trit-graph/src/kernel.rs",
        "old": "        graph.for_each_out(u, |v, edge| neighbors.push((edge_code(edge), colors[v])));",
        "new": "        graph.for_each_out(u, |v, _| neighbors.push((0, colors[v])));",
        "pkg": "trit-graph",
        "filter": "edge_signs_matter"
    },
    {
        "name": "kernel: 路径核忽略边符号",
        "file": "crates/trit-graph/src/kernel.rs",
        "old": "        signature.push(edge_code(edge));",
        "new": "        signature.push(0);",
        "pkg": "trit-graph",
        "filter": "edge_signs_matter"
    },
    {
        "name": "kernel: 归一化分母错误",
        "file": "crates/trit-graph/src/kernel.rs",
        "old": "    k as f64 / ((self_a as f64).sqrt() * (self_b as f64).sqrt())",
        "new": "    k as f64 / ((self_a * self_b) as f64)",
        "pkg": "trit-graph",
        "filter": "isomorphic_graphs_have_kernel_one"
    },
    {
        "name": "walk: 中立边不冻结",
        "file": "crates/trit-graph/src/walk.rs",
        "old": "                    Ternary::Unknown => (0.0, 0.0, mass.total()),",
        "new": "                    Ternary::Unknown => (mass.plus, mass.minus, mass.unknown),",
        "pkg": "trit-graph",
        "filter": "neutral_edge_freezes_to_unknown"
    },
    {
        "name": "walk: 反对边不翻转",
        "file": "crates/trit-graph/src/walk.rs",
        "old": "                    Ternary::False => (mass.minus, mass.plus, mass.unknown),",
        "new": "                    Ternary::False => (mass.plus, mass.minus, mass.unknown),",
        "pkg": "trit-graph",
        "filter": "oppose_edge_flips_state"
    },
    {
        "name": "walk: 忽略阻尼",
        "file": "crates/trit-graph/src/walk.rs",
        "old": "            let share = damping / degree as f64;",
        "new": "            let share = 1.0 / degree as f64;",
        "pkg": "trit-graph",
        "filter": "support_edges_carry_true"
    },
    {
        "name": "walk: 平票判真",
        "file": "crates/trit-graph/src/walk.rs",
        "old": "        } else if self.plus > self.minus {",
        "new": "        } else if self.plus >= self.minus {",
        "pkg": "trit-graph",
        "filter": "tie_is_unknown"
    },
    {
        "name": "hyper: 超边判决忽略成员状态",
        "file": "crates/trit-graph/src/hyper.rs",
        "old": "            (self.nodes.get(node), self.nodes.is_critical(node))",
        "new": "            (Ternary::True, self.nodes.is_critical(node))",
        "pkg": "trit-graph",
        "filter": "verdict_majority_and_veto"
    },
    {
        "name": "decomp: 不校验边覆盖",
        "file": "crates/trit-graph/src/decomp.rs",
        "old": "            let covered = (0..self.bags.len())\n                .any(|bag| self.bag_contains(bag, u) && self.bag_contains(bag, v));",
        "new": "            let covered = true;",
        "pkg": "trit-graph",
        "filter": "missing_edge_coverage_is_reported"
    },
    {
        "name": "decomp: 不校验运行交",
        "file": "crates/trit-graph/src/decomp.rs",
        "old": "            if links + 1 != holders.len() {",
        "new": "            if false {",
        "pkg": "trit-graph",
        "filter": "disconnected_running_intersection_is_reported"
    },
    {
        "name": "graph: CSR 偏移不累加",
        "file": "crates/trit-graph/src/csr.rs",
        "old": "            offsets[u + 1] = offsets[u] + graph.out_degree(u);",
        "new": "            offsets[u + 1] = offsets[u];",
        "pkg": "trit-graph",
        "filter": "conversion_preserves_edges_and_states"
    },
    {
        "name": "graph: CSR 丢掉边状态",
        "file": "crates/trit-graph/src/csr.rs",
        "old": "                states[cursor] = state;",
        "new": "                states[cursor] = Ternary::Unknown;",
        "pkg": "trit-graph",
        "filter": "conversion_preserves_edges_and_states"
    },
    {
        "name": "graph: WL 忽略边符号",
        "file": "crates/trit-graph/src/wl.rs",
        "old": "            view.for_each_out(u, |v, edge| neighbors.push((edge_code(edge), colors[v])));",
        "new": "            view.for_each_out(u, |v, _| neighbors.push((0, colors[v])));",
        "pkg": "trit-graph",
        "filter": "edge_signs_change_colors"
    },
    {
        "name": "graph: WL 一轮就停",
        "file": "crates/trit-graph/src/wl.rs",
        "old": "        let stable = partition_stable(&colors, &next);",
        "new": "        let stable = true;",
        "pkg": "trit-graph",
        "filter": "distinguishes_path_from_star"
    },
    {
        "name": "graph: 拓扑序忽略环",
        "file": "crates/trit-graph/src/view.rs",
        "old": "        (order.len() == self.len()).then_some(order)",
        "new": "        Some(order)",
        "pkg": "trit-graph",
        "filter": "cycle_is_detected"
    },
    {
        "name": "graph: 邻居只看第一个字",
        "file": "crates/trit-graph/src/graph.rs",
        "old": "                bits &= bits - 1;",
        "new": "                bits = 0;",
        "pkg": "trit-graph",
        "filter": "neighbor_iteration_crosses_words"
    },
    {
        "name": "council: 关键否决失效",
        "file": "crates/trit-graph/src/council.rs",
        "old": "                vetoed |= critical;",
        "new": "                vetoed |= false;",
        "pkg": "trit-graph",
        "filter": "critical_false_vetoes_even_with_majority"
    },
    {
        "name": "council: 平票判真",
        "file": "crates/trit-graph/src/council.rs",
        "old": "    } else if plus > minus {",
        "new": "    } else if plus >= minus {",
        "pkg": "trit-graph",
        "filter": "tie_falls_back_to_unknown"
    },
    {
        "name": "float: 锥形精度不衰减",
        "file": "crates/trit-float/src/format.rs",
        "old": "        self.precision.saturating_sub(drop).max(MIN_PRECISION)",
        "new": "        self.precision",
        "pkg": "trit-float",
        "filter": "taper_reduces_precision_away_from_one"
    },
    {
        "name": "float: 格式下溢不归零",
        "file": "crates/trit-float/src/format.rs",
        "old": "            return Some(TritFloat::zero(self.precision));\n        }\n        let precision = self.precision_at(exponent);",
        "new": "            return None;\n        }\n        let precision = self.precision_at(exponent);",
        "pkg": "trit-float",
        "filter": "quantize_clamps_to_format"
    },
    {
        "name": "float: 舍入不 clamp 到合法尾数",
        "file": "crates/trit-float/src/lib.rs",
        "old": "            let rounded = round_quotient(mantissa, divisor).clamp(-limit, limit);",
        "new": "            let rounded = round_quotient(mantissa, divisor);",
        "pkg": "trit-float",
        "filter": "precision_bounds_follow_balanced_digits"
    },
    {
        "name": "net-format: encode 不校验模型",
        "file": "crates/trit-net/src/format.rs",
        "old": "    model\n        .validate()\n        .map_err(|error| FormatError::Model(error.to_string()))?;",
        "new": "    let _ = &model;",
        "pkg": "trit-net",
        "filter": "encode_rejects_invalid_models_instead_of_truncating"
    },
    {
        "name": "net-format: 维数按 u16 静默截断",
        "file": "crates/trit-net/src/format.rs",
        "old": "        let inputs = u16::try_from(layer.inputs).map_err(|_| {",
        "new": "        let inputs = Ok::<u16, FormatError>((layer.inputs % 65536) as u16).map_err(|_| {",
        "pkg": "trit-net",
        "filter": "encode_rejects_invalid_models_instead_of_truncating"
    },
    {
        "name": "net-train: 学习率不校验有限/正",
        "file": "crates/trit-net/src/train.rs",
        "old": "    if !options.learning_rate.is_finite() || options.learning_rate <= 0.0 {",
        "new": "    if false {",
        "pkg": "trit-net",
        "filter": "rejects_non_finite_learning_rate_and_zero_epochs"
    },
    {
        "name": "net-train: 放行 0 轮训练",
        "file": "crates/trit-net/src/train.rs",
        "old": "    if options.epochs == 0 {",
        "new": "    if false {",
        "pkg": "trit-net",
        "filter": "rejects_non_finite_learning_rate_and_zero_epochs"
    },
    {
        "name": "snn: 模拟前不校验网络",
        "file": "crates/trit-machines/src/snn.rs",
        "old": "    network.validate()?;",
        "new": "    let _ = network;",
        "pkg": "trit-machines",
        "filter": "invalid_networks_are_rejected_instead_of_panicking"
    },
    {
        "name": "snn: 不限制 tick 预算",
        "file": "crates/trit-machines/src/snn.rs",
        "old": "    if ticks > MAX_TICKS {",
        "new": "    if false {",
        "pkg": "trit-machines",
        "filter": "invalid_networks_are_rejected_instead_of_panicking"
    },
    {
        "name": "dot: 长度不一致不报错",
        "file": "crates/trit-dot/src/lib.rs",
        "old": "        if self.len != other.len {",
        "new": "        if false {",
        "pkg": "trit-dot",
        "filter": "length_mismatch_is_rejected"
    },
    {
        "name": "dot: 放行重叠的双导轨",
        "file": "crates/trit-dot/src/lib.rs",
        "old": "            .find(|(_, overlap)| *overlap != 0)",
        "new": "            .find(|_| false)",
        "pkg": "trit-dot",
        "filter": "invalid_planes_are_rejected_at_construction"
    },
    {
        "name": "dot: 不查尾部填充位",
        "file": "crates/trit-dot/src/lib.rs",
        "old": "            if dirty_pos != 0 || dirty_neg != 0 {",
        "new": "            if false {",
        "pkg": "trit-dot",
        "filter": "invalid_planes_are_rejected_at_construction"
    },
    {
        "name": "blif: 导出前不校验结构",
        "file": "crates/trit-circuit/src/blif.rs",
        "old": "    crate::circuit::validate(circuit).map_err(|error| BlifError::Structure(error.to_string()))?;",
        "new": "    let _ = circuit;",
        "pkg": "trit-circuit",
        "filter": "export_validates_structure_and_identifiers"
    },
    {
        "name": "blif: 导出不查标识符",
        "file": "crates/trit-circuit/src/blif.rs",
        "old": "        if name.is_empty() || name.chars().any(char::is_whitespace) || name.starts_with('.') {",
        "new": "        if false {",
        "pkg": "trit-circuit",
        "filter": "export_validates_structure_and_identifiers"
    },
    {
        "name": "blif: 解析不做结构校验",
        "file": "crates/trit-circuit/src/blif.rs",
        "old": "    crate::circuit::validate(&circuit).map_err(|error| BlifError::Structure(error.to_string()))?;",
        "new": "    let _ = &circuit;",
        "pkg": "trit-circuit",
        "filter": "parser_rejects_structurally_invalid_blif"
    },
    {
        "name": "netlist: 解析不做结构校验",
        "file": "crates/trit-circuit/src/netlist.rs",
        "old": "    crate::circuit::validate(&circuit)\n        .map_err(|error| NetlistError::Structure(error.to_string()))?;",
        "new": "    let _ = &circuit;",
        "pkg": "trit-circuit",
        "filter": "parser_rejects_structurally_invalid_input"
    },
    {
        "name": "gguf: 接受非 v2/v3 版本",
        "file": "crates/trit-texture/src/gguf.rs",
        "old": '    if !SUPPORTED_VERSIONS.contains(&version) {',
        "new": '    if false {',
        "pkg": "trit-texture",
        "filter": "rejects_bad_magic_version_and_quantized_types"
    },
    {
        "name": "gguf: 张量长度不校验",
        "file": "crates/trit-texture/src/gguf.rs",
        "old": '    if needed > data.len() {',
        "new": '    if false {',
        "pkg": "trit-texture",
        "filter": "rejects_non_finite_and_truncated"
    },
    {
        "name": "gguf: 放行 NaN/Inf",
        "file": "crates/trit-texture/src/gguf.rs",
        "old": '    if !value.is_finite() {',
        "new": '    if false {',
        "pkg": "trit-texture",
        "filter": "rejects_non_finite_and_truncated"
    },
    {
        "name": "gguf: 阈值不做有限性校验",
        "file": "crates/trit-texture/src/gguf.rs",
        "old": '    if !options.zero_threshold.is_finite() || options.zero_threshold < 0.0 {',
        "new": '    if false {',
        "pkg": "trit-texture",
        "filter": "unsupported_nan_threshold_is_rejected"
    },
    {
        "name": "build: 缓存不比对产物哈希",
        "file": "crates/trit-build/src/main.rs",
        "old": '        if hash64(&bytes) != u64::from_str_radix(info.get("hash")?.as_str()?, 16).ok()? {',
        "new": '        if false {',
        "pkg": "trit-build",
        "filter": "cache_miss_when_artifact_was_tampered"
    },
    {
        "name": "build: 校验忽略不一致",
        "file": "crates/trit-build/src/main.rs",
        "old": '    let ok = bad.is_empty();',
        "new": '    let ok = true;',
        "pkg": "trit-build",
        "filter": "check_detects_tampered_artifact"
    },
    {
        "name": "schedule: 二进制抢先于三进制",
        "file": "crates/trit-build/src/schedule.rs",
        "old": '    if primary_pending > 0 && primary_running < slots {',
        "new": '    if false {',
        "pkg": "trit-build",
        "filter": "ternary_wins_while_slots_remain"
    },
    {
        "name": "schedule: 允许超槽位",
        "file": "crates/trit-build/src/schedule.rs",
        "old": '    if slots == 0 || load.running() >= slots {',
        "new": '    if slots == 0 {',
        "pkg": "trit-build",
        "filter": "never_exceeds_slots_or_returns_none_without_work"
    },
    {
        "name": "verilog: 对拍不一致不报错",
        "file": "crates/trit-build/src/verilog.rs",
        "old": '        if actual != *expected {',
        "new": '        if false {',
        "pkg": "trit-build",
        "filter": "cross_check_rejects_wrong_expression"
    },
    {
        "name": "trb: 放行非法字节 243",
        "file": "crates/trit-build/src/trb.rs",
        "old": '        if *byte >= 243 {',
        "new": '        if *byte > 243 {',
        "pkg": "trit-build",
        "filter": "pack_round_trips_every_byte"
    },
    {
        "name": "trb: 解码丢掉语义字节（B3 当 K3）",
        "file": "crates/trit-build/src/trb.rs",
        "old": '        2 => Algebra::Bochvar,',
        "new": '        2 => Algebra::Kleene,',
        "pkg": "trit-build",
        "filter": "decode_then_encode_is_byte_identical_for_any_length"
    },
    {
        "name": "rail: K3 与门的负轨用 AND",
        "file": "crates/trit-build/src/rail.rs",
        "old": '                self.wire(format!("({lp} & {rp})"), format!("({ln} | {rn})"))',
        "new": '                self.wire(format!("({lp} & {rp})"), format!("({ln} & {rn})"))',
        "pkg": "trit-build",
        "filter": "emitted_two_rail_matches_truth_table"
    },
    {
        "name": "build: 缓存键漏掉 --emit-verilog",
        "file": "crates/trit-build/src/main.rs",
        "old": '            emit_verilog,\n            verilog,',
        "new": '            false,\n            verilog,',
        "pkg": "trit-build",
        "filter": "emit_verilog_is_not_served_from_a_plain_cache"
    },
    {
        "name": "egraph: 规则加载不做自证",
        "file": "crates/trit-build/src/egraph.rs",
        "old": '        if eval(&lhs, &empty, alg) != eval(&rhs, &empty, alg) {',
        "new": '        if false {',
        "pkg": "trit-build",
        "filter": "loader_filters_rules_by_algebra"
    },
    {
        "name": "egraph: 重写结果没接进构建",
        "file": "crates/trit-build/src/main.rs",
        "old": '        let (rewritten, rules) = egraph::rewrite(&simplified, algebra, EGRAPH_ROUNDS);',
        "new": '        let (rewritten, rules) = (simplified.clone(), Vec::<&\'static str>::new());',
        "pkg": "trit-build",
        "filter": "rewrite_fires_and_preserves_truth_table"
    },
    {
        "name": "rail: 强加法按布尔异或导出",
        "file": "crates/trit-build/src/rail.rs",
        "old": '                    format!("((~{truth}) & ({lp} | {ln}) & ({rp} | {rn}))"),',
        "new": '                    format!("(({ln} & {rp}) | ({lp} & {rn}))"),',
        "pkg": "trit-build",
        "filter": "emitted_two_rail_matches_truth_table_for_strong_add"
    },
    {
        "name": "bitplan: 与门用错算子",
        "file": "crates/trit-build/src/bitplan.rs",
        "old": '                Expr::And(..) => alg.and(*a, *b),',
        "new": '                Expr::And(..) => alg.or(*a, *b),',
        "pkg": "trit-build",
        "filter": "every_operator_matches_scalar"
    },
    {


        "name": 'unsafe: 高危形态不再判 Risky',
        "file": 'crates/trit-unsafe/src/scan.rs',
        "old": '    if !documented && !guarded {',
        "new": '    if false {',
        "pkg": 'trit-unsafe',
        "filter": 'undocumented_transmute_is_risky'
    },
    {
        "name": 'unsafe: 不认 SAFETY/# Safety 文档',
        "file": 'crates/trit-unsafe/src/scan.rs',
        "old": '            .any(|line| line.contains("SAFETY") || line.contains("Safety"));',
        "new": '            .any(|_line| false);',
        "pkg": 'trit-unsafe',
        "filter": 'documented_unsafe_fn_is_unknown_not_risky'
    },
    {
        "name": 'unsafe: 注释里的 unsafe 也算站点',
        "file": 'crates/trit-unsafe/src/lexer.rs',
        "old": '                if ch == \'/\' && bytes.get(index + 1) == Some(&\'/\') {',
        "new": '                if false {',
        "pkg": 'trit-unsafe',
        "filter": 'strips_line_and_block_comments'
    },
    {
        "name": 'procscan: 不识别 shell 进程',
        "file": 'crates/trit-procscan/src/analyze.rs',
        "old": '                if SHELLS.contains(&lower.as_str()) {',
        "new": '                if false {',
        "pkg": 'trit-procscan',
        "filter": 'flags_shell_network_and_encoded_commands'
    },
    {
        "name": 'procscan: 不统计 schema 体积',
        "file": 'crates/trit-procscan/src/mcp.rs',
        "old": '    let schema_bytes = serde_json::to_string(&tools_value)',
        "new": '    let schema_bytes = 0 * serde_json::to_string(&tools_value)',
        "pkg": 'trit-procscan',
        "filter": 'handshake_reads_tools_and_measures_schema'
    },
    {
        "name": 'procscan: 目录快照看不出改动',
        "file": 'crates/trit-procscan/src/collector.rs',
        "old": '                out.insert(path, (mtime, meta.len()));',
        "new": '                out.insert(path, (mtime, 0u64));',
        "pkg": 'trit-procscan',
        "filter": 'snapshot_detects_new_and_changed_files'

    },
    {
        "name": 'redteam: 非规范编码永远判规范',
        "file": 'crates/trit-redteam/src/blob.rs',
        "old": '    Ok(last == last % 3u32.pow(filled))',
        "new": '    Ok(true)',
        "pkg": 'trit-redteam',
        "filter": 'strict_decoders_accept_only_the_canonical_encoding'
    },
    {
        "name": 'redteam: CRC 漏检不计数',
        "file": 'crates/trit-redteam/src/crc.rs',
        "old": '                if crc3(&broken) == base {',
        "new": '                if false {',
        "pkg": 'trit-redteam',
        "filter": 'minimum_undetected_weight_is_reachable_and_verified'
    },
    {
        "name": 'redteam: 非法输入不算非法',
        "file": 'crates/trit-redteam/src/rail.rs',
        "old": '            illegal |= state == 3;',
        "new": '            illegal |= false;',
        "pkg": 'trit-redteam',
        "filter": 'illegal_inputs_are_read_as_true_by_a_pos_only_consumer'
    },
    {
        "name": 'redteam: 不校验 len 与 nvars',
        "file": 'crates/trit-redteam/src/blob.rs',
        "old": '    if 3usize.checked_pow(nvars as u32) != Some(len) {',
        "new": '    if false {',
        "pkg": 'trit-redteam',
        "filter": 'rejects_nvars_length_mismatch'
    },
    {
        "name": 'redteam: 差分分歧不记账',
        "file": 'crates/trit-redteam/src/fuzz.rs',
        "old": '                if left != right {',
        "new": '                if false {',
        "pkg": 'trit-redteam',
        "filter": 'comparator_reports_both_kinds_of_disagreement'
    },
    {
        "name": 'redteam: 不识别 shell 包装',
        "file": 'crates/trit-redteam/src/chain.rs',
        "old": '        if is_shell(&cmd[0]) {',
        "new": '        if false {',
        "pkg": 'trit-redteam',
        "filter": 'flags_shell_wrappers_network_and_absolute_paths'
    },
    {
        "name": 'redteam: 侧信道泄漏恒为 0',
        "file": 'crates/trit-redteam/src/sidechannel.rs',
        "old": '        leakage_bits: (h_output - h_conditional).max(0.0),',
        "new": '        leakage_bits: 0.0,',
        "pkg": 'trit-redteam',
        "filter": 'weight_directly_reveals_unknowns'
    },
    {
        "name": 'redteam: 不报执行栈可执行',
        "file": 'crates/trit-redteam/src/binary.rs',
        "old": '        if self.exec_stack == Some(true) {',
        "new": '        if false {',
        "pkg": 'trit-redteam',
        "filter": 'elf_hardening_is_read_from_program_headers'
    },
    {
        "name": 'redteam: 不扫密钥形态字符串',
        "file": 'crates/trit-redteam/src/secrets.rs',
        "old": '        if count > 0 && plausible_secret(&text, name, &needle) {',
        "new": '        if false {',
        "pkg": 'trit-redteam',
        "filter": 'detects_secret_shaped_strings_and_attributes_them_to_us'
    },
    {
        "name": 'redteam: 归属不认 Rust std 路径',
        "file": 'crates/trit-redteam/src/attrib.rs',
        "old": '    let lower = text.to_ascii_lowercase();',
        "new": '    let lower = text.to_ascii_lowercase();\n    if !lower.is_empty() {\n        return false;\n    }',
        "pkg": 'trit-redteam',
        "filter": 'attributes_rust_std_paths_to_the_toolchain'
    },
    {
        "name": 'redteam: CFG 归属固定按 MSVC 算',
        "file": 'crates/trit-redteam/src/binary.rs',
        "old": '            out.push(if msvc {',
        "new": '            out.push(if true {',
        "pkg": 'trit-redteam',
        "filter": 'cfg_attribution_depends_on_the_pe_flavor'
    },
    {
        "name": 'redteam: remap 后仍判成用户名泄漏',
        "file": 'crates/trit-redteam/src/binary.rs',
        "old": '    let user_prefix = |text: &str| text.contains("C:\\\\Users\\\\") || text.contains("/home/");',
        "new": '    let user_prefix = |_text: &str| true;',
        "pkg": 'trit-redteam',
        "filter": 'remapped_toolchain_paths_are_not_findings'
    },
    {
        "name": 'trb: 生产解码器放行非规范编码',
        "file": 'crates/trit-build/src/trb.rs',
        "old": '    if u32::from(*blob.last().expect("非空")) >= 3u32.pow(used_in_last as u32) {',
        "new": '    if false {',
        "pkg": 'trit-build',
        "filter": 'rejects_non_canonical_padding'
    },
    {
        "name": 'runtime: C 解码器放行非规范编码',
        "file": 'crates/trit-build/runtime/trit_runtime.c',
        "old": '    if ((uint32_t)blob[len - 1] >= limit) {',
        "new": '    if (0) {',
        "pkg": 'trit-build',
        "filter": 'both_decoders_reject_non_canonical_padding'
    },
    {
        "name": 'redteam: 解析器放行非规范编码',
        "file": 'crates/trit-redteam/src/blob.rs',
        "old": '    if !canonical_tail(bytes, payload, payload_bytes) {',
        "new": '    if false {',
        "pkg": 'trit-redteam',
        "filter": 'strict_decoders_accept_only_the_canonical_encoding'

    },
    {
        "name": "machines: 搜寻取最低分块而不是最高分",
        "file": "crates/trit-machines/src/graded.rs",
        "old": "    scored.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));",
        "new": "    scored.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));",
        "pkg": "trit-machines",
        "filter": "search_picks_the_block_that_drives_the_rest"
    },
    {
        "name": "machines: 收敛计数把相同判成不同",
        "file": "crates/trit-machines/src/graded.rs",
        "old": "        } else if outcome == last {",
        "new": "        } else if outcome != last {",
        "pkg": "trit-machines",
        "filter": "two_groups_converge_to_positive"
    },
    {
        "name": "machines: 强化把同号写成抑制",
        "file": "crates/trit-machines/src/graded.rs",
        "old": "                let sign = polarity[source].signum() * polarity[listener].signum();",
        "new": "                let sign = -polarity[source].signum() * polarity[listener].signum();",
        "pkg": "trit-machines",
        "filter": "reinforcement_only_fills_empty_links"
    },
    {
        "name": "lsp: 律 lint 不比较否定差异",
        "file": "crates/trit-lsp/src/semantics.rs",
        "old": "        if left.name != right.name || left.negated == right.negated {",
        "new": "        if true {",
        "pkg": "trit-lsp",
        "filter": "contradiction_is_flagged_in_all_spellings"
    },
    {
        "name": "lsp: 语义分歧对拍恒不触发",
        "file": "crates/trit-lsp/src/semantics.rs",
        "old": "        if kleene != lukasiewicz {",
        "new": "        if false {",
        "pkg": "trit-lsp",
        "filter": "lukasiewicz_implies_self_diverges"
    },
    {
        "name": "lsp: 重命名不校验新名",
        "file": "crates/trit-lsp/src/nav.rs",
        "old": "    if !is_identifier(new_name) || analysis::constant(new_name).is_some() {",
        "new": "    if false {",
        "pkg": "trit-lsp",
        "filter": "rename_rejects_invalid_and_constant_names"
    },
    {
        "name": "unsafe: extern 声明块不是站点",
        "file": "crates/trit-unsafe/src/scan.rs",
        "old": '    } else if trimmed.starts_with("extern") {',
        "new": '    } else if false {',
        "pkg": "trit-unsafe",
        "filter": "extern_block_is_a_site_and_stays_unknown"
    },
    {
        "name": "procscan: 后代追踪漏掉间接子树",
        "file": "crates/trit-procscan/src/trace.rs",
        "old": "            worklist.extend(children.get(&current).cloned().unwrap_or_default());",
        "new": "            worklist.clear();",
        "pkg": "trit-procscan",
        "filter": "descendants_follow_multi_level_tree"
    },
    {
        "name": "procscan: 画像末次时间不更新",
        "file": "crates/trit-procscan/src/trace.rs",
        "old": "            entry.last_seen_ms = entry.last_seen_ms.max(*ts_ms);",
        "new": "            entry.last_seen_ms = entry.last_seen_ms.min(*ts_ms);",
        "pkg": "trit-procscan",
        "filter": "repeated_samples_merge_into_one_step"
    },
    {
        "name": "net: 私网误判成公网外联",
        "file": "crates/trit-procscan/src/net.rs",
        "old": "        [10, ..] => AddrClass::Private,",
        "new": "        [10, ..] => AddrClass::Public,",
        "pkg": "trit-procscan",
        "filter": "public_egress_is_flagged_private_is_not"
    },
    {
        "name": "net: 监听全部网卡不报",
        "file": "crates/trit-procscan/src/net.rs",
        "old": '    matches!(host, "0.0.0.0" | "::" | "*")',
        "new": '    false',
        "pkg": "trit-procscan",
        "filter": "listen_all_interfaces_is_flagged_loopback_is_not"
    },
    {
        "name": "net: 外联扇出阈值失效",
        "file": "crates/trit-procscan/src/net.rs",
        "old": "        if hosts_reachable.len() >= SCAN_FANOUT_HOSTS {",
        "new": "        if false {",
        "pkg": "trit-procscan",
        "filter": "egress_fanout_needs_eight_distinct_hosts"
    },
    {
        "name": "redteam: 路径段数过滤失效（needle 常量报成泄漏）",
        "file": "crates/trit-redteam/src/binary.rs",
        "old": "                if path_segments(&text) >= 3 && !samples.contains(&text) {",
        "new": "                if path_segments(&text) >= 0 && !samples.contains(&text) {",
        "pkg": "trit-redteam",
        "filter": "needle_constants_are_not_reported_as_leaks"

    },
    {
        "name": 'trit-re: JSON 引号不转义',
        "file": 'crates/trit-re/src/json.rs',
        "old": '            \'"\' => out.push_str("\\\\\\""),',
        "new": '            \'"\' => out.push(\'"\'),',
        "pkg": "trit-re",
        "filter": 'escapes_controls_and_quotes',
    },
    {
        "name": 'trit-re: 熵符号写反',
        "file": 'crates/trit-re/src/entropy.rs',
        "old": '            entropy -= p * p.log2();',
        "new": '            entropy += p * p.log2();',
        "pkg": "trit-re",
        "filter": 'uniform_full_byte_range_is_eight_bits',
    },
    {
        "name": 'trit-re: 覆盖率阈值由 >= 变 >',
        "file": 'crates/trit-re/src/functions/mod.rs',
        "old": '    exec_bytes > 0 && covered.saturating_mul(10) >= exec_bytes.saturating_mul(9)',
        "new": '    exec_bytes > 0 && covered.saturating_mul(10) > exec_bytes.saturating_mul(9)',
        "pkg": "trit-re",
        "filter": 'exact_coverage_needs_ninety_percent',
    },
    {
        "name": 'trit-re: 单调用点不判 True',
        "file": 'crates/trit-re/src/xref/mod.rs',
        "old": '    row.status = if !row.sites.is_empty() {',
        "new": '    row.status = if row.sites.len() > 1 {',
        "pkg": "trit-re",
        "filter": 'finds_call_through_iat',
    },
    {
        "name": 'trit-re: ARM64 地址窗口缩到 2',
        "file": 'crates/trit-re/src/xref/aarch64.rs',
        "old": 'const MAX_ADDR_STEPS: usize = 12;',
        "new": 'const MAX_ADDR_STEPS: usize = 2;',
        "pkg": "trit-re",
        "filter": 'interleaved_instructions_do_not_hide_the_reference',
    },
    {
        "name": 'trit-re: 取地址不再降级',
        "file": 'crates/trit-re/src/xref/mod.rs',
        "old": '        if row.status == Ternary::False && taken.contains(&row.slot) {',
        "new": '        if row.status == Ternary::True && taken.contains(&row.slot) {',
        "pkg": "trit-re",
        "filter": 'taken_slot_downgrades_false_to_unknown',
    },
    {
        "name": 'trit-re: 已定义符号当导入',
        "file": 'crates/trit-re/src/xref/mod.rs',
        "old": '        if symbol_info.defined || !matches!(symbol_info.kind, 0 | 2 | 10) {',
        "new": '        if false {',
        "pkg": "trit-re",
        "filter": 'defined_symbols_with_jump_slots_are_not_imports',
    },
    {
        "name": 'trit-re: mov 搬运链断开',
        "file": 'crates/trit-re/src/xref/aarch64.rs',
        "old": '            if let Some((dst, src)) = mov_alias(insn)\n                && src == register\n            {',
        "new": '            if let Some((dst, src)) = mov_alias(insn)\n                && src == register && false\n            {',
        "pkg": "trit-re",
        "filter": 'mov_alias_carries_the_address',
    },
    {
        "name": 'trit-re: 未归属 call 记成 jmp',
        "file": 'crates/trit-re/src/functions/mod.rs',
        "old": '                        crate::xref::SiteKind::Call => unattributed_calls += 1,',
        "new": '                        crate::xref::SiteKind::Call => unattributed_jumps += 1,',
        "pkg": "trit-re",
        "filter": 'unattributed_sites_are_split_by_kind',
    },
    {
        "name": 'trit-re: eh_frame pcrel 基准偏移 4',
        "file": 'crates/trit-re/src/elf/eh.rs',
        "old": '            field_addr.wrapping_add(relative as i64 as u64)',
        "new": '            field_addr.wrapping_add(relative as i64 as u64).wrapping_add(4)',
        "pkg": "trit-re",
        "filter": 'eh_frame_fde_gives_exact_range',
    },
    {
        "name": 'trit-re: 不用 eh_frame 边界',
        "file": 'crates/trit-re/src/functions/mod.rs',
        "old": '        (false, true) => Some((FuncSource::EhFrame, eh_frame, None)),',
        "new": '        (false, true) => None,',
        "pkg": "trit-re",
        "filter": 'eh_frame_boundaries_enable_attribution',
    },
    {
        "name": 'trit-re: TLS 只读一个回调',
        "file": 'crates/trit-re/src/pe.rs',
        "old": '    for _ in 0..64 {',
        "new": '    for _ in 0..1 {',
        "pkg": "trit-re",
        "filter": 'parses_clr_and_tls_facts',
    },
    {
        "name": 'trit-re: CLR 入口 token 读成 flags',
        "file": 'crates/trit-re/src/pe.rs',
        "old": '    let entry_token = u32_at(bytes, base + 20).unwrap_or(0);',
        "new": '    let entry_token = u32_at(bytes, base + 16).unwrap_or(0);',
        "pkg": "trit-re",
        "filter": 'parses_clr_and_tls_facts',
    },
    {
        "name": 'trit-re: debug_frame 地址宽度写死 4',
        "file": 'crates/trit-re/src/elf/debug_frame.rs',
        "old": '    let (begin, _) = read_encoded(\n        data,\n        &mut cursor,\n        info.encoding,\n        info.address_size,\n        field_addr,\n    )?;',
        "new": '    let (begin, _) = read_encoded(\n        data,\n        &mut cursor,\n        info.encoding,\n        4,\n        field_addr,\n    )?;',
        "pkg": "trit-re",
        "filter": 'address_size_eight_reads_full_width_addresses',
    },
    {
        "name": 'trit-re: 不用 debug_frame 边界',
        "file": 'crates/trit-re/src/functions/mod.rs',
        "old": '        (true, false) => Some((FuncSource::DebugFrame, debug_frame, None)),',
        "new": '        (true, false) => None,',
        "pkg": "trit-re",
        "filter": 'debug_frame_boundaries_enable_attribution',
    },
    {
        "name": 'trit-re: 不看调用点覆盖，永远选 eh_frame',
        "file": 'crates/trit-re/src/functions/mod.rs',
        "old": '            let pick_debug = debug_sites > eh_sites',
        "new": '            let pick_debug = false && debug_sites > eh_sites',
        "pkg": "trit-re",
        "filter": 'frame_source_prefers_the_one_covering_call_sites',
    },
    {
        "name": 'trit-re: debug_frame 忽略 address_size',
        "file": 'crates/trit-re/src/elf/debug_frame.rs',
        "old": '        address_size = *data.get(cursor)? as usize;',
        "new": '        address_size = 4;',
        "pkg": "trit-re",
        "filter": 'address_size_eight_reads_full_width_addresses',
    },
    {
        "name": 'trit-re: debug_frame 忽略指针编码',
        "file": 'crates/trit-re/src/elf/debug_frame.rs',
        "old": '    let (begin, _) = read_encoded(\n        data,\n        &mut cursor,\n        info.encoding,\n        info.address_size,\n        field_addr,\n    )?;',
        "new": '    let (begin, _) = read_encoded(\n        data,\n        &mut cursor,\n        0,\n        info.address_size,\n        field_addr,\n    )?;',
        "pkg": "trit-re",
        "filter": 'augmentation_zr_pcrel_pointer',
    },
    {
        "name": 'trit-re: x64 取地址不记录',
        "file": 'crates/trit-re/src/xref/mod.rs',
        "old": '                    taken.insert(target.wrapping_sub(image_base));',
        "new": '                    let _ = target;',
        "pkg": "trit-re",
        "filter": 'x64_address_only_import_is_unknown',
    },

    {
        "name": 'trit-re: ELF32 重定位把 r_info 当偏移',
        "file": 'crates/trit-re/src/xref/mod.rs',
        "old": '        let offset = u32::from_le_bytes(data.get(base..base + 4)?.try_into().ok()?);',
        "new": '        let offset = u32::from_le_bytes(data.get(base + 4..base + 8)?.try_into().ok()?);',
        "pkg": "trit-re",
        "filter": 'elf32_pic_stub_is_resolved_through_get_pc_thunk',
    },
    {
        "name": 'trit-re: GOT 基址忽略 add 立即数',
        "file": 'crates/trit-re/src/xref/x86.rs',
        "old": '                    .insert(return_address.wrapping_add(imm as i64 as u64));',
        "new": '                    .insert(return_address);',
        "pkg": "trit-re",
        "filter": 'elf32_pic_stub_is_resolved_through_get_pc_thunk',
    },
    {
        "name": 'trit-re: ELF32 PLT 跳板算成调用点',
        "file": 'crates/trit-re/src/xref/x86.rs',
        "old": '                if kind == SiteKind::Jump && in_plt {',
        "new": '                if false && in_plt {',
        "pkg": "trit-re",
        "filter": 'elf32_pic_stub_is_resolved_through_get_pc_thunk',
    },
    {
        "name": 'trit-re: GLOB_DAT 槽位被忽略',
        "file": 'crates/trit-re/src/xref/mod.rs',
        "old": '            if (kind == slot_type || kind == glob_dat_type) && known.contains(&symbol) {',
        "new": '            if kind == slot_type && known.contains(&symbol) {',
        "pkg": "trit-re",
        "filter": 'elf32_pic_stub_is_resolved_through_get_pc_thunk',
    },
    {
        "name": 'trit-re: 取址不认 PLT 桩地址',
        "file": 'crates/trit-re/src/xref/x86.rs',
        "old": '            } else if let Some(symbol) = stubs.get(&value)',
        "new": '            } else if let Some(symbol) = stubs.get(&value).filter(|_| false)',
        "pkg": "trit-re",
        "filter": 'elf32_pic_stub_is_resolved_through_get_pc_thunk',
    },
    {
        "name": 'trit-re: GOT 基址只认 EBX',
        "file": 'crates/trit-re/src/xref/x86.rs',
        "old": '        .get(&register)?',
        "new": '        .get(&3u8)?',
        "pkg": "trit-re",
        "filter": 'eax_relative_got_access_is_recognised',
    },
    {
        "name": 'trit-re: EAX 的短 add 编码不认',
        "file": 'crates/trit-re/src/xref/x86.rs',
        "old": '    if register == 0 && data.get(at) == Some(&0x05) {',
        "new": '    if false && data.get(at) == Some(&0x05) {',
        "pkg": "trit-re",
        "filter": 'pc_thunk_ax_uses_the_short_add_encoding',
    },
    {
        "name": 'trit-re: C7 立即数偏移少 1',
        "file": 'crates/trit-re/src/xref/x86.rs',
        "old": '            0x45 => index + 3, // [ebp+disp8]',
        "new": '            0x45 => index + 4, // [ebp+disp8]',
        "pkg": "trit-re",
        "filter": 'immediate_reference_reads_each_encoding',
    },
    # trit-immune 依赖坐标层（2026-11-02，随 crates/trit-immune 坐标层同步登记）。
    # 这 8 把在独立仓侧逐条复算过：每把都有断言消息判红（不是编译红），锚点各命中 1 次。
    {
        "name": 'immune: 坐标按家族匹配连边（丢版本/来源/摘要）',
        "file": 'crates/trit-immune/src/coords.rs',
        "old": 'p.coord.key() != u.coord.key()',
        "new": 'p.coord.family() != u.coord.family()',
        "pkg": "trit-immune",
        "filter": 'derive_edges_only_for_exact_key_match_and_never_self',
    },
    {
        "name": 'immune: 坐标缺口一律不上报（fail-open）',
        "file": 'crates/trit-immune/src/coords.rs',
        "old": 'if exact {',
        "new": 'if true {',
        "pkg": "trit-immune",
        "filter": 'coordinate_without_provider_is_reported',
    },
    {
        "name": 'immune: 坐标节点不连回本仓根',
        "file": 'crates/trit-immune/src/coords.rs',
        "old": '        j.cross_link(node, NodeId(0), Trit::Vulnerable, &format!("消费 {title}"))\n            .map_err(io::Error::other)?;\n',
        "new": '        let _ = (node, &title);\n',
        "pkg": "trit-immune",
        "filter": 'coordinate_breach_reaches_the_gate_and_vetoes_merge',
    },
    {
        "name": 'immune: 入账不按标题采纳既有坐标节点（幂等破）',
        "file": 'crates/trit-immune/src/coords.rs',
        "old": '.find(|n| n.title == title)',
        "new": '.find(|n| n.title == "不可能命中的键")',
        "pkg": "trit-immune",
        "filter": 'attach_and_sync_are_idempotent',
    },
    {
        "name": 'immune: 策略忽略传播后的 -1',
        "file": 'crates/trit-immune/src/policy.rs',
        "old": 'if pr.verdict == Trit::Vulnerable {',
        "new": 'if pr.verdict == Trit::Vulnerable && false {',
        "pkg": "trit-immune",
        "filter": 'propagated_minus_one_vetoes_even_with_allow_gate',
    },
    {
        "name": 'immune: 只按包名连边（忽略版本）',
        "file": 'crates/trit-immune/src/coords.rs',
        "old": 'p.coord.key() != u.coord.key()',
        "new": 'p.coord.name != u.coord.name',
        "pkg": "trit-immune",
        "filter": 'fanout_is_scoped_to_the_exact_coordinate',
    },
    {
        "name": 'immune: 放宽「变坏才落账」（相等也落）',
        "file": 'crates/trit-immune/src/coords.rs',
        "old": 'if out >= dv {',
        "new": 'if out > dv {',
        "pkg": "trit-immune",
        "filter": 'fanout_is_scoped_to_the_exact_coordinate',
    },
    {
        "name": 'immune: 跨仓落账不溯源坐标',
        "file": 'crates/trit-immune/src/coords.rs',
        "old": 'format!("{note} ← coord {}:{}", fr, fid.0)',
        "new": 'format!("{note}")',
        "pkg": "trit-immune",
        "filter": 'landing_is_monotonic_and_never_whitewashes',
    },
    # trit-capacity 显存容量判决（2026-11-02 登记；门改为空集判红后补的第一批）。
    {
        "name": 'capacity: headroom 小于 1 不再夹到 1.0',
        "file": 'crates/trit-capacity/src/lib.rs',
        "old": '    if headroom.is_finite() && headroom >= 1.0 {',
        "new": '    if headroom.is_finite() {',
        "pkg": "trit-capacity",
        "filter": 'headroom_below_one_is_clamped',
    },
    {
        "name": 'capacity: free==required 判成 WARN（边界收紧）',
        "file": 'crates/trit-capacity/src/lib.rs',
        "old": '    let (verdict, reason) = if free >= required {',
        "new": '    let (verdict, reason) = if free > required {',
        "pkg": "trit-capacity",
        "filter": 'boundary_free_equals_required_is_ok',
    },
    {
        "name": 'capacity: 只够峰值也算 REJECT（WARN 层消失）',
        "file": 'crates/trit-capacity/src/lib.rs',
        "old": '    } else if free >= need {',
        "new": '    } else if false {',
        "pkg": "trit-capacity",
        "filter": 'warn_when_only_bare_peak_fits',
    },
    {
        "name": 'capacity: 读数不可用时猜成 OK',
        "file": 'crates/trit-capacity/src/lib.rs',
        "old": '        verdict: Verdict::Warn,',
        "new": '        verdict: Verdict::Ok,',
        "pkg": "trit-capacity",
        "filter": 'unknown_is_warn_not_ok',
    },
    {
        "name": 'capacity: --require 下不再拒绝无读数（fail-open）',
        "file": 'crates/trit-capacity/src/lib.rs',
        "old": '            if j.free.is_none() && strict {',
        "new": '            if false {',
        "pkg": "trit-capacity",
        "filter": 'unknown_runs_by_default_but_strict_refuses',
    },
    # trit-belnap FOUR 双轨代数与预算闸门（2026-11-02 登记）。
    {
        "name": 'belnap: 合取假轨该 OR 写成 AND',
        "file": 'crates/trit-belnap/src/lib.rs',
        "old": 'Self::from_rails(t1 && t2, f1 || f2)',
        "new": 'Self::from_rails(t1 && t2, f1 && f2)',
        "pkg": "trit-belnap",
        "filter": 'de_morgan_holds_on_all_sixteen_assignments',
    },
    {
        "name": 'belnap: 析取假轨该 AND 写成 OR',
        "file": 'crates/trit-belnap/src/lib.rs',
        "old": 'Self::from_rails(t1 || t2, f1 && f2)',
        "new": 'Self::from_rails(t1 || t2, f1 || f2)',
        "pkg": "trit-belnap",
        "filter": 'truth_tables_have_the_expected_conflicts',
    },
    {
        "name": 'belnap: 否定不再交换两轨（变成恒等）',
        "file": 'crates/trit-belnap/src/lib.rs',
        "old": 'Self::from_rails(f, t)',
        "new": 'Self::from_rails(t, f)',
        "pkg": "trit-belnap",
        "filter": 'truth_tables_have_the_expected_conflicts',
    },
    {
        "name": 'belnap: 未赋值的变量默认判成 F 而非 N',
        "file": 'crates/trit-belnap/src/lib.rs',
        "old": '.unwrap_or(B4::N)',
        "new": '.unwrap_or(B4::F)',
        "pkg": "trit-belnap",
        "filter": 'eval_defaults_missing_variables_to_neither',
    },
    {
        "name": 'belnap: 真值表生成前不查变量数预算',
        "file": 'crates/trit-belnap/src/lib.rs',
        "old": '    let vars = expr.variables();\n    if vars.len() > limits.max_vars {',
        "new": '    let vars = expr.variables();\n    if false {',
        "pkg": "trit-belnap",
        "filter": 'budgets_are_enforced_before_enumeration',
    },
    # trit-wasm JS 出口面（2026-11-02 登记；6 变量上限/未知四值字符/name=value 报错三条规则**当前无测试面**，故不登记）。
    {
        "name": 'wasm: 未赋值的变量在 JSON 里不再标 X',
        "file": 'crates/trit-wasm/src/lib.rs',
        "old": '.unwrap_or_else(|| "X".to_string());',
        "new": '.unwrap_or_else(|| "?".to_string());',
        "pkg": "trit-wasm",
        "filter": 'assignment_json_marks_variables_without_a_value_as_x',
    },
    {
        "name": 'wasm: 等价判定把结论取反',
        "file": 'crates/trit-wasm/src/lib.rs',
        "old": '"equivalent": verdict.equivalent,',
        "new": '"equivalent": !verdict.equivalent,',
        "pkg": "trit-wasm",
        "filter": 'de_morgan_is_equivalent',
    },
    # trit-train 双导轨位平面算子（2026-11-02 登记；对照标量参考实现与尾部污染）。
    {
        "name": 'train: ∧ 的假导轨该 OR 写成 AND',
        "file": 'crates/trit-train/src/plane.rs',
        "old": '(ap & bp, an | bn),',
        "new": '(ap & bp, an & bn),',
        "pkg": "trit-train",
        "filter": 'exhaustive_binary_ops_match_scalar',
    },
    {
        "name": 'train: ∨ 的假导轨该 AND 写成 OR',
        "file": 'crates/trit-train/src/plane.rs',
        "old": '(ap | bp, an & bn),',
        "new": '(ap | bp, an | bn),',
        "pkg": "trit-train",
        "filter": 'exhaustive_binary_ops_match_scalar',
    },
    {
        "name": 'train: ¬ 不再交换两轨（恒等）',
        "file": 'crates/trit-train/src/plane.rs',
        "old": 'pos: self.neg.clone(),\n            neg: self.pos.clone(),',
        "new": 'pos: self.pos.clone(),\n            neg: self.neg.clone(),',
        "pkg": "trit-train",
        "filter": 'not_matches_scalar_and_is_involutive',
    },
    {
        "name": 'train: 尾部位不再清零',
        "file": 'crates/trit-train/src/plane.rs',
        "old": 'if word + 1 < self.words() || tail == 0 {',
        "new": 'if true {',
        "pkg": "trit-train",
        "filter": 'tail_bits_stay_clean',
    },
    # trit-ops 打包校验与量化闸门（2026-11-02 登记；三条"拒绝"去掉就是 fail-open）。
    {
        "name": 'ops: 不再拒绝两轨重叠（脏输入放行）',
        "file": 'crates/trit-ops/src/pack.rs',
        "old": 'if pos.iter().zip(&neg).any(|(p, n)| p & n != 0) {',
        "new": 'if false {',
        "pkg": "trit-ops",
        "filter": 'rail_rejects_overlapping_planes',
    },
    {
        "name": 'ops: 不再拒绝尾部脏位',
        "file": 'crates/trit-ops/src/pack.rs',
        "old": 'if (pos[last] | neg[last]) & mask != 0 {',
        "new": 'if false {',
        "pkg": "trit-ops",
        "filter": 'rail_rejects_dirty_tail',
    },
    {
        "name": 'ops: 不再拒绝越界的 tryte 码',
        "file": 'crates/trit-ops/src/pack.rs',
        "old": 'if data.iter().any(|&b| (b as usize) >= TRYTE_STATES) {',
        "new": 'if false {',
        "pkg": "trit-ops",
        "filter": 'trytes_reject_out_of_range_code',
    },
    {
        "name": 'ops: 量化尺度不按绝对值取（丢符号位）',
        "file": 'crates/trit-ops/src/quant.rs',
        "old": 'let s = values.iter().fold(0.0f32, |a, v| a.max(v.abs()));',
        "new": 'let s = values.iter().fold(0.0f32, |a, v| a.max(*v));',
        "pkg": "trit-ops",
        "filter": 'abs_max_scale_uses_magnitude_and_degenerate_input_falls_back',
    },
    {
        "name": 'ops: 量化结果不再夹到 [-1,1]',
        "file": 'crates/trit-ops/src/quant.rs',
        "old": 'Trit::from_i8_unchecked(q.clamp(-1.0, 1.0) as i8)',
        "new": 'Trit::from_i8_unchecked(q as i8)',
        "pkg": "trit-ops",
        "filter": 'quantize_respects_sign_and_clamps',
    },
    # 门脚本自身（python 语言面）：runner=python 走脚本自带的 `--self-test`，
    # filter 记的是应当变红的那个自测例名（证据用；命令不看它）。
    # 注意 `LIMIT = 800` 不当条目：自测的夹具按 LIMIT 自己构造，改它属等价变异。
    {
        "name": "budget: 新超预算不再上报（fail-open）",
        "file": "scripts/module_budget.py",
        "old": "        if lines > LIMIT and file not in baseline:",
        "new": "        if False:",
        "pkg": "scripts/module_budget.py",
        "filter": "red: one line over",
        "runner": "python",
    },
    {
        "name": "budget: 完全跳过台账比对（GREW/STALE/ORPHAN 三条一起失效）",
        "file": "scripts/module_budget.py",
        "old": "    for file, recorded in sorted(baseline.items()):",
        "new": "    for file, recorded in []:",
        "pkg": "scripts/module_budget.py",
        "filter": "red: baselined file grew",
        "runner": "python",
    },
    {
        "name": "budget: 台账变胖不再拒绝（--update 变成无条件放行）",
        "file": "scripts/module_budget.py",
        "old": "    if grew:",
        "new": "    if False:",
        "pkg": "scripts/module_budget.py",
        "filter": "update refuses growth",
        "runner": "python",
    },
    {
        "name": "budget: 记账值变胖不判红",
        "file": "scripts/module_budget.py",
        "old": "        if lines > recorded:",
        "new": "        if False:",
        "pkg": "scripts/module_budget.py",
        "filter": "red: baselined file grew",
        "runner": "python",
    },
    # 其余三个带 `--self-test` 的门脚本（python 语言面，逐条实测判红）。
    {
        "name": "hazard: 已登记但缺 SAFETY 不再拦截",
        "file": "scripts/hazard_gate.py",
        "old": "                if rel in allow_files and not in_tests:",
        "new": "                if False:",
        "pkg": "scripts/hazard_gate.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "hazard: 测试/示例豁免失效（正例被误报）",
        "file": "scripts/hazard_gate.py",
        "old": "                if in_tests:",
        "new": "                if False:",
        "pkg": "scripts/hazard_gate.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "hazard: SAFETY 检索窗口收成 0 行",
        "file": "scripts/hazard_gate.py",
        "old": "SAFETY_WINDOW = 8",
        "new": "SAFETY_WINDOW = 0",
        "pkg": "scripts/hazard_gate.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "publish: 不再检查发布元数据（MISSING 全瞎）",
        "file": "scripts/publish_graph.py",
        "old": "        if publishable:",
        "new": "        if False:",
        "pkg": "scripts/publish_graph.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "hygiene: 同文件重叠不再上报",
        "file": "scripts/pr_hygiene.py",
        "old": "            if shared:",
        "new": "            if False:",
        "pkg": "scripts/pr_hygiene.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "hygiene: 同树两份不再判 DUP",
        "file": "scripts/pr_hygiene.py",
        "old": "        if len(members) < 2:",
        "new": "        if len(members) < 3:",
        "pkg": "scripts/pr_hygiene.py",
        "filter": "",
        "runner": "python",
    },
    # 第二批 python 门脚本变异（同样逐条实测为断言红）。
    {
        "name": "publish: 依赖未发布 crate 不再拦截",
        "file": "scripts/publish_graph.py",
        "old": "            if not is_publishable(target):",
        "new": "            if False:",
        "pkg": "scripts/publish_graph.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "publish: dev 依赖不再豁免（正例被误报）",
        "file": "scripts/publish_graph.py",
        "old": "            if dependency.get(\"kind\") == \"dev\":",
        "new": "            if False:",
        "pkg": "scripts/publish_graph.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "hygiene: 点名两文件不再算描述不符（门槛抬到 3）",
        "file": "scripts/pr_hygiene.py",
        "old": "    if len(missing) < 2:",
        "new": "    if len(missing) < 3:",
        "pkg": "scripts/pr_hygiene.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "hazard: SAFETY 注释识别失效（正例全被误报）",
        "file": "scripts/hazard_gate.py",
        "old": "SAFETY_RE = re.compile(r\"#\\s*Safety|SAFETY\\s*[::]|Safety\\s*[::]\")",
        "new": "SAFETY_RE = re.compile(r\"NEVER_MATCHES_THIS_SENTINEL\")",
        "pkg": "scripts/hazard_gate.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "budget: 台账变瘦不再要求收紧（两条 STALE 一起失效）",
        "file": "scripts/module_budget.py",
        "old": "        elif lines <= LIMIT:\n            failures.append(\n                f\"STALE BASELINE: {file} \u5df2\u964d\u5230 {lines}\uff08\u9884\u7b97\u5185\uff09\uff0c\u53f0\u8d26\u4ecd\u5199 {recorded}\uff1b\u8dd1 --update \u79fb\u9664\"\n            )\n        elif lines < recorded:\n            failures.append(\n                f\"STALE BASELINE: {file} \u5df2\u964d\u5230 {lines}\uff0c\u53f0\u8d26\u4ecd\u5199 {recorded}\uff1b\u8dd1 --update \u6536\u7d27\"\n            )\n",
        "new": "        elif False:\n            pass\n        elif False:\n            pass\n",
        "pkg": "scripts/module_budget.py",
        "filter": "",
        "runner": "python",
    },
    # ---- bash 面：cli_smoke.sh 的断言工具（--self-test 钉正反两侧）----
    {
        "name": "smoke: expect 的匹配分支放宽成无条件放行",
        "file": "crates/trit-immune/scripts/cli_smoke.sh",
        "old": "    *\"$1\"*) say \"$3\" ;;",
        "new": "    *) say \"$3\" ;;",
        "pkg": "crates/trit-immune/scripts/cli_smoke.sh",
        "filter": "",
        "runner": "bash",
    },
    {
        "name": "smoke: expect 判红后不再退出（断言空转）",
        "file": "crates/trit-immune/scripts/cli_smoke.sh",
        "old": "实际输出：%s\\n' \"$3\" \"$1\" \"$2\" >&2; exit 1",
        "new": "实际输出：%s\\n' \"$3\" \"$1\" \"$2\" >&2; exit 0",
        "pkg": "crates/trit-immune/scripts/cli_smoke.sh",
        "filter": "",
        "runner": "bash",
    },
    {
        "name": "smoke: expect_absent 只在空输出时判脏",
        "file": "crates/trit-immune/scripts/cli_smoke.sh",
        "old": "    *\"$1\"*) printf 'FAIL %s：不该出现",
        "new": "    \"\") printf 'FAIL %s：不该出现",
        "pkg": "crates/trit-immune/scripts/cli_smoke.sh",
        "filter": "",
        "runner": "bash",
    },
    {
        "name": "smoke: fail 打印了却没让脚本失败",
        "file": "crates/trit-immune/scripts/cli_smoke.sh",
        "old": "fail() { printf 'FAIL %s\\n' \"$*\" >&2; exit 1; }",
        "new": "fail() { printf 'FAIL %s\\n' \"$*\" >&2; exit 0; }",
        "pkg": "crates/trit-immune/scripts/cli_smoke.sh",
        "filter": "",
        "runner": "bash",
    },
    {
        "name": "smoke: run 吞掉命令的非零退出",
        "file": "crates/trit-immune/scripts/cli_smoke.sh",
        "old": "run() { \"$BIN\" \"$@\" -f \"$LEDGER\" 2>&1 || fail \"命令失败：$BIN $*\"; }",
        "new": "run() { \"$BIN\" \"$@\" -f \"$LEDGER\" 2>&1; }",
        "pkg": "crates/trit-immune/scripts/cli_smoke.sh",
        "filter": "",
        "runner": "bash",
    },
    {
        "name": "smoke: expect 比错了变量（拿标签当输出比对）",
        "file": "crates/trit-immune/scripts/cli_smoke.sh",
        "old": "expect() {\n  case \"$2\" in",
        "new": "expect() {\n  case \"$3\" in",
        "pkg": "crates/trit-immune/scripts/cli_smoke.sh",
        "filter": "",
        "runner": "bash",
    },
    {
        "name": "smoke: expect_absent 拿 needle 自己比对（干净输出误判）",
        "file": "crates/trit-immune/scripts/cli_smoke.sh",
        "old": "expect_absent() {\n  case \"$2\" in",
        "new": "expect_absent() {\n  case \"$1\" in",
        "pkg": "crates/trit-immune/scripts/cli_smoke.sh",
        "filter": "",
        "runner": "bash",
    },
    # ---- 门自身：裁决链（--self-test 钉住；这些判定都真被改坏过）----
    {
        "name": "gate: 编译/语法红不再单列（假杀当抓住）",
        "file": "scripts/mutation_check.py",
        "old": "    if \"could not compile\" in diag or any(tok in diag for tok in syntax.split(\"|\")):",
        "new": "    if False:",
        "pkg": "scripts/mutation_check.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gate: 崩溃红当断言（CRASH-KILL 归零）",
        "file": "scripts/mutation_check.py",
        "old": "            return \"crash\", hint",
        "new": "            return \"caught\", hint",
        "pkg": "scripts/mutation_check.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gate: python 裸 traceback 不再算崩溃",
        "file": "scripts/mutation_check.py",
        "old": "        ) or (\n            runner == \"python\"\n            and \"Traceback (most recent call last)\" in out\n            and \"AssertionError\" not in out\n        )",
        "new": "        ) or False",
        "pkg": "scripts/mutation_check.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gate: bash 语法红只认英文签名",
        "file": "scripts/mutation_check.py",
        "old": "    \"bash\": {\"cmd\": \"bash\", \"syntax\": \"syntax error|语法错误\"},",
        "new": "    \"bash\": {\"cmd\": \"bash\", \"syntax\": \"syntax error\"},",
        "pkg": "scripts/mutation_check.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gate: .sh 目标回落到 cargo",
        "file": "scripts/mutation_check.py",
        "old": "    if pkg.endswith(\".sh\"):\n        return \"bash\"",
        "new": "    if pkg.endswith(\".sh\"):\n        return \"cargo\"",
        "pkg": "scripts/mutation_check.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gate: 空集不再判红（绿=没东西可杀）",
        "file": "scripts/mutation_check.py",
        "old": "            print(f\"EXEMPT 过期：{pkg} 已经有自己的 fn/impl 了，豁免不再成立 ⇒ 请补登记变异。\")\n        return 1",
        "new": "            print(f\"EXEMPT 过期：{pkg} 已经有自己的 fn/impl 了，豁免不再成立 ⇒ 请补登记变异。\")\n        return 0",
        "pkg": "scripts/mutation_check.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gate: 不点名失败的测试（抓住说不出是谁）",
        "file": "scripts/mutation_check.py",
        "old": "    tests = sorted({m.group(1) for m in re.finditer(r\"^---- (\\S+) stdout ----\", out, re.M)})",
        "new": "    tests = []",
        "pkg": "scripts/mutation_check.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gate: 忽略条目显式 runner",
        "file": "scripts/mutation_check.py",
        "old": "    explicit = mutant.get(\"runner\")\n    if explicit:\n        return explicit",
        "new": "    explicit = mutant.get(\"runner\")\n    if False:\n        return explicit",
        "pkg": "scripts/mutation_check.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gate: preflight 不按语言分判自测边界",
        "file": "scripts/mutation_check.py",
        "old": "        if runner in (\"python\", \"bash\"):\n            marker = selftest_marker(runner)",
        "new": "        if False:\n            marker = selftest_marker(runner)",
        "pkg": "scripts/mutation_check.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gate: bash 自测面识别失效（豁免永不过期）",
        "file": "scripts/mutation_check.py",
        "old": "        return selftest_marker(\"bash\" if pkg.endswith(\".sh\") else \"python\") in text",
        "new": "        return selftest_marker(\"python\") in text",
        "pkg": "scripts/mutation_check.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gate: 逐目标 0 条不判红（被别的目标带绿）",
        "file": "scripts/mutation_check.py",
        "old": "    if unexplained or stale:\n        scope = ",
        "new": "    if False:\n        scope = ",
        "pkg": "scripts/mutation_check.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gate: 全仓登记为空也放行",
        "file": "scripts/mutation_check.py",
        "old": "            print(\"NO MUTANTS REGISTERED：（全仓）MUTANTS 是空的 ⇒ 判红。\")\n            return 1",
        "new": "            print(\"NO MUTANTS REGISTERED：（全仓）MUTANTS 是空的 ⇒ 判红。\")\n            return 0",
        "pkg": "scripts/mutation_check.py",
        "filter": "",
        "runner": "python",
    },
    # ---- hazard_gate 判定面 ----
    {
        "name": "hazard: 不剥字符串/char 字面量（needle 自指复活）",
        "file": "scripts/hazard_gate.py",
        "old": "        joined = scrub(\"\\n\".join(original_lines) + \"\\n\")",
        "new": "        joined = \"\\n\".join(original_lines) + \"\\n\"",
        "pkg": "scripts/hazard_gate.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "hazard: 内联测试边界失效（测试演练当产品代码）",
        "file": "scripts/hazard_gate.py",
        "old": "        if \"#[cfg(test)]\" in lines[index]:",
        "new": "        if False:",
        "pkg": "scripts/hazard_gate.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "hazard: Relaxed 规则匹配错枚举",
        "file": "scripts/hazard_gate.py",
        "old": "        \"pattern\": r\"Ordering::Relaxed\",",
        "new": "        \"pattern\": r\"Ordering::SeqCst\",",
        "pkg": "scripts/hazard_gate.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "hazard: 忽略登记表（已登记的文件也报红）",
        "file": "scripts/hazard_gate.py",
        "old": "            allow = (allow_overrides or {}).get(rule[\"id\"]) or rule[\"allow\"]",
        "new": "            allow = {}",
        "pkg": "scripts/hazard_gate.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "hazard: 报行号偏一行",
        "file": "scripts/hazard_gate.py",
        "old": "                    \"line\": line_no + 1,",
        "new": "                    \"line\": line_no + 2,",
        "pkg": "scripts/hazard_gate.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "hazard: 登记了但没写不变量也放行",
        "file": "scripts/hazard_gate.py",
        "old": "                        entry[\"hint\"] = (\n                            \"已登记但匹配点（及其所在 unsafe fn 签名）向上没有\"\n                            \" SAFETY/# Safety 不变量\"\n                        )\n                        findings.append(entry)",
        "new": "                        entry[\"hint\"] = (\n                            \"已登记但匹配点（及其所在 unsafe fn 签名）向上没有\"\n                            \" SAFETY/# Safety 不变量\"\n                        )\n                        allow_hits.append(entry)",
        "pkg": "scripts/hazard_gate.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "hazard: SAFETY 检索窗口缩到 0 行",
        "file": "scripts/hazard_gate.py",
        "old": "SAFETY_WINDOW = 8\n\n\ndef scrub",
        "new": "SAFETY_WINDOW = 0\n\n\ndef scrub",
        "pkg": "scripts/hazard_gate.py",
        "filter": "",
        "runner": "python",
    },
    # ---- module_budget 台账面 ----
    {
        "name": "budget: 无视台账（已登记的债务也算新增超限）",
        "file": "scripts/module_budget.py",
        "old": "        if lines > LIMIT and file not in baseline:",
        "new": "        if lines > LIMIT:",
        "pkg": "scripts/module_budget.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "budget: 台账里的文件变胖不判红",
        "file": "scripts/module_budget.py",
        "old": "        if lines > recorded:",
        "new": "        if False:",
        "pkg": "scripts/module_budget.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "budget: 已删除文件留在台账里不判红",
        "file": "scripts/module_budget.py",
        "old": "        if file not in sizes:",
        "new": "        if False:",
        "pkg": "scripts/module_budget.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "budget: --update 不再拒绝增长",
        "file": "scripts/module_budget.py",
        "old": "        if new is not None and new > old",
        "new": "        if False",
        "pkg": "scripts/module_budget.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "budget: --update 把预算内文件也写进台账",
        "file": "scripts/module_budget.py",
        "old": "def save_baseline(path: Path, sizes: dict[str, int]) -> None:\n    over = {f: n for f, n in sizes.items() if n > LIMIT}",
        "new": "def save_baseline(path: Path, sizes: dict[str, int]) -> None:\n    over = {f: n for f, n in sizes.items() if n >= LIMIT}",
        "pkg": "scripts/module_budget.py",
        "filter": "",
        "runner": "python",
    },
    # ---- 快照底座（不依赖 git 的字节级回退）----
    {
        "name": "snapshot: 改动文件检不出来",
        "file": "scripts/snapshot.py",
        "old": "        if sha256_file(root / rel) != wanted[rel][\"sha\"]:",
        "new": "        if False:",
        "pkg": "scripts/snapshot.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "snapshot: 控制台编码崩掉回退（降级打印被去掉）",
        "file": "scripts/snapshot.py",
        "old": "    try:\n        print(text)\n    except UnicodeEncodeError:\n        print(text.encode(encoding, \"replace\").decode(encoding, \"replace\"))",
        "new": "    print(text)",
        "pkg": "scripts/snapshot.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "snapshot: 被删文件不补回",
        "file": "scripts/snapshot.py",
        "old": "    missing = sorted(set(wanted) - current_set)\n    extra = sorted(current_set - set(wanted))",
        "new": "    missing = []\n    extra = sorted(current_set - set(wanted))",
        "pkg": "scripts/snapshot.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "snapshot: 快照后新增的文件看不见",
        "file": "scripts/snapshot.py",
        "old": "    missing = sorted(set(wanted) - current_set)\n    extra = sorted(current_set - set(wanted))",
        "new": "    missing = sorted(set(wanted) - current_set)\n    extra = []",
        "pkg": "scripts/snapshot.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "snapshot: dry-run 也真写文件",
        "file": "scripts/snapshot.py",
        "old": "    if dry_run:\n        print(\"dry-run：未改动任何文件\")",
        "new": "    if False:\n        print(\"dry-run：未改动任何文件\")",
        "pkg": "scripts/snapshot.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "snapshot: --prune 不删新增文件",
        "file": "scripts/snapshot.py",
        "old": "    if prune:\n        for rel in plan[\"extra\"]:",
        "new": "    if False:\n        for rel in plan[\"extra\"]:",
        "pkg": "scripts/snapshot.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "snapshot: verify 永远报一致",
        "file": "scripts/snapshot.py",
        "old": "    if not any(plan.values()):\n        print(f\"verify {manifest['id']}：工作树与快照逐字节一致\")",
        "new": "    if True:\n        print(f\"verify {manifest['id']}：工作树与快照逐字节一致\")",
        "pkg": "scripts/snapshot.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "snapshot: 写回后不校验（坏对象静默落盘）",
        "file": "scripts/snapshot.py",
        "old": "        if sha256_file(target) != manifest[\"files\"][rel][\"sha\"]:",
        "new": "        if False:",
        "pkg": "scripts/snapshot.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "snapshot: 把 target/ 也存进快照",
        "file": "scripts/snapshot.py",
        "old": "EXCLUDE_DIRS = {\".git\", \"target\", \".trit\", \"node_modules\", \"__pycache__\", \".venv\"}",
        "new": "EXCLUDE_DIRS = {\".git\", \".trit\", \"node_modules\", \"__pycache__\", \".venv\"}",
        "pkg": "scripts/snapshot.py",
        "filter": "",
        "runner": "python",
    },
    # ---- 依赖脱离体检（deps.py）----
    {
        "name": "deps: 继承字段识别失效（摘出去才会炸）",
        "file": "scripts/deps.py",
        "old": "        if isinstance(v, dict) and v.get(\"workspace\") is True\n    ]",
        "new": "        if False\n    ]",
        "pkg": "scripts/deps.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "deps: workspace 依赖里的内部 crate 被判成外部",
        "file": "scripts/deps.py",
        "old": "            kind = \"internal\" if \"path\" in base else \"external\"",
        "new": "            kind = \"external\"",
        "pkg": "scripts/deps.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "deps: 直接 path 依赖不算内部",
        "file": "scripts/deps.py",
        "old": "    if isinstance(spec, dict) and \"path\" in spec:",
        "new": "    if False:",
        "pkg": "scripts/deps.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "deps: 越界引用不再单列（../ 指到 crate 外也放行）",
        "file": "scripts/deps.py",
        "old": "                if not inside:",
        "new": "                if False:",
        "pkg": "scripts/deps.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "deps: 不解析相对路径（crate 内的 ../ 被误报越界）",
        "file": "scripts/deps.py",
        "old": "                target = (path.parent / ref).resolve()",
        "new": "                target = path.parent / ref",
        "pkg": "scripts/deps.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "deps: 指向不存在的文件不报",
        "file": "scripts/deps.py",
        "old": "                elif not target.is_file():",
        "new": "                elif False:",
        "pkg": "scripts/deps.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "deps: 不看 dev-dependencies",
        "file": "scripts/deps.py",
        "old": "DEP_SECTIONS = (\"dependencies\", \"dev-dependencies\", \"build-dependencies\")",
        "new": "DEP_SECTIONS = (\"dependencies\", \"build-dependencies\")",
        "pkg": "scripts/deps.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "deps: dev 依赖被当成普通内部依赖",
        "file": "scripts/deps.py",
        "old": "                kind = \"内部 dev\" if section == \"dev-dependencies\" else \"内部依赖\"",
        "new": "                kind = \"内部依赖\"",
        "pkg": "scripts/deps.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "deps: 不可发布的依赖没被标出",
        "file": "scripts/deps.py",
        "old": "        publish = package.get(\"publish\", True)",
        "new": "        publish = True",
        "pkg": "scripts/deps.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "deps: build.rs 不体检",
        "file": "scripts/deps.py",
        "old": "    if (root / crate.rel / \"build.rs\").is_file():",
        "new": "    if False:",
        "pkg": "scripts/deps.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "deps: 内部依赖不进图（graph 会空）",
        "file": "scripts/deps.py",
        "old": "                if section != \"dev-dependencies\":\n                    internal.append(name)",
        "new": "                if section != \"dev-dependencies\":\n                    pass",
        "pkg": "scripts/deps.py",
        "filter": "",
        "runner": "python",
    },
    # ---- deps.py 的 copy 模式脱离 ----
    {
        "name": "deps: 点号形式的 workspace 依赖不展开",
        "file": "scripts/deps.py",
        "old": "            if section_tail in DEP_SECTIONS:",
        "new": "            if False:",
        "pkg": "scripts/deps.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "deps: 内部依赖的 path 不改指新布局",
        "file": "scripts/deps.py",
        "old": "    if \"path\" in merged and name in closure:\n        merged[\"path\"] = f\"../{name}\"",
        "new": "    if False:\n        merged[\"path\"] = f\"../{name}\"",
        "pkg": "scripts/deps.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "deps: 写回时没用重写后的清单",
        "file": "scripts/deps.py",
        "old": "        manifest.write_text(new_text, encoding=\"utf-8\", newline=\"\")",
        "new": "        manifest.write_text(text, encoding=\"utf-8\", newline=\"\")",
        "pkg": "scripts/deps.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "deps: 只带走直接依赖（传递依赖漏拷）",
        "file": "scripts/deps.py",
        "old": "        for dep in plan[\"internal_deps\"]:\n            stack.append(dep)",
        "new": "        for dep in plan[\"internal_deps\"]:\n            pass",
        "pkg": "scripts/deps.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "deps: dev 传递依赖漏拷",
        "file": "scripts/deps.py",
        "old": "            if blocker[\"kind\"] == \"内部 dev\":\n                stack.append(blocker[\"detail\"].split(\"（\", 1)[0])",
        "new": "            if False:\n                stack.append(blocker[\"detail\"].split(\"（\", 1)[0])",
        "pkg": "scripts/deps.py",
        "filter": "",
        "runner": "python",
    },
    # ---- 离线自足（vendor.py）----
    {
        "name": "vendor: 重复 save 会重复插段",
        "file": "scripts/vendor.py",
        "old": "    if START in text and END in text:",
        "new": "    if False:",
        "pkg": "scripts/vendor.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "vendor: 还原后多留一行空行",
        "file": "scripts/vendor.py",
        "old": "    if head.endswith(\"\\n\\n\"):  # 插段时多垫的一行\n        head = head[:-1]",
        "new": "    if False:\n        head = head[:-1]",
        "pkg": "scripts/vendor.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "vendor: 无尾换行的配置被吞行",
        "file": "scripts/vendor.py",
        "old": "    if text and not text.endswith(\"\\n\"):\n        text += \"\\n\"",
        "new": "    if False:\n        text += \"\\n\"",
        "pkg": "scripts/vendor.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "vendor: restore 不优先用备份",
        "file": "scripts/vendor.py",
        "old": "    if backup.exists():",
        "new": "    if False:",
        "pkg": "scripts/vendor.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "vendor: 第二次 save 覆盖真原件",
        "file": "scripts/vendor.py",
        "old": "    if config.exists() and not backup.exists():",
        "new": "    if config.exists():",
        "pkg": "scripts/vendor.py",
        "filter": "",
        "runner": "python",
    },
    # ---- autoflow（失败自动回退）----
    {
        "name": "autoflow: 失败也当成功（不回退）",
        "file": "scripts/autoflow.py",
        "old": "    if returncode == 0:",
        "new": "    if True:",
        "pkg": "scripts/autoflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "autoflow: --keep-on-fail 失效（失败也回退）",
        "file": "scripts/autoflow.py",
        "old": "    if keep_on_fail:",
        "new": "    if False:",
        "pkg": "scripts/autoflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "autoflow: 回退时不删命令新建的文件",
        "file": "scripts/autoflow.py",
        "old": "    try:\n        code = snap.restore(root, manifest, prune=True, dry_run=False)",
        "new": "    try:\n        code = snap.restore(root, manifest, prune=False, dry_run=False)",
        "pkg": "scripts/autoflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "autoflow: 吞掉失败码（返回 0）",
        "file": "scripts/autoflow.py",
        "old": "    return returncode if code == 0 else code",
        "new": "    return 0",
        "pkg": "scripts/autoflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "snapshot: prune 后不清空目录",
        "file": "scripts/snapshot.py",
        "old": "            while directory != root and directory.is_dir() and not any(directory.iterdir()):",
        "new": "            while False:",
        "pkg": "scripts/snapshot.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "autoflow: 命令起不来时误报成功",
        "file": "scripts/autoflow.py",
        "old": "        returncode = 127",
        "new": "        returncode = 0",
        "pkg": "scripts/autoflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: .env.* 漏收（只认 .env）",
        "file": "scripts/gitflow.py",
        "old": "        if path.name in SECRET_FILE_NAMES or path.name == \".env\" or path.name.startswith(\".env.\"):",
        "new": "        if path.name in SECRET_FILE_NAMES or path.name == \".env\":",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "snapshot: 非法快照 id 不拦（../ 可跳出）",
        "file": "scripts/snapshot.py",
        "old": "    if not SNAP_ID_RE.match(snap_id):",
        "new": "    if False:",
        "pkg": "scripts/snapshot.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "vendor: restore 误删外部 vendor/",
        "file": "scripts/vendor.py",
        "old": "    if path.is_dir() and (owned or force):",
        "new": "    if path.is_dir():",
        "pkg": "scripts/vendor.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 分支名以 - 开头不拦（可注入 git 选项）",
        "file": "scripts/gitflow.py",
        "old": "    if name.startswith(\"-\"):",
        "new": "    if False:",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 分支名 .. / .lock 不拦",
        "file": "scripts/gitflow.py",
        "old": "    if \"..\" in name or \"//\" in name or name.endswith(\"/\") or name.endswith(\".lock\"):",
        "new": "    if False:",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 仓库名不校验",
        "file": "scripts/gitflow.py",
        "old": "    return [] if REPO_RE.match(repo) else [f\"仓库名应为 owner/name：{repo}\"]",
        "new": "    return []",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "snapshot: gc 不删任何清单",
        "file": "scripts/snapshot.py",
        "old": "    kept = snaps[-keep:]",
        "new": "    kept = snaps",
        "pkg": "scripts/snapshot.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "snapshot: gc 永远不执行",
        "file": "scripts/snapshot.py",
        "old": "    if len(snaps) <= keep:",
        "new": "    if True:",
        "pkg": "scripts/snapshot.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: ahead/behind 反着解析",
        "file": "scripts/gitflow.py",
        "old": "        return int(parts[0]), int(parts[1])",
        "new": "        return int(parts[1]), int(parts[0])",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 可清理分支把 main 也算进去",
        "file": "scripts/gitflow.py",
        "old": "    return sorted(name for name in names if name in merged and name != current and name not in bases)",
        "new": "    return sorted(name for name in names if name in merged and name != current)",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 未并入的分支也被标陈旧",
        "file": "scripts/gitflow.py",
        "old": "        row[\"stale\"] = bool(row.get(\"merged\")) and (now - int(row.get(\"last_commit\", now))) > stale_days * 86400",
        "new": "        row[\"stale\"] = (now - int(row.get(\"last_commit\", now))) > stale_days * 86400",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: worktree 落进仓里",
        "file": "scripts/gitflow.py",
        "old": "    return root.parent / f\"{root.name}-{sanitize(branch)}\"",
        "new": "    return root / f\"{sanitize(branch)}\"",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: PR 链按 base 建索引（串不起来）",
        "file": "scripts/gitflow.py",
        "old": "            children = [c for c in by_base.get(chain[-1][\"headRefName\"], []) if c[\"number\"] not in seen]",
        "new": "            children = []",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 链根取全部 PR（重复/环）",
        "file": "scripts/gitflow.py",
        "old": "    roots = [pr for pr in prs if pr[\"baseRefName\"] not in heads]",
        "new": "    roots = list(prs)",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 离干 PR 不告警",
        "file": "scripts/gitflow.py",
        "old": "    off_trunk = [pr for pr in prs if pr[\"baseRefName\"] not in PROTECTED]",
        "new": "    off_trunk = []",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "secret: 不跳过二进制（误报爆炸）",
        "file": "scripts/secret_scan.py",
        "old": "    if is_binary(data):",
        "new": "    if False:",
        "pkg": "scripts/secret_scan.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "secret: 不跳过超大文件",
        "file": "scripts/secret_scan.py",
        "old": "        if path.stat().st_size > max_bytes:",
        "new": "        if False:",
        "pkg": "scripts/secret_scan.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "secret: 只认同行标记，不认上一行",
        "file": "scripts/secret_scan.py",
        "old": "        allowed = ALLOW_MARKER in line or (index > 0 and ALLOW_MARKER in lines[index - 1])",
        "new": "        allowed = ALLOW_MARKER in line",
        "pkg": "scripts/secret_scan.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "secret: 豁免标记失效",
        "file": "scripts/secret_scan.py",
        "old": "        if allowed:",
        "new": "        if False:",
        "pkg": "scripts/secret_scan.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "secret: sk- 规则不生效",
        "file": "scripts/secret_scan.py",
        "old": "    (re.compile(\"sk\" + r\"-[A-Za-z0-9]{16,}\"), \"sk- 形状密钥\"),",
        "new": "    (re.compile(\"NEVER_MATCHES\"), \"sk- 形状密钥\"),",
        "pkg": "scripts/secret_scan.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 不认显式豁免标记",
        "file": "scripts/gitflow.py",
        "old": "            if ALLOW_MARKER in line or (index > 0 and ALLOW_MARKER in lines[index - 1]):",
        "new": "            if False:",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 检查竞态被当成失败",
        "file": "scripts/gitflow.py",
        "old": "    return \"no checks reported\" in lowered or \"no checks\" in lowered",
        "new": "    return False",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "snapshot: intact 不查对象（库坏了也当可用）",
        "file": "scripts/snapshot.py",
        "old": "        if not blob_path(root, entry[\"sha\"]).is_file():",
        "new": "        if False:",
        "pkg": "scripts/snapshot.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "autoflow: 快照库被破坏仍当回退成功",
        "file": "scripts/autoflow.py",
        "old": "    if missing:",
        "new": "    if False:",
        "pkg": "scripts/autoflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "ci-integrity: 拼错的必需检查不判红",
        "file": "scripts/ci_integrity.py",
        "old": "        if not matches:",
        "new": "        if False:",
        "pkg": "scripts/ci_integrity.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "ci-integrity: 只在 push 触发的 job 不判红",
        "file": "scripts/ci_integrity.py",
        "old": "            if not job[\"on_pr\"]:",
        "new": "            if False:",
        "pkg": "scripts/ci_integrity.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "ci-integrity: if 不覆盖 PR 不判红",
        "file": "scripts/ci_integrity.py",
        "old": "            if cond and (\"pull_request\" not in cond or negated):",
        "new": "            if False:",
        "pkg": "scripts/ci_integrity.py",
        "filter": "",
        "runner": "python",
    },
    # ---- 绿即合前的证据审计（必需检查到底跑没跑）----
    {
        "name": "green-audit: 没跑过的必需检查当通过",
        "file": "scripts/green_audit.py",
        "old": "        if conclusion not in PASSING:",
        "new": "        if False:",
        "pkg": "scripts/green_audit.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "green-audit: 必需检查压根没上报也放行",
        "file": "scripts/green_audit.py",
        "old": "        hit = find_reported(context, rollup, ci)\n        if hit is None:",
        "new": "        hit = find_reported(context, rollup, ci) or {\n            \"name\": context, \"conclusion\": \"success\", \"status\": \"\", \"detailsUrl\": \"\"\n        }\n        if hit is None:",
        "pkg": "scripts/green_audit.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "green-audit: 取不到必需清单当\"没有要求\"",
        "file": "scripts/green_audit.py",
        "old": "    if not required:",
        "new": "    if False:",
        "pkg": "scripts/green_audit.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "green-audit: job 清单没取到也当有证据",
        "file": "scripts/green_audit.py",
        "old": "                if entries is None:",
        "new": "                if False:",
        "pkg": "scripts/green_audit.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "green-audit: 0 个 job 的 run 当证据",
        "file": "scripts/green_audit.py",
        "old": "                elif not entries:",
        "new": "                elif False:",
        "pkg": "scripts/green_audit.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "green-audit: 形状不认识返回空列表（混淆\"取不到\"与\"没 job\"）",
        "file": "scripts/green_audit.py",
        "old": "        return body[\"jobs\"]\n    return None",
        "new": "        return body[\"jobs\"]\n    return []",
        "pkg": "scripts/green_audit.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "green-audit: 警告也算拦（不分两档）",
        "file": "scripts/green_audit.py",
        "old": "            \"red\": any(finding[\"blocking\"] for finding in findings),",
        "new": "            \"red\": bool(findings),",
        "pkg": "scripts/green_audit.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "green-audit: 必需清单有拦的但不判红",
        "file": "scripts/green_audit.py",
        "old": "    findings.append({\"kind\": kind, \"detail\": detail, \"blocking\": kind in BLOCKING})",
        "new": "    findings.append({\"kind\": kind, \"detail\": detail, \"blocking\": False})",
        "pkg": "scripts/green_audit.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "green-audit: 批准后推送不点出",
        "file": "scripts/green_audit.py",
        "old": "        if commit_id and head_sha and commit_id != head_sha:",
        "new": "        if False:",
        "pkg": "scripts/green_audit.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "green-audit: 非必需的 skipped 不点出",
        "file": "scripts/green_audit.py",
        "old": "        if conclusion == \"SKIPPED\" and entry[\"name\"] not in required:",
        "new": "        if False:",
        "pkg": "scripts/green_audit.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "green-audit: 控制台编码崩掉判决输出",
        "file": "scripts/green_audit.py",
        "old": "    try:\n        print(text)\n    except UnicodeEncodeError:\n        print(text.encode(encoding, \"replace\").decode(encoding, \"replace\"))",
        "new": "    print(text)",
        "pkg": "scripts/green_audit.py",
        "filter": "",
        "runner": "python",
    },
    # ---- 测试面体检（绿不等于跑了东西）----
    {
        "name": "test-surface: 空壳测试检不出来",
        "file": "scripts/test_surface.py",
        "old": "        if not body.strip():",
        "new": "        if False:",
        "pkg": "scripts/test_surface.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "test-surface: prop_assert 不算断言（属性测试被误判）",
        "file": "scripts/test_surface.py",
        "old": "    r\"(assert|expect\\(|unwrap\\(|panic!|unreachable!|ensure|\"",
        "new": "    r\"(\\bassert\\b|expect\\(|unwrap\\(|panic!|unreachable!|ensure|\"",
        "pkg": "scripts/test_surface.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "test-surface: 扫描器连文档一起扫（自扫误报）",
        "file": "scripts/test_surface.py",
        "old": "    code = blank_strings(text) if is_python else text",
        "new": "    code = text",
        "pkg": "scripts/test_surface.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "test-surface: 裸 ignore 放行",
        "file": "scripts/test_surface.py",
        "old": "    for match in RUST_IGNORE_BARE.finditer(text):",
        "new": "    for match in []:",
        "pkg": "scripts/test_surface.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "test-surface: 无理由 skip 放行",
        "file": "scripts/test_surface.py",
        "old": "        for match in PY_SKIP_NOREASON.finditer(code):",
        "new": "        for match in []:",
        "pkg": "scripts/test_surface.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "test-surface: 空转自测不抓（门拿它当判据=空转）",
        "file": "scripts/test_surface.py",
        "old": "        if not ASSERT_ANY.search(strip_comments(tail)):",
        "new": "        if False:",
        "pkg": "scripts/test_surface.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "test-surface: 未登记的计数单位放行",
        "file": "scripts/test_surface.py",
        "old": "        if name not in baseline:",
        "new": "        if False:",
        "pkg": "scripts/test_surface.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "test-surface: 测试变少不判红",
        "file": "scripts/test_surface.py",
        "old": "        elif entry[\"tests\"] < baseline[name]:",
        "new": "        elif False:",
        "pkg": "scripts/test_surface.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "test-surface: --update 接受测试变少",
        "file": "scripts/test_surface.py",
        "old": "    if grew:",
        "new": "    if False:",
        "pkg": "scripts/test_surface.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "test-surface: 台账里的死单位不报",
        "file": "scripts/test_surface.py",
        "old": "    for name in sorted(set(baseline) - set(units)):",
        "new": "    for name in []:",
        "pkg": "scripts/test_surface.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "test-surface: 警告也算拦（不分两档）",
        "file": "scripts/test_surface.py",
        "old": "        \"red\": any(finding[\"blocking\"] for finding in findings),",
        "new": "        \"red\": bool(findings),",
        "pkg": "scripts/test_surface.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "test-surface: 拦了也不判红",
        "file": "scripts/test_surface.py",
        "old": "        \"red\": any(finding[\"blocking\"] for finding in findings),",
        "new": "        \"red\": False,",
        "pkg": "scripts/test_surface.py",
        "filter": "",
        "runner": "python",
    },
    # ---- 合并取证（冲突解完有没有把另一边整块丢掉）----
    {
        "name": "merge-forensics: 整块取一边检不出来",
        "file": "scripts/merge_forensics.py",
        "old": "        if merged_blob == left_blob and right_blob != left_blob:",
        "new": "        if False:",
        "pkg": "scripts/merge_forensics.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "merge-forensics: 反向整块取也检不出来",
        "file": "scripts/merge_forensics.py",
        "old": "        if merged_blob == right_blob and right_blob != left_blob:",
        "new": "        if False:",
        "pkg": "scripts/merge_forensics.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "merge-forensics: 文件整块没了不报",
        "file": "scripts/merge_forensics.py",
        "old": "        if merged_blob == \"\":",
        "new": "        if False:",
        "pkg": "scripts/merge_forensics.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "merge-forensics: 逐行核对失效（丢行检不出）",
        "file": "scripts/merge_forensics.py",
        "old": "                if line not in text:",
        "new": "                if False:",
        "pkg": "scripts/merge_forensics.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "merge-forensics: 生成物豁免不计数",
        "file": "scripts/merge_forensics.py",
        "old": "        if _is_generated(path):",
        "new": "        if False:",
        "pkg": "scripts/merge_forensics.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "merge-forensics: 什么文件都当生成物（豁免面失控）",
        "file": "scripts/merge_forensics.py",
        "old": "    return name in {\"Cargo.lock\", \"package-lock.json\", \"yarn.lock\", \"poetry.lock\", \"uv.lock\"} \\",
        "new": "    return True \\",
        "pkg": "scripts/merge_forensics.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "merge-forensics: 拦了也不判红",
        "file": "scripts/merge_forensics.py",
        "old": "            \"red\": any(finding[\"blocking\"] for finding in findings)}",
        "new": "            \"red\": False}",
        "pkg": "scripts/merge_forensics.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "merge-forensics: 单亲提交假装查过了",
        "file": "scripts/merge_forensics.py",
        "old": "        base[\"note\"] = \"只有一个父（squash/rebase 落地）——没有「冲突解法」可取证，跳过而不是放行\"",
        "new": "        base[\"note\"] = \"\"",
        "pkg": "scripts/merge_forensics.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "merge-forensics: 取不到父提交当绿",
        "file": "scripts/merge_forensics.py",
        "old": "        return {**base, \"findings\": [{\"kind\": \"取不到父提交\", \"detail\": str(exc), \"blocking\": True}],\n                \"red\": True, \"blocking\": 1}",
        "new": "        return {**base, \"findings\": [{\"kind\": \"取不到父提交\", \"detail\": str(exc), \"blocking\": True}],\n                \"red\": False, \"blocking\": 0}",
        "pkg": "scripts/merge_forensics.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "merge-forensics: 控制台编码崩掉判决输出",
        "file": "scripts/merge_forensics.py",
        "old": "        try:\n            print(line)\n        except UnicodeEncodeError:\n            print(line.encode(encoding, \"replace\").decode(encoding, \"replace\"))",
        "new": "        print(line)",
        "pkg": "scripts/merge_forensics.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "split: 共变相似度不算比例（全连）",
        "file": "scripts/split.py",
        "old": "        score = count / union_size",
        "new": "        score = count",
        "pkg": "scripts/split.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "split: 闭包不追传递依赖",
        "file": "scripts/split.py",
        "old": "        stack.extend(plan[\"internal_deps\"])",
        "new": "        pass",
        "pkg": "scripts/split.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "split: 下游识别失效",
        "file": "scripts/split.py",
        "old": "    return sorted(p[\"crate\"] for p in plans if target in p[\"internal_deps\"])",
        "new": "    return []",
        "pkg": "scripts/split.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 缺必需检查不标",
        "file": "scripts/gitflow.py",
        "old": "    if missing:\n        labels.append(\"missing-required-check\")",
        "new": "    if False:\n        labels.append(\"missing-required-check\")",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 重复 PR 不标",
        "file": "scripts/gitflow.py",
        "old": "        if overlap >= 0.8:",
        "new": "        if False:",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 陈旧不标",
        "file": "scripts/gitflow.py",
        "old": "        if now - stamp > stale_days * 86400:",
        "new": "        if False:",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "doctor: 索引 CRLF 不报",
        "file": "scripts/repo_doctor.py",
        "old": "        if \"i/crlf\" in flags:",
        "new": "        if False:",
        "pkg": "scripts/repo_doctor.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "doctor: 工作树 CRLF 不计数",
        "file": "scripts/repo_doctor.py",
        "old": "        elif \"w/crlf\" in flags:",
        "new": "        elif False:",
        "pkg": "scripts/repo_doctor.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "doctor: CODEOWNERS 缺口不报",
        "file": "scripts/repo_doctor.py",
        "old": "    return [directory for directory in required_dirs if directory not in covered]",
        "new": "    return []",
        "pkg": "scripts/repo_doctor.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "release: 读不到最新 tag",
        "file": "scripts/release.py",
        "old": "    return tags[0].lstrip(\"v\") if tags else \"\"",
        "new": "    return \"\"",
        "pkg": "scripts/release.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "release: 版本与 tag 不一致不拦",
        "file": "scripts/release.py",
        "old": "    if tag and tag != version:",
        "new": "    if False:",
        "pkg": "scripts/release.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "release: 打 tag 前不校验版本号",
        "file": "scripts/release.py",
        "old": "    if current and current != version:",
        "new": "    if False:",
        "pkg": "scripts/release.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "release: 重复 tag 不拦",
        "file": "scripts/release.py",
        "old": "    if git([\"rev-parse\", \"--verify\", f\"refs/tags/v{version}\"], root):",
        "new": "    if False:",
        "pkg": "scripts/release.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "hooks: 覆盖外来钩子",
        "file": "scripts/hooks.py",
        "old": "    if path.exists() and not is_ours(path) and not force:",
        "new": "    if False:",
        "pkg": "scripts/hooks.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "hooks: 卸载不动外来钩子失效",
        "file": "scripts/hooks.py",
        "old": "    if not is_ours(path):\n        print(f\"拒绝：{path} 不是本工具装的，不动它\")",
        "new": "    if False:\n        print(f\"拒绝：{path} 不是本工具装的，不动它\")",
        "pkg": "scripts/hooks.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "hooks: 不拦直推主干",
        "file": "scripts/hooks.py",
        "old": "    refs/heads/main|refs/heads/master)",
        "new": "    refs/heads/never-main)",
        "pkg": "scripts/hooks.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "doctor: 历史大 blob 不报",
        "file": "scripts/repo_doctor.py",
        "old": "        if size >= BIG_FILE:",
        "new": "        if False:",
        "pkg": "scripts/repo_doctor.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "doctor: LFS 方案不区分类型",
        "file": "scripts/repo_doctor.py",
        "old": "    suffixes = sorted({pathlib.PurePosixPath(name).suffix for name in big if pathlib.PurePosixPath(name).suffix})",
        "new": "    suffixes = []",
        "pkg": "scripts/repo_doctor.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: DCO 缺签名不报",
        "file": "scripts/gitflow.py",
        "old": "        if \"Signed-off-by:\" not in body:",
        "new": "        if False:",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: flake 不一致不报",
        "file": "scripts/gitflow.py",
        "old": "            if len(conclusions) > 1:",
        "new": "            if False:",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    # ---- gitflow（一句话提交 GitHub）----
    {
        "name": "gitflow: 密钥扫描失效",
        "file": "scripts/gitflow.py",
        "old": "            for pattern, label in SECRET_PATTERNS:\n                if pattern.search(line):",
        "new": "            for pattern, label in SECRET_PATTERNS:\n                if False:",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 按文件名判敏感文件失效（.env 会被提交）",
        "file": "scripts/gitflow.py",
        "old": "        if path.name in SECRET_FILE_NAMES or path.name == \".env\" or path.name.startswith(\".env.\"):",
        "new": "        if False:",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 空文件列表也放行",
        "file": "scripts/gitflow.py",
        "old": "    if not files:\n        problems.append(",
        "new": "    if False:\n        problems.append(",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 退回 git add -A",
        "file": "scripts/gitflow.py",
        "old": "    commands: list[list[str]] = [[\"git\", \"add\", \"--\", *files]]",
        "new": "    commands: list[list[str]] = [[\"git\", \"add\", \"-A\"]]",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: --no-merge 也会去合",
        "file": "scripts/gitflow.py",
        "old": "    if not args[\"no_merge\"]:",
        "new": "    if True:",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 允许跳出仓库的路径",
        "file": "scripts/gitflow.py",
        "old": "        posix = pathlib.PurePosixPath(name)\n        if \"..\" in posix.parts or posix.is_absolute() or pathlib.Path(name).is_absolute():",
        "new": "        if False:",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 读空的必需清单也往保护上推（等于自己撤保护）",
        "file": "scripts/gitflow.py",
        "old": "    if not wanted:",
        "new": "    if False:",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 不剥只读 url（PUT 必 422）",
        "file": "scripts/gitflow.py",
        "old": "        return {key: strip_urls(value) for key, value in node.items() if key != \"url\"}",
        "new": "        return {key: strip_urls(value) for key, value in node.items()}",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 复核不点名少掉的检查",
        "file": "scripts/gitflow.py",
        "old": "    problems = [f\"少 {context}\" for context in sorted(set(wanted) - set(got))]",
        "new": "    problems = []",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 复核不点名多出来的检查",
        "file": "scripts/gitflow.py",
        "old": "    problems += [f\"多 {context}\" for context in sorted(set(got) - set(wanted))]",
        "new": "    problems += []",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 推保护前不看开着的 PR（会把人卡死）",
        "file": "scripts/gitflow.py",
        "old": "    if added and not force:",
        "new": "    if False:",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 只对新增的必需检查负责（把老账算进本次）",
        "file": "scripts/gitflow.py",
        "old": "    return sorted(set(wanted) - set(current))",
        "new": "    return sorted(wanted)",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 取不到分支 workflow 当\"不缺\"",
        "file": "scripts/gitflow.py",
        "old": "    if jobs is None:\n        return list(wanted)",
        "new": "    if jobs is None:\n        return []",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 拦路清单不点名具体 PR",
        "file": "scripts/gitflow.py",
        "old": "        if gaps:\n            blockers.append(",
        "new": "        if False:\n            blockers.append(",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 读不到开着的 PR 清单照样推",
        "file": "scripts/gitflow.py",
        "old": "                  \"确认无误再加 --force。\")\n            return 1",
        "new": "                  \"确认无误再加 --force。\")\n            return 0",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 读不到 workflow 混成\"确实没这个 job\"",
        "file": "scripts/gitflow.py",
        "old": "            blockers.append(f\"#{pr.get('number')}（{branch}）读不到 workflow ⇒ 无法证明会跑：\"",
        "new": "            blockers.append(f\"#{pr.get('number')}（{branch}）不会上报：\"",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 没有新增也去拦（无关 PR 被卡）",
        "file": "scripts/gitflow.py",
        "old": "    if not added:\n        return blockers",
        "new": "    if False:\n        return blockers",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 冲突预览只印块头不点名文件",
        "file": "scripts/gitflow.py",
        "old": "        if stripped.endswith(\"in both\") or stripped.startswith(\"<<<<<<<\"):",
        "new": "        if False:",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "gitflow: 冲突文件去重失效（同一文件报三遍）",
        "file": "scripts/gitflow.py",
        "old": "            if match.group(1) not in paths:",
        "new": "            if False:",
        "pkg": "scripts/gitflow.py",
        "filter": "",
        "runner": "python",
    },
    {
        "name": "vendor: source replacement 写成错的名字",
        "file": "scripts/vendor.py",
        "old": "        'replace-with = \"vendored-sources\"\\n'",
        "new": "        'replace-with = \"vendor\"\\n'",
        "pkg": "scripts/vendor.py",
        "filter": "",
        "runner": "python",
    },
]


def ran_any(output: str) -> bool:
    match = re.search(r"test result: ok\. (\d+) passed", output)
    return bool(match and int(match.group(1)) > 0)


# 每种语言一条命令口径：pkg 就是要跑的目标（crate 名 或 脚本路径），
# 由 CI 的 collect 步骤按改动文件生成 `--pkg <目标>`。
# 门脚本自测面的标记串只能拼出来：本文件自己也是被检测对象，
# 写死字面量会让 `has_own_logic("scripts/mutation_check.py")` 把自己判成"有自测面"。
SELFTEST_MARKER = "def self_" + "test("
# bash 侧同理：标记串拼出来，别让自己的字面量把检测结果带偏。
SELFTEST_MARKER_SH = "self_" + "test() {"

RUNNERS: dict[str, dict[str, str]] = {
    # cargo：跑 crate 里被点名的那个测试
    "cargo": {"cmd": "cargo", "syntax": "error[E"},
    # python：门脚本自带的 --self-test（正例 + 逐项反证），失败时打印 `FAIL <例名>`
    "python": {"cmd": "python", "syntax": "SyntaxError"},
    # bash：冒烟/门脚本自带的 --self-test，失败时同样打印 `FAIL <例名>`
    # 语法判红的签名分语言环境：中文 Windows 的 bash 打「语法错误」而不是 "syntax error"
    "bash": {"cmd": "bash", "syntax": "syntax error|语法错误"},
}


def selftest_marker(runner: str) -> str:
    return SELFTEST_MARKER_SH if runner == "bash" else SELFTEST_MARKER


def run_tests(pkg: str, test_filter: str, runner: str = "cargo") -> subprocess.CompletedProcess[str]:
    if runner in ("python", "bash"):
        argv = [sys.executable if runner == "python" else "bash", pkg, "--self-test"]
    else:
        argv = ["cargo", "test", "-q", "-p", pkg, test_filter, "--locked"]
    return subprocess.run(
        argv,
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
    )


# 显式豁免：只有"整个 crate 不含自有逻辑"这种**可以被机器复核**的理由才收在这里。
# 复核在 has_own_logic()：一旦该包出现了 fn/impl，豁免自动失效并判红（防止理由烂掉）。
EXEMPT: dict[str, str] = {
    "sansheng": "三值工具箱的伞包：lib.rs 只有 pub use 转发与文档 doctest，无自有规则",
    "scripts/gen_docs_charts.py": "出图脚本，无 --self-test 且非承重判定；不参与发布门禁",
    # 这两个是薄壳（5 / 19 行），没有可钉的判定；一旦有人给它们补 self_test()，豁免自动过期。
    "scripts/bench.sh": "基准薄壳（5 行）：只是把参数转给 cargo bench，无可变异判定",
    "scripts/publish.sh": "发布薄壳（19 行）：版本校验后转给 cargo publish，无可变异判定",
    "crates/trit-immune/scripts/sync-upstream.sh": "上游同步脚本：一键同步本就不成立（补丁表达不了删除），无 --self-test 面",
}

FN_OR_IMPL = re.compile(
    r"^\s*(pub\s+)?(const\s+|unsafe\s+|async\s+)*(fn|impl)\b", re.M
)


def has_own_logic(pkg: str, *, root: pathlib.Path = ROOT) -> bool:
    """豁免是否已过期。

    - python 门脚本（`scripts/*.py`）：**有 `--self-test` 面却不登记变异** ⇒ 过期（该补条目）；
      没有自测面才允许豁免（钉不住任何东西），等它补上自测再逼登记。
    - bash 脚本（`*.sh`）：同上口径，认 `self_test() {`。
    - rust crate：出现了自有 `fn`/`impl` ⇒ 过期（纯转发伞包才允许豁免）。
    """
    if pkg.endswith(".py") or pkg.endswith(".sh"):
        script = root / pkg
        if not script.is_file():
            return False
        text = script.read_text(encoding="utf-8")
        return selftest_marker("bash" if pkg.endswith(".sh") else "python") in text
    src = root / "crates" / pkg / "src"
    if not src.is_dir():
        return True
    for path in sorted(src.rglob("*.rs")):
        body = "\n".join(
            line
            for line in path.read_text(encoding="utf-8").splitlines()
            if not line.lstrip().startswith(("//!", "///", "//"))
        )
        if FN_OR_IMPL.search(body):
            return True
    return False


def runner_of(mutant: dict) -> str:
    """条目的 runner 优先；没写就按 pkg 后缀推断，免得脚本被当成 crate 去 `cargo test`。"""
    explicit = mutant.get("runner")
    if explicit:
        return explicit
    pkg = mutant["pkg"]
    if pkg.endswith(".py"):
        return "python"
    if pkg.endswith(".sh"):
        return "bash"
    return "cargo"


def select(pkgs: list[str]) -> list[dict]:
    if not pkgs:
        return MUTANTS
    return [mutant for mutant in MUTANTS if mutant["pkg"] in pkgs]


TEST_MODULE = re.compile(r"#\[cfg\(test\)\]\s*mod\s+\w+")


def preflight(mutants: list[dict], *, root: pathlib.Path = ROOT) -> list[str]:
    """跑测试之前先把文件与锚点查一遍：路径写错/锚点漂移要立刻报全，不等到跑一半才崩。

    锚点落在 `#[cfg(test)] mod ...` 之后也算错：改测试代码不会改产品行为，
    这类变异就算「没抓住」也不说明断言弱，只会把债账搅浑（拆模块时真踩到过）。
    """
    problems: list[str] = []
    for mutant in mutants:
        raw = mutant["file"]
        if "\\" in raw:
            problems.append(f"{mutant['name']}: 路径含反斜杠（Linux 打不开）: {raw}")
            continue
        path = root / raw
        if not path.is_file():
            problems.append(f"{mutant['name']}: 文件不存在: {raw}")
            continue
        text = path.read_text(encoding="utf-8")
        count = text.count(mutant["old"])
        if count != 1:
            problems.append(f"{mutant['name']}: 锚点出现 {count} 次（{raw}）")
            continue
        anchor = text.find(mutant["old"])
        runner = runner_of(mutant)
        if runner in ("python", "bash"):
            marker = selftest_marker(runner)
            at = text.rfind(marker)
            label = marker
        else:
            # cargo 侧仍按原口径：只认 `#[cfg(test)] mod xxx`，
            # 放宽成裸 `#[cfg(test)]` 会把 trit-build 的 4 条正常条目误判成"落在测试里"。
            matches = [m.start() for m in TEST_MODULE.finditer(text)]
            at = matches[-1] if matches else -1
            label = "#[cfg(test)] mod"
        if at >= 0 and anchor > at:
            problems.append(f"{mutant['name']}: 锚点落在测试/自测代码里（{raw}，{label} 之后）")
    return problems


def classify(out: str, rc: int, runner: str) -> tuple[str, str]:
    """把一次变异运行的结果判成四类：caught / false_kill / crash / unnamed。

    rc==0 的"存活"由调用方先判，不在这里。
    返回 (kind, detail)：caught 的 detail 是点名，crash 的 detail 是崩溃提示。
    这一层是门最容易被改坏的地方（编译红当抓住、把崩溃当断言各踩过一次），
    所以抽成纯函数，由 `--self-test` 用合成输出逐类钉住。
    """
    syntax = RUNNERS[runner]["syntax"]
    # 只认工具链自己打的诊断：断言行的例名里可能出现 "SyntaxError" 这类字样，
    # 把断言文本当成编译失败就会把"真抓住"误判成假杀（门自测第一版就这么翻车）。
    diag = "\n".join(line for line in out.splitlines() if not line.startswith("FAIL "))
    if "could not compile" in diag or any(tok in diag for tok in syntax.split("|")):
        # 变异让代码编不过 ⇒ 测试根本没跑，这条"红"不证明任何断言。
        return "false_kill", ""
    named = next((line for line in out.splitlines() if "FAILED" in line), None)
    if runner in ("python", "bash"):
        # 门脚本自测面的点名方式各不相同，按能点名的优先：
        #   `FAIL <例名>`（module_budget / publish_graph / bash 冒烟脚本）、
        #   `SELF-TEST FAIL: <规则>`（hazard_gate）、
        #   裸 `assert` 抛出的 `AssertionError: <话>`（pr_hygiene）—— 那也是断言。
        for probe in (
            lambda: next((l for l in out.splitlines() if l.startswith("FAIL ")), None),
            lambda: next((l for l in out.splitlines() if "SELF-TEST FAIL" in l), None),
            lambda: next(
                (
                    "断言失败: {} ｜ 位置 {}".format(
                        l.split("AssertionError", 1)[-1].strip(": ").strip() or "(无消息)",
                        next(
                            (
                                f"{m.group(1).rsplit('/', 1)[-1].rsplit(chr(92), 1)[-1]}:{m.group(2)} in {m.group(3)}"
                                for m in reversed(
                                    [
                                        fr
                                        for fr in re.finditer(
                                            r'File "([^"]+)", line (\d+), in (\S+)', out
                                        )
                                    ]
                                )
                                if "mutation_check" not in m.group(1)
                            ),
                            "?",
                        ),
                    )
                    for l in out.splitlines()
                    if "AssertionError" in l
                ),
                None,
            ),
        ):
            if named is None:
                named = probe()
        if named is not None and "self-test:" in out:
            tally = next((l for l in out.splitlines() if "self-test:" in l), "")
            named = f"{named.strip()} ｜ {tally.strip()}"
    tests = sorted({m.group(1) for m in re.finditer(r"^---- (\S+) stdout ----", out, re.M)})
    if named is None and tests:
        named = " / ".join(tests) + " 断言失败"
    if named is None:
        # 没有断言痕迹，只有进程异常退出（Windows 上常见 0xC00000FD 栈溢出、
        # 0xC0000005 访问违例；Linux 上是 signal: 6/11）。这类"红"只证明
        # 去掉守卫会炸，不证明有断言钉住它 ⇒ 单列成 CRASH-KILL，不与断言混计。
        crash = rc < 0 or rc >= 0xC0000000 or bool(
            re.search(r"overflowed its stack|signal: \d+|exit code: 0xc00000", out)
        ) or (
            runner == "python"
            and "Traceback (most recent call last)" in out
            and "AssertionError" not in out
        )
        if crash:
            hint = next(
                (
                    line.strip()
                    for line in out.splitlines()
                    if "overflowed" in line or "exit code" in line or "signal:" in line
                ),
                f"rc={rc}",
            )
            return "crash", hint
        return "unnamed", ""
    return "caught", named.strip()


def main(pkgs: list[str]) -> int:
    # Windows 控制台默认可能是 GBK；变异名里有中国字与 Ł，统一按 UTF-8 输出。
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    mutants = select(pkgs)
    # fail-closed（**逐目标**）：某个目标在登记表里 0 条时，"它绿了"没有任何含义。
    # 多个目标一起选中时更危险——别的目标有变异可杀，这个目标会被静默跳过
    # （2026-11-02 的 trit-immune 就是这样在 CI 报 14s 通过的）。
    # 唯一的松口是 EXEMPT：必须是"纯转发/无判决面"这种**可自证**的理由，自证一失效就判红。
    counts = {pkg: sum(1 for mutant in MUTANTS if mutant["pkg"] == pkg) for pkg in pkgs}
    uncovered = [pkg for pkg, n in counts.items() if n == 0]
    unexplained = [pkg for pkg in uncovered if pkg not in EXEMPT]
    stale = [pkg for pkg in uncovered if pkg in EXEMPT and has_own_logic(pkg)]
    if unexplained or stale:
        scope = ", ".join(unexplained or ["(全仓)"]) if unexplained else ", ".join(stale)
        print(f"NO MUTANTS REGISTERED：{scope} 在 MUTANTS 里是 0 条 ⇒ 判红。"
              "补登记（锚点 + 对应测试名），或确认它是纯转发后登记进 EXEMPT 并写理由。")
        for pkg in stale:
            print(f"EXEMPT 过期：{pkg} 已经有自己的 fn/impl 了，豁免不再成立 ⇒ 请补登记变异。")
        return 1
    for pkg in uncovered:
        print(f"EXEMPT（已自证无自有逻辑）: {pkg} —— {EXEMPT[pkg]}")
    if not mutants:
        if not pkgs:
            print("NO MUTANTS REGISTERED：（全仓）MUTANTS 是空的 ⇒ 判红。")
            return 1
        return 0
    if pkgs:
        detail = ", ".join(f"{pkg}={count}" for pkg, count in sorted(counts.items()))
        print(f"只跑所选包（{detail}）：{len(mutants)} 个变异")
    problems = preflight(mutants)
    if problems:
        print(f"PREFLIGHT FAILED（{len(problems)}/{len(mutants)} 条）:")
        for problem in problems:
            print(f"  {problem}")
        return 1

    baseline: dict[tuple[str, str], bool] = {}
    survived = 0
    false_kills: list[str] = []
    crash_kills: list[str] = []
    unnamed: list[str] = []
    for mutant in mutants:
        key = (mutant["pkg"], mutant["filter"], runner_of(mutant))
        pkg, filt, runner = key
        if key not in baseline:
            result = run_tests(pkg, filt, runner)
            if result.returncode != 0:
                print(f"BASELINE RED: {key}\n{result.stdout[-2000:]}")
                return 1
            baseline[key] = True

        path = ROOT / mutant["file"]
        original_bytes = path.read_bytes()
        # 与 preflight 的 read_text 同口径（通用换行）：CRLF 工作树里锚点仍按 \n 匹配
        original = original_bytes.decode("utf-8").replace("\r\n", "\n")
        count = original.count(mutant["old"])
        if count != 1:
            print(f"BAD MUTANT {mutant['name']}: anchor 出现 {count} 次")
            return 1

        # newline="" 是硬要求：默认写回会把整篇 \n 翻成 \r\n（Windows），
        # bash/sh 见到 \r 直接语法错，结果被误判成"崩溃"而不是断言——本地与 CI 就分叉了。
        path.write_text(
            original.replace(mutant["old"], mutant["new"]),
            encoding="utf-8",
            newline="",
        )
        try:
            result = run_tests(pkg, filt, runner)
        finally:
            # 恢复按字节，保证工作树回到原样（换行风格也不许被门顺手改掉）
            path.write_bytes(original_bytes)
            assert path.read_bytes() == original_bytes, "恢复失败"

        if result.returncode == 0:
            survived += 1
            print(f"SURVIVED  {mutant['name']}")
            continue
        out = result.stdout
        runner = runner_of(mutant)
        kind, detail = classify(out, result.returncode, runner)
        if kind == "false_kill":
            false_kills.append(mutant["name"])
            print(f"FALSE-KILL {mutant['name']}  [{runner} 编译/语法红：测试没跑，不算锁]")
            continue
        if kind == "crash":
            crash_kills.append(mutant["name"])
            print(f"CRASH-KILL {mutant['name']}  [崩溃红 {hex(result.returncode & 0xFFFFFFFF)}：{detail[:100]}]")
            continue
        if kind == "unnamed":
            unnamed.append(mutant["name"])
            print(f"caught    {mutant['name']}  [(未点名测试)]")
            continue
        print(f"caught    {mutant['name']}  [{detail}]")

    killed = len(mutants) - survived - len(false_kills) - len(crash_kills)
    print(
        f"\n断言抓住 {killed}，崩溃抓住 {len(crash_kills)}，编译红假杀 {len(false_kills)}，"
        f"存活 {survived}（共 {len(mutants)}）"
    )
    for name in false_kills:
        print(f"  FALSE-KILL {name}")
    if crash_kills:
        print("  CRASH-KILL（证明守卫移除会炸，但不是断言钉住）：")
        for name in crash_kills:
            print(f"    - {name}")
    if unnamed:
        print(f"  未点名（既无断言也无崩溃签名）: {', '.join(unnamed)}")
    if survived or false_kills:
        print("⇒ 门判红：假杀的条目要么改成语义变异（编得过、只改行为），要么删除。")
        return 1
    return 0


def parse_args(argv: list[str]) -> list[str]:
    pkgs: list[str] = []
    rest = list(argv)
    while rest:
        arg = rest.pop(0)
        if arg == "--pkg" and rest:
            pkgs.append(rest.pop(0))
        elif arg.startswith("--pkg="):
            pkgs.append(arg.split("=", 1)[1])
        else:
            raise SystemExit(f"未知参数: {arg}")
    return pkgs


def self_test() -> int:
    """门自身判定链的自测：合成输出逐类判定 + 临时夹具验 preflight/has_own_logic 边界。

    这不是"再跑一遍变异"：不碰 cargo，毫秒级，专钉门里最容易改坏的那几行
    （编译红当抓住、崩溃当断言、脚本落到 cargo、空集放行，这四类都真踩过）。
    失败打印 `FAIL <例名>`（与门脚本族同口径），全过打印 `self-test: N/N passed`。
    """
    total = 0
    failures: list[str] = []

    def check(label: str, actual, expected) -> None:
        nonlocal total
        total += 1
        if actual != expected:
            failures.append(f"FAIL {label}: 期望 {expected!r}，实际 {actual!r}")

    # 一、四类裁决：编译红不算锁、崩溃不算断言
    check("cargo 编译红→false_kill", classify("error[E0308]: mismatched types", 1, "cargo")[0], "false_kill")
    check("cargo 栈溢出→crash", classify("thread 'x' has overflowed its stack", 134, "cargo")[0], "crash")
    check("cargo 0xc00000fd→crash", classify("boom", 0xC00000FD, "cargo")[0], "crash")
    check(
        "cargo 点名测试→caught",
        classify("---- t1 stdout ----\nthread 't1' panicked", 101, "cargo"),
        ("caught", "t1 断言失败"),
    )
    check("cargo 无痕迹→unnamed", classify("nothing useful", 1, "cargo")[0], "unnamed")
    check("python SyntaxError→false_kill", classify("SyntaxError: invalid syntax", 1, "python")[0], "false_kill")
    check(
        "python FAIL 点名→caught",
        # 真脚本里 FAIL 行与 `self-test:` 计数行是两行，门会把计数拼在后面
        classify("FAIL red: stale baseline\nself-test: 9/10 passed", 1, "python"),
        ("caught", "FAIL red: stale baseline ｜ self-test: 9/10 passed"),
    )
    check("python 裸 traceback→crash", classify("Traceback (most recent call last):\nKeyError: 1", 1, "python")[0], "crash")
    check("python AssertionError→caught", classify("AssertionError: 断言失败: x", 1, "python")[0], "caught")
    check(
        "断言文本里的 SyntaxError 不算编译红",
        classify("FAIL python SyntaxError→false_kill: 期望 a，实际 b\nself-test: 26/27 passed", 1, "python")[0],
        "caught",
    )
    check("bash 中文语法红→false_kill", classify("行 13: 未预期的记号 附近有语法错误", 2, "bash")[0], "false_kill")
    check(
        "bash FAIL 点名→caught",
        classify("FAIL 自测：run 失败后没有中止脚本", 1, "bash"),
        ("caught", "FAIL 自测：run 失败后没有中止脚本"),
    )

    # 二、runner 推断：脚本绝不能落到 cargo 去跑
    check("runner .py→python", runner_of({"pkg": "scripts/x.py"}), "python")
    check("runner .sh→bash", runner_of({"pkg": "crates/a/scripts/x.sh"}), "bash")
    check("runner crate→cargo", runner_of({"pkg": "trit-core"}), "cargo")
    check("runner 显式优先", runner_of({"pkg": "scripts/x.py", "runner": "cargo"}), "cargo")

    # 三、空集 fail-closed：没有登记就"绿"没有含义（真跑 main，输出吞掉免得像报错）
    def rc_of(targets: list[str]) -> object:
        # 判定链被改坏时 main 可能直接抛异常（例如 EXEMPT[pkg] KeyError）；
        # 把异常也变成可比较的返回值，这样它会以断言失败被点名，而不是被算成"崩溃红"。
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                return main(targets)
        except Exception as exc:
            return f"异常 {type(exc).__name__}"

    empty_rc = rc_of(["no-such-target-xyz"])
    exempt_rc = rc_of(["sansheng"])
    # 多目标一起选中时，0 条的那个不能被别的目标"带绿"
    partial_rc = rc_of(["scripts/gen_docs_charts.py", "no-such-target-xyz"])
    covered_rc = rc_of(["sansheng", "scripts/gen_docs_charts.py"])
    saved = MUTANTS[:]
    try:
        MUTANTS.clear()
        whole_empty_rc = rc_of([])
    finally:
        MUTANTS.extend(saved)
    check("空集未豁免→判红", empty_rc, 1)
    # 这三条的夹具是主仓里真实存在的目标（伞包 sansheng、无自测面的 gen_docs_charts.py）；
    # 独立镜像仓里没有 ⇒ 大声 SKIP，不许把它们算成"通过"。
    has_ambient = (ROOT / "crates" / "sansheng").is_dir() and (ROOT / "scripts" / "gen_docs_charts.py").is_file()
    if has_ambient:
        check("空集但自证豁免→放行", exempt_rc, 0)
        check("多目标里 0 条的那个→判红", partial_rc, 1)
        check("全部是自证豁免→放行", covered_rc, 0)
    else:
        print("SKIP 豁免/多目标 3 条：本仓没有夹具目标（主仓专属）")
    check("全仓登记为空→判红", whole_empty_rc, 1)
    check("选中过滤", [m["pkg"] for m in select(["sansheng"])], [])
    check("空选择=全表", len(select([])), len(MUTANTS))
    # 豁免项自己也要可复核：路径写错时 has_own_logic 会返回 False，豁免会静静地套在空气上
    if (ROOT / "crates").is_dir():
        for pkg in EXEMPT:
            exists = pkg.endswith((".py", ".sh")) and (ROOT / pkg).is_file()
            check(f"EXEMPT 路径存在：{pkg}", exists or (ROOT / "crates" / pkg).is_dir(), True)
    else:
        print("SKIP EXEMPT 台账对账：本仓没有 crates/ 目录（台账属于主仓）")

    # 四、豁免过期与 preflight 边界（按语言分判），用临时夹具，不碰真仓
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        (root / "crates" / "umbrella" / "src").mkdir(parents=True)
        (root / "crates" / "umbrella" / "src" / "lib.rs").write_text("pub use trit_core::*;\n", encoding="utf-8")
        (root / "crates" / "real" / "src").mkdir(parents=True)
        (root / "crates" / "real" / "src" / "lib.rs").write_text("pub fn f() -> u8 {\n    1\n}\n", encoding="utf-8")
        (root / "scripts").mkdir()
        (root / "scripts" / "plain.py").write_text("print(1)\n", encoding="utf-8")
        (root / "scripts" / "gate.py").write_text("x = 1\ndef self_test():\n    return 0\ny = 2\n", encoding="utf-8")
        (root / "scripts" / "gate.sh").write_text("a=1\nself_test() {\n  :\n}\nb=2\n", encoding="utf-8")
        check("伞包（只 pub use）→不过期", has_own_logic("umbrella", root=root), False)
        check("自有 fn→过期", has_own_logic("real", root=root), True)
        check("脚本无自测面→不过期", has_own_logic("scripts/plain.py", root=root), False)
        check("python 有自测面→过期", has_own_logic("scripts/gate.py", root=root), True)
        check("bash 有自测面→过期", has_own_logic("scripts/gate.sh", root=root), True)

        def fixture(file: str, old: str, runner: str) -> list[dict]:
            return [{"name": "fixture", "file": file, "old": old, "pkg": file, "filter": "", "runner": runner}]

        check("preflight 锚点在 python 自测面之后→报错", len(preflight(fixture("scripts/gate.py", "y = 2", "python"), root=root)), 1)
        check("preflight 锚点在自测面之前→放行", preflight(fixture("scripts/gate.py", "x = 1", "python"), root=root), [])
        check("preflight 锚点在 bash 自测面之后→报错", len(preflight(fixture("scripts/gate.sh", "b=2", "bash"), root=root)), 1)

    if failures:
        for line in failures:
            print(line)
        print(f"self-test: {total - len(failures)}/{total} passed")
        return 1
    print(f"self-test: {total}/{total} passed")
    return 0


if __name__ == "__main__":
    if "--self-test" in sys.argv[1:]:
        sys.exit(self_test())
    sys.exit(main(parse_args(sys.argv[1:])))
