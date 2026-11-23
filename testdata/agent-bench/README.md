# agent-bench fixtures

[`tasks.json`](tasks.json) is the single source of truth for the local agent
benchmark: 28 tasks across Python, Node, Go and Rust, each with a real test
command, a gold patch and a behaviorally wrong negative patch. It is not a copy
of any public benchmark; every fixture is small, self-contained and deterministic.

## Task schema

A patch-like task (`kind` = `patch` or `compact`) has:

- `files`: initial fixture tree (relative paths, UTF-8 text only).
- `gold`: files replaced for the reference solution.
- `negative`: files replaced for a behaviorally wrong solution.
- `changed` / `negative_changed`: paths passed to `agent_verify -Changed`.
- `commands`: commands run by `agent_verify`; `{py}` is replaced with the quoted
  current Python executable.
- `extra`: extra `agent_verify` flags (`--compact`, `--budget-token 0`, `-DryRun`).
- `expect`: `{gold, negative}` expected verdict classes.

Special tasks (`policy`, `budget`, `dry_run`) have no gold/negative patch; they
assert the corresponding guard branch instead.

## Adding a task

1. Add a JSON object with a unique `id`, category, source label and files.
2. Make sure the initial/gold/negative variants are behaviorally different.
3. Run `python tools/agent_bench.py -SelfTest`, then
   `python tools/agent_bench.py -Run -Task <id>`.
4. Do not put upstream copyrighted code or private data into this directory.

Candidate trees for an external solver live outside this directory, e.g.
`E:\tmp\solutions\<task-id>\<relative-path>`, and are read with
`python tools/agent_bench.py -Run -Candidates E:\tmp\solutions`.
