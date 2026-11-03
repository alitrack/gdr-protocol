# GDR Wire Protocol (v0.1)

Harness-neutral contract for guarded decision reuse. Any agent harness that
owns its model-call loop can implement this; the protocol does not assume a
specific tool registry, provider, or language.

## 1. Action bank

A bank is an ordered list of nodes declared by the model **alongside** its
current tool calls (or via an inline fenced block — see §4).

```json
[
  {
    "label": "run-tests",
    "tool": "bash",
    "args": { "command": "python3 test_fib.py" },
    "pred": "",
    "guard": "last_ok"
  }
]
```

| Field | Type | Meaning |
|---|---|---|
| `label` | string | Stable ID the controller cites in certificates. Unique within a bank. |
| `tool` | string | Tool name as registered in the harness. Unknown tool → node rejected at parse time. |
| `args` | JSON object | **Complete literal arguments.** See §2. |
| `pred` | string | Label of the causal predecessor, `""` for a root node. A node is *pred-complete* when its predecessor has been executed (or, for roots, when a tool receipt has landed). |
| `guard` | string | Observation-only predicate. See §3. Defaults to `last_ok`. |

### Node semantics

- **Single-fire.** A node executes at most once per episode; execution moves
  its label to `done`.
- **Failure drops dependents.** When a node's execution fails, all nodes
  whose `pred` transitively points at it are discarded.
- **Replacement on fallback.** When the controller abstains and control
  returns to the model, the model re-authors the remaining bank. Executed
  history (`done`) is preserved across replacement.

## 2. Literal closure (hard requirement)

Every string value inside `args` must be concrete at declaration time. The
sole rejected placeholder shapes are strings that are entirely a
`${...}` or `{{...}}` token (whole-string match after trim; nested
structures are walked recursively). The check runs at parse time **and**
again at admission (before every claim).

Rationale: a decision is only reusable if it was fully decided. A
placeholder converts "stored decision" into "stored guess", which defeats
the certificate (§5) — you would be certifying a guess, not a decision.

## 3. Guard vocabulary

Guards consume **observed receipts only**. The harness feeds the bank one
receipt per tool execution: `(tool_name, ok, output_snippet)`.

| Guard | Passes when |
|---|---|
| `last_ok` | The last observed receipt had `ok == true` |
| `file_exists:<path>` | The path exists (relative paths resolve against the harness workspace) |
| `none` | Always (ordering-only edges) |

**Unknown guard → defer to the model.** Never guess. The cost of a wrong
defer is one model call; the cost of a guessed guard is executing an action
the model never sanctioned.

Extending the vocabulary is allowed (e.g. `exit_code:0`, `output_contains:x`)
as long as new guards remain observation-only: they may read receipts and
filesystem state, never model output or predictions.

## 4. Authoring channels

Two supported channels; a harness may implement either or both.

1. **Tool channel** — a `declare_edg`-style tool the model calls with the
   nodes array. Costs one tool round-trip; always parses (arguments arrive
   structured).
2. **Inline channel** — a fenced block in the assistant message:

   ~~~
   ```edg
   [ { "label": "run", "tool": "bash", "args": {"command": "pytest -q"}, "pred": "", "guard": "last_ok" } ]
   ```
   ~~~

   Zero round-trip cost, same validation. **Known limitation (measured):**
   when a model emits both an inline block and tool calls in one streaming
   response, the text block is frequently dropped by stream assembly. Treat
   the inline channel as best-effort; do not build correctness on it.

## 5. Substitution certificates

Every executed substitution mints an append-only certificate:

```
[edg] cert#<seq> label=<label> tool=<tool> guard=<guard> args=<json>
```

The certificate binds three things: the literal action executed, the guard
that admitted it, and the model call that was skipped. A run report must
expose `(certified_bypasses, abstentions)` so any token-savings claim can be
audited against physical model-call counts. **A fast path without a falling
slow-call count is not savings** — that substitution paradox is exactly what
the certificate exists to catch.

## 6. Controller hook point

The controller runs inside the harness's agent loop at exactly one place:

```
... tool results arrive → receipts observed → [CONTROLLER: while a node is
eligible → execute stored action through the normal tool path (same
registry, same approval gate) → mint certificate → observe new receipt]
→ next model call (skipped iff any substitution fired)
```

Transcript invariant: for every tool execution — substituted or model-called
— the transcript must carry an assistant tool_call/tool_result pair. A
substitution synthesizes the pair (assistant_with_tools with the stored
call, tool_result with the real output) so provider-side message-pairing
checks never break.

## 7. What is intentionally out of scope

- General workflow engines (this is a cache of *decided* actions, not a
  planner).
- Guards that read model output.
- Cross-episode persistence (each episode re-authors; measured win rate did
  not justify persistence complexity).
