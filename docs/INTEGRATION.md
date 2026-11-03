# Integrating GDR into your agent loop

The core crate (`gdr-core`) is deliberately harness-agnostic: it owns the
bank, guards, closure validation, and certificates. Your harness owns the
model-call loop. Integration is one hook point plus three plumbing calls.

## The hook point

```
loop {
    response = call_model(messages)           // ① the only place tokens are spent
    execute tool calls                        // ② normal tool path (registry + approval gate)
    for each result: bank.observe(tool, ok, out)   // ③ receipts are facts — always
    if gdr_enabled {
        while let Some((node, cert)) = bank.next_eligible(workspace) {
            let out = execute_via_normal_path(node.tool, node.args)  // same gate!
            eprintln!("{cert}")               // ④ the ledger
            bank.observe(node.tool, out.is_ok(), &out.text)
            if out.failed { bank.record_failure(&node.label) }
            substituted = true
        }
        if !substituted { bank.abstain() }
    }
    messages.push(assistant + tool_results)   // ⑤ transcript invariant below
    if !substituted { continue }              // skipped call = the saved tokens
}
```

## Transcript invariant (the one hard rule)

For every tool execution — model-called or substituted — the transcript must
carry an assistant `tool_call` / `tool_result` pair, or providers that
enforce pairing (and your own debugging self) will break. For a
substitution, synthesize the pair: an assistant message containing exactly
the stored call, and a tool_result with the real output. The certificate
sequence in stderr plus the synthetic pairs make every skipped call
reconstructible from the transcript alone.

## Plumbing

1. **Expose the authoring channel.** Either a `declare_edg`-style tool
   (structured, always parses) or the inline fenced block parsed from
   assistant text (free, but best-effort under streaming — measured in
   EVAL.md). Tool channel is recommended as the source of truth.
2. **Register the receipt feed.** `bank.observe()` after *every* tool
   execution, regardless of the on/off switch — receipts are how guards
   stay honest.
3. **Report the counters.** Surface `bank.stats()` in your run report:
   `(certified_bypasses, abstentions)`. Any token-savings claim must be
   auditable against physical model-call counts — a fast path whose slow
   callbacks did not actually fall is the substitution paradox, not
   savings.

## Gate the switch, keep the instruction

Measurements (EVAL.md) say the authoring *instruction* ("only declare
actions you have already fully decided; think the plan through before the
first tool call") changes model behavior enough to halve exploratory calls
by itself. The mechanism only pays on callback-heavy workloads. Recommended
defaults: instruction always on; mechanism behind a runtime switch, off for
short tasks.

## What you must NOT do

- Do not let guards read model output or predictions (protocol §3).
- Do not admit nodes with placeholder args (protocol §2) — the certificate
  would attest a guess.
- Do not execute substituted actions outside your normal tool path — same
  registry, same approval gate, same sandbox. A substitution skips the
  *model*, never the *safety*.
