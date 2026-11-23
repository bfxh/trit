# work-bench task fixtures

`tasks.json` is a deterministic, work-task benchmark for the artifacts an agent
leaves behind.  It is intentionally **not** a code benchmark: there is no patch,
no compiler and no hidden test runner.  Each task has a prompt, input files,
a reference artifact set, a defective negative artifact set, and deterministic
checks over the resulting files.

Current suite: **20 tasks, 15 categories, 81 checks, 22 artifact specs** (some
tasks produce multiple files).  The authoritative current counts are in
`docs/engineering/WORK-BENCH.md` and in
`python tools/work_bench.py -Run -Out ...`.

## Task shape

```json
{
  "id": "data-budget-xlsx",
  "title": "从预算明细生成带公式的 XLSX",
  "category": "finance",
  "source": "finance",
  "prompt": "...",
  "inputs": {"budget.csv": "..."},
  "artifacts": {
    "budget.xlsx": {
      "format": "xlsx",
      "sheets": [{"name": "Budget", "rows": [["科目", "金额"], ["研发", 1200], ["合计", "=SUM(B2:B3)"]]}]
    }
  },
  "negative": {
    "budget.xlsx": {
      "format": "xlsx",
      "sheets": [{"name": "Budget", "rows": [["科目", "金额"], ["研发", 1200], ["合计", 999]]}]
    }
  },
  "checks": [
    {"artifact": "budget.xlsx", "type": "xlsx_sheet", "values": ["Budget"]},
    {"artifact": "budget.xlsx", "type": "xlsx_cell", "sheet": "Budget", "cell": "B3", "equals": "=SUM(B2:B3)"}
  ]
}
```

Artifacts may be:

- a UTF-8 string (`md`, `json`, `csv`, `txt`);
- an object with `"format": "xlsx"` and `"sheets"` (written by `openpyxl`);
- a file tree represented by multiple artifact paths (`sorted/...`).

Use `"remove_inputs": ["inbox/a.txt"]` when the task requires moving inputs
rather than copying them (used by `file-triage`).  Reference materialization
removes those paths after writing the output; negative materialization leaves
them in place so the check catches the incomplete move.

## Check types

| Group | Types |
|---|---|
| Markdown | `md_sections`, `md_phrases`, `md_min_chars`, `md_forbidden` |
| JSON | `json_keys`, `json_value`, `json_min_chars`, `json_contains`, `json_forbidden`, `json_list_len`, `json_list_field_min`, `json_no_overlap`, `json_dag` |
| CSV | `csv_columns`, `csv_row_count`, `csv_no_nulls`, `csv_lookup` |
| XLSX | `xlsx_sheet`, `xlsx_cell`, `xlsx_formula` |
| Files | `files_exist`, `files_absent` |

The checks are deliberately deterministic and cheap.  They prove that the
required sections, facts, row/column contracts, formulas, dates, dependencies
and file moves exist.  They do **not** prove prose quality or factual
correctness beyond the facts encoded in the fixture.

## Commands

```powershell
python tools/work_bench.py -SelfTest
python tools/work_bench.py -List
python tools/work_bench.py -Run -Out E:\tmp\work-bench-report-20.json
python tools/work_bench.py -Run -Repeat 2 -Out E:\tmp\work-bench-repeat2-20.json
python tools/work_bench.py -Run -Task data-budget-xlsx -Strict -Out E:\tmp\one.json
python tools/work_bench.py -Run -Candidates <candidate-root> -Strict
python tools/cmd_driver.py -Plan testdata/work-bench/ptc-plan.json -Out F:\trit-target-data\ptc-receipt.json
```

`ptc-plan.json` 是同一套任务的一键质量门：preflight、`-SelfTest`、`-Run`、`-Repeat 2` 四步，
只把摘要交回模型；完整报告和日志留在 receipt。它不跳过任何检查，任一步失败即停止。

`-Candidates <dir>` copies only artifacts that exist under `<candidate-root>/
<task-id>/...` into a fresh sandbox; missing artifacts fail their checks.  It
does not silently substitute the reference artifacts, so a candidate cannot
pass by doing nothing.

## Adding a task

1. Add a task with at least two checks and both `artifacts` and `negative`.
2. Reference must pass all checks; negative must fail at least one.
3. Every check artifact must exist in both reference and negative maps.
4. Run `python tools/work_bench.py -SelfTest`; add a case to
   `tools/selftest.ps1` if the task introduces a new check type.
5. Keep fixtures synthetic and free of personal data.  Real customer data does
   not belong in this repository.

The suite validates at least 20 tasks, unique ids, at least 10 categories,
at least two checks per task, known check types and reference/negative coverage.
