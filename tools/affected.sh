#!/usr/bin/env bash
# affected.sh -- default entry for the substring-selection planner.
#
# usage:
#   tools/affected.sh -Path <root> -Changed <file>[,<file>] -Lang rust|python|node \
#                     [-Out plan.json] [-AssumeIsolated] [-MaxFiles <n>]
#
# The contract lives in affected.py: a subset plan may only claim "equivalent to full"
# when it carries the reachable set, and anything unmodelled must degrade to a full run.
#
# Rust goes down to cargo-target level: within each changed package the plan selects the
# targets (lib/bins/tests/examples/benches) whose mod tree reaches a changed file, e.g.
# `cargo test -p trit-parts --bin parts-cli --no-fail-fast`; when every target is
# reachable (or the cut is not provable) it degrades to today's package-level `-p` form.
# A target may be EXCLUDED only when its mod tree is walked to the end. cfg-gated mods
# count as reachable when the file exists (over-approximation, sound under any feature
# set); #[path] literals are resolved and fall back if missing; include!(), cfg_attr,
# macro-generated mods, build.rs and non-Rust files fall back to the package level with
# an unproven reason. include_str!/include_bin! expand to literals and cannot declare
# modules, so they do not affect what a mod tree reaches. Plans stay additive: rust adds
# a `targets` array, every other field keeps its meaning. -MaxFiles (default 5000) caps
# how many source files the walk may read; exceeding it fails closed to package level.

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python affected
exec "$PY" "$here/affected.py" "$@"
