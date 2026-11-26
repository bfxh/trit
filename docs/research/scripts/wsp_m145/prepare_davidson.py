"""M1.45: prepare Davidson third hate corpus.

- Binary primary: class 0 (hate) -> 1; class 1 (offensive) + class 2 (neither) -> 0.
- No official split; fixed stratified 80/20, random_state=7.
- Deduplicate normalized text; remove test rows overlapping TweetEval/hate or
  HateXplain train+test by exact normalized text.
"""

from __future__ import annotations

import collections
import csv
import hashlib
import json
import re
from pathlib import Path

from sklearn.model_selection import train_test_split

ROOT = Path(r"D:\trit-results\davidson")
RAW = ROOT / "raw" / "labeled_data.csv"
TWEET = Path(r"D:\trit-results\tweet-hate\raw")
HX = Path(r"D:\trit-results\hatexplain\raw")
SEED = 7


def norm(text: str) -> str:
    return re.sub(r"\s+", " ", str(text)).strip()


def read_jsonl_texts(paths: list[Path]) -> set[str]:
    out = set()
    for path in paths:
        with path.open(encoding="utf-8") as fh:
            for line in fh:
                out.add(norm(json.loads(line)["text"]))
    return out


def main() -> None:
    rows = list(csv.DictReader(RAW.open(encoding="utf-8")))
    seen = set()
    clean = []
    for row in rows:
        text = norm(row["tweet"])
        if not text or text in seen:
            continue
        seen.add(text)
        raw_class = int(row["class"])
        clean.append({"text": text, "label": 1 if raw_class == 0 else 0, "raw_class": raw_class})
    labels = [r["label"] for r in clean]
    train, test = train_test_split(
        clean, test_size=0.2, stratify=labels, random_state=SEED
    )

    overlap = read_jsonl_texts(
        [
            TWEET / "train.jsonl",
            TWEET / "test.jsonl",
            HX / "train.jsonl",
            HX / "test.jsonl",
        ]
    )
    test = [r for r in test if r["text"] not in overlap]

    out_dir = ROOT / "raw"
    for split, items in (("train", train), ("test", test)):
        with (out_dir / f"{split}.jsonl").open("w", encoding="utf-8") as fh:
            for i, row in enumerate(items):
                fh.write(
                    json.dumps(
                        {
                            "id": f"{split}-{i}",
                            "text": row["text"],
                            "label": row["label"],
                            "raw_class": row["raw_class"],
                            "source": "twitter",
                        },
                        ensure_ascii=False,
                    )
                    + "\n"
                )

    summary = {
        "dataset": "Davidson et al. 2017",
        "url": "https://raw.githubusercontent.com/t-davidson/hate-speech-and-offensive-language/master/data/labeled_data.csv",
        "sha256": hashlib.sha256(RAW.read_bytes()).hexdigest().upper(),
        "raw_rows": len(rows),
        "raw_class_counts": dict(collections.Counter(int(r["class"]) for r in rows)),
        "clean_rows": len(clean),
        "clean_class_counts": dict(collections.Counter(labels)),
        "fit_rows": len(train),
        "test_rows": len(test),
        "fit_label_counts": dict(collections.Counter(r["label"] for r in train)),
        "test_label_counts": dict(collections.Counter(r["label"] for r in test)),
        "fit_hate_prior": sum(r["label"] for r in train) / len(train),
        "test_hate_prior": sum(r["label"] for r in test) / len(test),
        "split": "stratified 80/20, random_state=7",
        "removed_test_overlap": 1,
        "note": "Binary mapping; descriptive external transport only.",
    }
    (ROOT / "prepare-summary.json").write_text(
        json.dumps(summary, indent=2, ensure_ascii=False), encoding="utf-8"
    )
    print(json.dumps(summary, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
