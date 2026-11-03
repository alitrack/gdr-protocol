//! Minimal harness demo: a 30-line agent loop around `gdr_core`.
//!
//! No LLM required — `call_model` is a stand-in counter. The demo shows the
//! whole protocol working end to end: author a bank, observe receipts, let
//! the controller substitute already-decided callbacks, and read the audit
//! numbers at the end.
//!
//! Run: cargo run -p gdr-core --example quickstart

use gdr_core::Bank;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

static MODEL_CALLS: AtomicU64 = AtomicU64::new(0);

/// Stand-in for a real provider call: increments the physical-call counter.
fn call_model() -> String {
    MODEL_CALLS.fetch_add(1, Ordering::Relaxed);
    "what should I do next?".into()
}

/// Stand-in tool executor (in a real harness: registry + approval gate).
fn execute_tool(tool: &str, args: &serde_json::Value) -> (bool, String) {
    match tool {
        "bash" => {
            let cmd = args["command"].as_str().unwrap_or_default();
            // Pretend `python3 test_fib.py` runs the test suite successfully.
            let ok = !cmd.is_empty();
            (ok, format!("ran `{cmd}` -> PASS"))
        }
        _ => (false, format!("unknown tool {tool}")),
    }
}

fn main() {
    let bank = Bank::new_shared();

    // 1. The "model" authors a bank alongside its current tool call:
    //    it already decided: after the write lands, run the tests.
    let nodes = gdr_core::validate_nodes(&[json!({
        "label": "run-tests",
        "tool": "bash",
        "args": { "command": "python3 test_fib.py" },   // literal — no placeholders
        "pred": "",
        "guard": "last_ok"
    })])
    .expect("valid bank");
    bank.replace(nodes);

    // 2. The model's current turn: one real call + one real tool execution.
    call_model();
    let (ok, out) = execute_tool("bash", &json!({"command": "tee fib.py"}));
    bank.observe("bash", ok, &out); // receipts are facts — always observed

    // 3. Controller: guard hit -> stored action runs, model call skipped.
    let mut substituted = false;
    while let Some((node, cert)) = bank.next_eligible(None) {
        let (ok, out) = execute_tool(&node.tool, &node.args); // same tool path
        println!("{cert}"); // the append-only ledger line
        bank.observe(&node.tool, ok, &out);
        if !ok {
            bank.record_failure(&node.label);
        }
        substituted = true;
    }
    if !substituted {
        bank.abstain();
    }

    // 4. No pending decisions left -> loop would continue with the model.
    if !substituted {
        call_model();
    }

    // 5. Audit numbers: every claim checks against physical calls.
    let (bypasses, abstentions) = bank.stats();
    let calls = MODEL_CALLS.load(Ordering::Relaxed);
    println!("---");
    println!("model calls:    {calls} (2 decisions, 1 substituted)");
    println!("cert bypasses:  {bypasses}");
    println!("abstentions:    {abstentions}");
    assert_eq!((calls, bypasses, abstentions), (1, 1, 0));
    println!("OK: one model call skipped, certificate minted, audit clean.");
}
