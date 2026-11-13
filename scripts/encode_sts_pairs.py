"""编码一组「更难」的 STS 风格句子对：细粒度差异、否定、长句。

与之前的 `encode_sts.py` 不同，这里标签来自**人工标注**，不是 cos 阈值：

- 同义改写（含长句改写）→ positive
- 细粒度差异（垫子 vs 沙发、大房间 vs 小房间）→ negative
- 否定（is / is not）→ negative
- 跨主题 → negative

句子对用模板批量生成 + 手写补充，目标是让 edit 集（三值学生判错的对）足够大。

用法（隔离 venv 里）：

```bash
python scripts/encode_sts_pairs.py --model <本地 MiniLM 快照> --out sts-pairs.json
```
"""

import argparse
import json

import torch
from transformers import AutoModel, AutoTokenizer


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True, help="本地 all-MiniLM-L6-v2 快照目录")
    parser.add_argument("--out", required=True, help="输出 pairs JSON")
    return parser.parse_args()


def build_pairs():
    pairs = []

    def add(left, right, label):
        pairs.append((left, right, label))

    # 1) 同义改写（positive）
    paraphrases = [
        ("The cat sat on the mat.", "A cat is sitting on a mat."),
        ("Quantum computers use qubits.", "Qubits are the unit of quantum computation."),
        ("The stock market fell sharply today.", "Share prices dropped a lot today."),
        ("She wrote a long letter to her friend.", "She composed a lengthy letter to a friend."),
        ("The river flows through the valley.", "A river runs through the valley."),
        ("Neural networks learn from data.", "Models learn patterns from training data."),
        ("The train arrives at noon.", "The train will arrive at midday."),
        ("He plays the piano every evening.", "He practices piano each evening."),
        ("The library closes at six.", "The library shuts at six o'clock."),
        ("The doctor examined the patient carefully.", "The physician checked the patient thoroughly."),
        ("The children built a sandcastle on the beach.", "Kids made a sandcastle at the beach."),
        ("The company hired three engineers this month.", "Three engineers were hired by the company this month."),
        ("The recipe calls for two cups of flour.", "Two cups of flour are needed for this recipe."),
        ("The meeting was moved to Friday.", "The meeting has been rescheduled to Friday."),
        ("The bird flew over the lake.", "A bird flew above the lake."),
        ("The students prepared for the final exam.", "The pupils got ready for the final test."),
        ("The rain stopped before dawn.", "The rain ended before sunrise."),
        ("The museum exhibits ancient pottery.", "Ancient ceramics are displayed at the museum."),
        ("The manager approved the budget.", "The budget was approved by the manager."),
        ("The athlete broke the record.", "The record was broken by the athlete."),
    ]
    for left, right in paraphrases:
        add(left, right, "positive")

    # 2) 细粒度差异（negative）：形容词、动词、对象
    adjectives = [
        ("large", "small"),
        ("blue", "green"),
        ("hot", "cold"),
        ("fast", "slow"),
        ("cheap", "expensive"),
        ("new", "old"),
        ("tall", "short"),
        ("bright", "dark"),
        ("empty", "full"),
        ("quiet", "loud"),
    ]
    for left, right in adjectives:
        add(f"The room is {left}.", f"The room is {right}.", "negative")

    verbs = [
        ("kicked", "caught"),
        ("painted", "cleaned"),
        ("bought", "sold"),
        ("read", "wrote"),
        ("opened", "closed"),
        ("pushed", "pulled"),
        ("found", "lost"),
        ("built", "destroyed"),
    ]
    for left, right in verbs:
        add(f"The boy {left} the ball.", f"The boy {right} the ball.", "negative")

    objects = [
        ("The cat sat on the mat.", "The cat sat on the sofa."),
        ("The restaurant serves Italian food.", "The restaurant serves Chinese food."),
        ("The laptop has a large screen.", "The laptop has a small screen."),
        ("He drove to the office.", "He walked to the office."),
        ("The package arrived on Monday.", "The package arrived on Tuesday."),
        ("She works as a teacher.", "She works as a nurse."),
        ("The dog barked at the stranger.", "The dog wagged its tail at the stranger."),
    ]
    for left, right in objects:
        add(left, right, "negative")

    # 3) 否定（negative）
    negations = [
        ("The cat is on the mat.", "The cat is not on the mat."),
        ("The store is open today.", "The store is closed today."),
        ("She likes coffee.", "She does not like coffee."),
        ("The train arrived on time.", "The train did not arrive on time."),
        ("The project is finished.", "The project is not finished."),
        ("He speaks French fluently.", "He does not speak French fluently."),
        ("The tickets are available.", "The tickets are not available."),
        ("The machine works properly.", "The machine does not work properly."),
        ("The report includes the data.", "The report excludes the data."),
        ("The door was locked.", "The door was unlocked."),
        ("The results were significant.", "The results were not significant."),
        ("The applicant has experience.", "The applicant lacks experience."),
        ("The road is safe at night.", "The road is unsafe at night."),
        ("The sample was contaminated.", "The sample was uncontaminated."),
        ("The answer is correct.", "The answer is incorrect."),
    ]
    for left, right in negations:
        add(left, right, "negative")

    # 4) 长句：改写（positive）+ 细粒度/否定（negative）
    long_pairs = [
        (
            "The committee decided to postpone the meeting until next week because several "
            "members were unavailable.",
            "Several members were unavailable, so the committee postponed the meeting to next week.",
            "positive",
        ),
        (
            "The research team published the results after the review process was completed.",
            "After the review, the research team released the results.",
            "positive",
        ),
        (
            "The committee decided to postpone the meeting until next week because several "
            "members were unavailable.",
            "The committee decided to cancel the meeting because several members were unavailable.",
            "negative",
        ),
        (
            "The research team published the results after the review process was completed.",
            "The research team did not publish the results after the review process was completed.",
            "negative",
        ),
        (
            "The new policy requires all employees to complete the training before the end of the month.",
            "All employees must finish the training by the end of the month under the new policy.",
            "positive",
        ),
        (
            "The new policy requires all employees to complete the training before the end of the month.",
            "The new policy requires only managers to complete the training before the end of the month.",
            "negative",
        ),
        (
            "The factory reduced its emissions by installing new filters in the production line.",
            "New filters installed in the production line lowered the factory's emissions.",
            "positive",
        ),
        (
            "The factory reduced its emissions by installing new filters in the production line.",
            "The factory increased its emissions after installing new filters in the production line.",
            "negative",
        ),
        (
            "The patient recovered quickly because the treatment was started early.",
            "Early treatment helped the patient recover quickly.",
            "positive",
        ),
        (
            "The patient recovered quickly because the treatment was started early.",
            "The patient recovered slowly because the treatment was started late.",
            "negative",
        ),
    ]
    for left, right, label in long_pairs:
        add(left, right, label)

    # 5) 跨主题（negative）
    cross = [
        ("The cat sat on the mat.", "Quantum computers use qubits."),
        ("She wrote a long letter to her friend.", "The stock market fell sharply today."),
        ("The river flows through the valley.", "He plays the piano every evening."),
        ("The library closes at six.", "The doctor examined the patient carefully."),
        ("The children built a sandcastle on the beach.", "The company hired three engineers this month."),
        ("The recipe calls for two cups of flour.", "The meeting was moved to Friday."),
        ("The bird flew over the lake.", "The students prepared for the final exam."),
        ("The rain stopped before dawn.", "The museum exhibits ancient pottery."),
        ("The manager approved the budget.", "The athlete broke the record."),
        ("The room is quiet.", "The boy kicked the ball."),
        ("The store is open today.", "The laptop has a large screen."),
        ("The train arrived on time.", "The restaurant serves Italian food."),
        ("The project is finished.", "The door was locked."),
        ("The results were significant.", "He drove to the office."),
        ("The answer is correct.", "The dog barked at the stranger."),
    ]
    for left, right in cross:
        add(left, right, "negative")

    return pairs


def main():
    args = parse_args()
    triples = build_pairs()
    sentences = sorted({text for left, right, _ in triples for text in (left, right)})

    tokenizer = AutoTokenizer.from_pretrained(args.model)
    model = AutoModel.from_pretrained(args.model)
    model.eval()
    with torch.no_grad():
        batch = tokenizer(sentences, padding=True, truncation=True, return_tensors="pt")
        hidden = model(**batch).last_hidden_state
        mask = batch["attention_mask"].unsqueeze(-1).to(hidden.dtype)
        pooled = (hidden * mask).sum(dim=1) / mask.sum(dim=1)
        vectors = torch.nn.functional.normalize(pooled, dim=1).tolist()
    index = {text: vectors[position] for position, text in enumerate(sentences)}

    pairs = [
        {"a": index[left], "b": index[right], "label": label}
        for left, right, label in triples
    ]
    payload = {"model": args.model, "sentences": sentences, "pairs": pairs}
    with open(args.out, "w", encoding="utf-8") as handle:
        json.dump(payload, handle)
    positives = sum(1 for _, _, label in triples if label == "positive")
    print(
        json.dumps(
            {
                "out": args.out,
                "sentences": len(sentences),
                "pairs": len(pairs),
                "positive": positives,
                "negative": len(pairs) - positives,
            }
        )
    )


if __name__ == "__main__":
    main()
