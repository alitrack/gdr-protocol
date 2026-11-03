# Evaluation: two A/B rounds, raw data

Independent implementation on a private Rust harness (single-binary agent,
Rust tool registry, streaming OpenAI-compatible provider). Paper
(arXiv:2610.32813) has no official code; these numbers are ours, not
reproductions of the paper's benchmarks.

## Setup

- Model: Qwen3.8-27B, local GPU server (10.10.10.115:8014, SGLang),
  temperature per harness default.
- Tasks: 4 mini coding tasks (fib / string-reverse / is-prime / median).
  Each: write a module file, run a pre-written pytest file, report output.
  Success = test file prints PASS (runner-verified, not model-claimed).
- Arms differ ONLY in the prompt suffix and the runtime switch:
  - **off** — plain 3-step instruction.
  - **tool** — plus authoring instruction + `declare_edg` tool, switch on.
  - **inline** — plus authoring instruction + fenced ```edg block request,
    switch on.
- Metrics from the harness run report: physical model calls (steps),
  prompt/completion tokens from provider usage (stream_options.include_usage),
  certified substitutions from the `[edg] cert#N` ledger.

## Round 1 (mechanism only, tool channel)

| arm | success | steps | prompt tok | completion tok | total | bypasses |
|---|---|---|---|---|---|---|
| off | 4/4 | 19 | 24,336 | 1,170 | 25,506 | 0 |
| on  | 4/4 | 15 | 24,336 | 3,735 | 28,071 | 4 |

Reading: substitutions are real (4 certified, steps -21%, success intact)
but **total tokens +10%** — the authoring tax (2,565 extra completion
tokens of declaration JSON) exceeded the callbacks saved. This is the
substitution paradox of paper App. D.8 reproduced at small scale, and it is
why the certificate ledger matters: the "savings" would have been invisible
(and claimable anyway) without physical call counts.

## Round 2 (three arms, revised instruction)

Instruction revised to "only declare actions you have ALREADY fully
decided"; inline channel added.

| arm | success | steps | prompt tok | completion tok | total | bypasses |
|---|---|---|---|---|---|---|
| off    | 4/4 | 21 | 133,856 | 2,465 | 136,321 | 0 |
| tool   | 4/4 | 11 | 69,901 | 5,558 | 75,459 | 5 |
| inline | 4/4 | 12 | 76,953 | 3,029 | 79,982 | 0 |

Readings:

1. **-45% total tokens in the tool arm, but only 5 substitutions.** The
   bulk of the saving is behavioral: steps fell 21 → 11. The authoring
   instruction made the model plan completely before acting, eliminating
   exploratory tool calls. The mechanism contributed the substitution
   count, not the headline.
2. **Inline channel fired zero substitutions.** When the model emitted both
   a fenced block and tool calls in one streaming response, stream
   assembly frequently dropped the text block. The -41% in that arm is
   entirely the behavioral effect. Inline declaration needs a structured
   side-channel, not free text.
3. Note prompt tokens differ across arms within round 2 because fewer
   turns mean fewer context replays — that compounding is real and is part
   of why behavioral savings dominate.

## Cross-check against the paper

| claim | paper | our measurement |
|---|---|---|
| headline savings | 87.93% (96.28k → 11.62k tok/task) | -45% (round 2 tool arm) — but mostly behavioral |
| clean ablation GDR | 26.91% (one CI excluding zero) | mechanism-only contribution: 5 subs in 4 tasks |
| CCS ablation | 19.52% (CI includes zero) | consistent with "small, workload-dependent" |
| paradox case (App. D.8) | fast path 94% coverage, +127% tokens | round 1: +10% tokens at 4 subs |
| general-agent coverage | GAIA2 fast-path coverage 1.40% | inline arm coverage 0/12 opportunities |

The paper's honest ablations and our independent numbers agree on the
shape: configuration-level effects are large; mechanism-level contribution
is moderate and workload-dependent; short tasks lose money.

## Threats to validity

- 4 tasks, 1 model, 1 seed per arm — directional, not statistical. Treat
  as a mechanism smoke test + accounting demo, not a benchmark.
- Authoring instruction and mechanism are confounded in the "on" arms by
  design (that is how the paper deploys too); the inline arm accidentally
  isolates the instruction-only effect (0 substitutions, -41%).
- Task family (write-code → run-test) is the mechanism's best case; do not
  extrapolate to open-ended tasks (the paper's own GAIA2 coverage of 1.4%
  is the warning).

Raw per-task JSON: `data/round1.json`, `data/round2.json`.
