#!/usr/bin/env python3
"""N1 批次汇总：samples_manifest.tsv + review_worksheet.tsv（机器列预填）。

用法（批次目录结构）：
  <base>/{benign-x64,benign-pe32,benign-elf64,attack-sim-x64}/  logs-*.jsonl
  python scripts/n1_tables.py [base]

产出 <base>/samples_manifest.tsv（每样本 sha256 注册）与
review_worksheet.tsv（每 judgment 一行，support/refute 带出，人工两列
留空——N1 的人工面只填 human_verdict/reason 两列）。
"""

from __future__ import annotations

import hashlib
import json
import sys
from collections import Counter
from pathlib import Path

BATCHES = ['benign-x64', 'benign-pe32', 'benign-elf64', 'attack-sim-x64']


def main() -> int:
    base = Path(sys.argv[1] if len(sys.argv) > 1 else 'E:/tmp/n1')
    # ── manifest：每样本一行（批次 / 文件名 / sha256 / 大小）────────────
    manifest_rows = ['batch\tfile\tsha256\tbytes']
    for batch in BATCHES:
        for f in sorted((base / batch).iterdir()):
            if not f.is_file():
                continue
            data = f.read_bytes()
            sha = hashlib.sha256(data).hexdigest()
            manifest_rows.append(f'{batch}\t{f.name}\t{sha}\t{len(data)}')
    (base / 'samples_manifest.tsv').write_text(
        '\n'.join(manifest_rows) + '\n', encoding='utf-8')

    # ── worksheet：每 judgment 一行（support/refute 带出，人工两列留空）──
    ws_rows = ['batch\tsha16\tjudgment\tvalue\tevidence\thuman_verdict\treason']
    true_rows = []
    for batch in BATCHES:
        log = base / f'logs-{batch}.jsonl'
        if not log.exists():
            continue
        for line in log.read_text(encoding='utf-8').splitlines():
            try:
                r = json.loads(line)
            except json.JSONDecodeError:
                continue
            if r.get('kind') != 'judgment':
                continue
            sha = r.get('sha256', '')[:16]
            jid = r.get('id', '')
            ev = json.dumps({'support': r.get('support', []),
                             'refute': r.get('refute', []),
                             'unknown': r.get('unknown', [])},
                            ensure_ascii=False)[:220]
            value = r.get('value', '')
            ws_rows.append(f'{batch}\t{sha}\t{jid}\t{value}\t{ev}\t\t')
            if value == 'true':
                true_rows.append(f'{batch}\t{sha}\t{jid}')
    (base / 'review_worksheet.tsv').write_text(
        '\n'.join(ws_rows) + '\n', encoding='utf-8')

    total = len(ws_rows) - 1
    vals = Counter(row.split('\t')[3] for row in ws_rows[1:])
    print(f'judgment 总数: {total}')
    print('按值分布:', dict(vals))
    print(f'true（-1 入账）{len(true_rows)} 条，按 批×judgment 聚合：')
    agg = Counter((r.split('\t')[0], r.split('\t')[2]) for r in true_rows)
    for (batch, jid), n in sorted(agg.items()):
        print(f'  {batch}\t{jid}\t{n}')
    return 0


if __name__ == '__main__':
    if hasattr(sys.stdout, 'reconfigure'):
        sys.stdout.reconfigure(encoding='utf-8', errors='replace')
    sys.exit(main())
