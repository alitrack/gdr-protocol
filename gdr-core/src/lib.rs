//! gdr-core: Guarded Decision Reuse — the harness-neutral core extracted
//! from an independent implementation of the HarnessMM mechanism
//! (arXiv:2610.32813).
//!
//! The model authors a literal action bank alongside its current tool
//! call; after tool receipts arrive, the harness checks each node's guard
//! against *observed* receipts only. An eligible node executes through the
//! harness's normal tool path and **replaces the next model call**,
//! minting a substitution certificate. New semantic decisions, unsupported
//! evidence, or abstention return control to the model.
//!
//! Three safety properties (see docs/PROTOCOL.md):
//! 1. Literal closure — args must be concrete JSON at declaration time.
//! 2. Observation-only guards — never model predictions; unknown defers.
//! 3. Append-only certificates — every skipped call is auditable.

use serde_json::Value;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// A single stored decision node (PROTOCOL §1).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Node {
    /// Stable ID cited in certificates. Unique within a bank.
    pub label: String,
    /// Tool name as registered in the harness.
    pub tool: String,
    /// Complete literal arguments (closure enforced, PROTOCOL §2).
    pub args: Value,
    /// Label of the causal predecessor ("" = root).
    pub pred: String,
    /// Observation-only guard (PROTOCOL §3). Defaults to "last_ok".
    pub guard: String,
}

impl Node {
    /// Literal closure: string values must not be sole `${...}` / `{{...}}`
    /// placeholder tokens; nested structures are walked.
    pub fn is_literal(v: &Value) -> bool {
        match v {
            Value::String(s) => {
                let t = s.trim();
                !(t.starts_with("${") && t.ends_with('}'))
                    && !(t.starts_with("{{") && t.ends_with("}}"))
            }
            Value::Array(a) => a.iter().all(Self::is_literal),
            Value::Object(m) => m.values().all(Self::is_literal),
            _ => true,
        }
    }

    fn closed(&self) -> bool {
        Self::is_literal(&self.args)
    }
}

/// Substitution certificate (PROTOCOL §5): binds the executed literal
/// action to the omitted model call. The `[edg] cert#N` line IS the ledger.
#[derive(Debug, Clone, PartialEq)]
pub struct Certificate {
    pub seq: u64,
    pub label: String,
    pub tool: String,
    pub args: String,
    pub guard: String,
}

impl std::fmt::Display for Certificate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[edg] cert#{} label={} tool={} guard={} args={}",
            self.seq, self.label, self.tool, self.guard, self.args
        )
    }
}

#[derive(Default)]
struct BankInner {
    nodes: Vec<Node>,
    done: Vec<String>,
    failed: Vec<String>,
    last_receipt: Option<(String, bool, String)>,
    cert_seq: u64,
}

/// The shared bank + counters the harness controller reads after tool turns.
/// `Default` derives an empty bank; share it via `Arc`.
#[derive(Default)]
pub struct Bank {
    inner: RwLock<BankInner>,
    /// Certified callback substitutions performed.
    pub bypasses: AtomicU64,
    /// Times no eligible node was found → fell back to the model.
    pub abstentions: AtomicU64,
}

impl Bank {
    pub fn new_shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Replace the unresolved remainder with a freshly authored bank.
    /// Executed history (`done`) is preserved across replacement.
    pub fn replace(&self, nodes: Vec<Node>) -> usize {
        let mut b = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let keep_done = std::mem::take(&mut b.done);
        b.nodes = nodes;
        b.failed.clear();
        b.done = keep_done;
        b.nodes.len()
    }

    /// Record an observed tool receipt. Called by the harness after EVERY
    /// tool execution — receipts are facts, independent of the switch.
    pub fn observe(&self, tool: &str, ok: bool, output: &str) {
        let mut b = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let snippet: String = output.chars().take(200).collect();
        b.last_receipt = Some((tool.to_string(), ok, snippet));
    }

    /// Guard evaluation consumes observed receipts only. Returns the next
    /// eligible node or None (caller counts an abstention).
    fn eligible(&self, workspace: Option<&Path>) -> Option<Node> {
        let b = self.inner.read().unwrap_or_else(|e| e.into_inner());
        if b.nodes.is_empty() {
            return None;
        }
        let (_tool, last_ok, _snippet) = b.last_receipt.as_ref()?;
        for n in &b.nodes {
            if b.done.contains(&n.label) || b.failed.contains(&n.label) {
                continue;
            }
            if !n.pred.is_empty() && !b.done.contains(&n.pred) {
                continue;
            }
            if !n.closed() {
                continue;
            }
            let guard_ok = match n.guard.as_str() {
                "last_ok" => *last_ok,
                g if g.starts_with("file_exists:") => {
                    let p = g.trim_start_matches("file_exists:");
                    let full = match workspace {
                        Some(ws) if !p.starts_with('/') => ws.join(p),
                        _ => std::path::PathBuf::from(p),
                    };
                    full.exists()
                }
                "none" => true,
                _ => false, // unknown guard → defer to the model
            };
            if guard_ok {
                return Some(n.clone());
            }
        }
        None
    }

    /// Controller entry: find the next eligible node without claiming it.
    pub fn peek_eligible(&self, workspace: Option<&Path>) -> Option<Node> {
        self.eligible(workspace)
    }

    /// Claim a node: mark executed and mint the substitution certificate.
    /// The harness then executes `node` through its normal tool path.
    pub fn claim(&self, node: &Node) -> Certificate {
        let mut b = self.inner.write().unwrap_or_else(|e| e.into_inner());
        b.cert_seq += 1;
        b.done.push(node.label.clone());
        self.bypasses.fetch_add(1, Ordering::Relaxed);
        Certificate {
            seq: b.cert_seq,
            label: node.label.clone(),
            tool: node.tool.clone(),
            args: node.args.to_string(),
            guard: node.guard.clone(),
        }
    }

    /// Convenience: peek + claim in one call.
    pub fn next_eligible(&self, workspace: Option<&Path>) -> Option<(Node, Certificate)> {
        let n = self.eligible(workspace)?;
        let cert = self.claim(&n);
        Some((n, cert))
    }

    /// Record that a node's execution failed; drops its dependents.
    pub fn record_failure(&self, label: &str) {
        let mut b = self.inner.write().unwrap_or_else(|e| e.into_inner());
        b.failed.push(label.to_string());
        b.nodes.retain(|n| n.pred != label);
    }

    /// Run report counters: (certified bypasses, abstentions).
    pub fn stats(&self) -> (u64, u64) {
        (
            self.bypasses.load(Ordering::Relaxed),
            self.abstentions.load(Ordering::Relaxed),
        )
    }

    /// Count an abstention (controller found nothing eligible).
    pub fn abstain(&self) {
        self.abstentions.fetch_add(1, Ordering::Relaxed);
    }
}

/// Validate a raw JSON nodes array (PROTOCOL §1–2). Shared by the tool
/// channel and the inline parser.
pub fn validate_nodes(nodes: &[Value]) -> Result<Vec<Node>, String> {
    let mut out = Vec::with_capacity(nodes.len());
    for (i, raw) in nodes.iter().enumerate() {
        let label = raw
            .get("label")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("node {i}: missing label"))?
            .to_string();
        let tool = raw
            .get("tool")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("node {i}: missing tool"))?
            .to_string();
        let args = raw
            .get("args")
            .cloned()
            .ok_or_else(|| format!("node {i}: missing args"))?;
        let pred = raw
            .get("pred")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let guard = raw
            .get("guard")
            .and_then(Value::as_str)
            .unwrap_or("last_ok")
            .to_string();
        let node = Node {
            label,
            tool,
            args,
            pred,
            guard,
        };
        if !node.closed() {
            return Err(format!(
                "node {} ('{}'): args contain a placeholder — literal closure violated",
                i, node.label
            ));
        }
        out.push(node);
    }
    Ok(out)
}

/// Parse an inline ```edg fenced block from assistant text (PROTOCOL §4,
/// inline channel). Best-effort by design — see the protocol's measured
/// limitation on streaming responses.
pub fn parse_inline(text: &str) -> Option<Vec<Node>> {
    let mut in_block = false;
    let mut body = String::new();
    for line in text.lines() {
        let t = line.trim();
        if !in_block {
            if t == "```edg" {
                in_block = true;
            }
            continue;
        }
        if t == "```" {
            break;
        }
        body.push_str(line);
        body.push('\n');
    }
    if !in_block {
        return None;
    }
    let v: Value = serde_json::from_str(body.trim()).ok()?;
    let arr = match v {
        Value::Array(a) => a,
        _ => return None,
    };
    let nodes = validate_nodes(&arr).ok()?;
    if nodes.is_empty() {
        return None;
    }
    Some(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(label: &str, tool: &str, args: Value, pred: &str, guard: &str) -> Value {
        json!({ "label": label, "tool": tool, "args": args, "pred": pred, "guard": guard })
    }

    #[test]
    fn closure_rejects_placeholders() {
        let bad = vec![node(
            "run",
            "bash",
            json!({"command": "${next_cmd}"}),
            "",
            "last_ok",
        )];
        assert!(validate_nodes(&bad).is_err());

        let good = vec![node(
            "run",
            "bash",
            json!({"command": "python3 test_fib.py"}),
            "",
            "last_ok",
        )];
        assert!(validate_nodes(&good).is_ok());
    }

    #[test]
    fn closure_allows_embedded_dollars() {
        // Only whole-string placeholder tokens are rejected.
        let ok = vec![node(
            "sh",
            "bash",
            json!({"command": "echo $HOME is fine"}),
            "",
            "none",
        )];
        assert!(validate_nodes(&ok).is_ok());
    }

    #[test]
    fn guard_requires_receipt() {
        let bank = Bank::new_shared();
        bank.replace(vec![Node {
            label: "run".into(),
            tool: "bash".into(),
            args: json!({"command": "true"}),
            pred: "".into(),
            guard: "last_ok".into(),
        }]);
        assert!(bank.next_eligible(None).is_none());
        bank.observe("write_file", true, "wrote 20 bytes");
        let (n, cert) = bank.next_eligible(None).unwrap();
        assert_eq!(n.label, "run");
        assert_eq!(cert.seq, 1);
        // single-fire
        assert!(bank.peek_eligible(None).is_none());
        assert_eq!(bank.stats().0, 1);
    }

    #[test]
    fn failed_receipt_fails_last_ok() {
        let bank = Bank::new_shared();
        bank.replace(vec![Node {
            label: "run".into(),
            tool: "bash".into(),
            args: json!({"command": "true"}),
            pred: "".into(),
            guard: "last_ok".into(),
        }]);
        bank.observe("write_file", false, "error: disk full");
        assert!(bank.next_eligible(None).is_none());
        bank.abstain();
        assert_eq!(bank.stats().1, 1);
    }

    #[test]
    fn chained_nodes_execute_in_order() {
        let bank = Bank::new_shared();
        bank.replace(vec![
            Node {
                label: "step1".into(),
                tool: "bash".into(),
                args: json!({"command": "echo a"}),
                pred: "".into(),
                guard: "last_ok".into(),
            },
            Node {
                label: "step2".into(),
                tool: "bash".into(),
                args: json!({"command": "echo b"}),
                pred: "step1".into(),
                guard: "last_ok".into(),
            },
        ]);
        bank.observe("write_file", true, "ok");
        let (n1, _) = bank.next_eligible(None).unwrap();
        assert_eq!(n1.label, "step1");
        bank.observe("bash", true, "a");
        let (n2, _) = bank.next_eligible(None).unwrap();
        assert_eq!(n2.label, "step2");
        assert_eq!(bank.stats().0, 2);
    }

    #[test]
    fn failed_pred_drops_dependents() {
        let bank = Bank::new_shared();
        bank.replace(vec![Node {
            label: "verify".into(),
            tool: "bash".into(),
            args: json!({"command": "cat out.txt"}),
            pred: "gen".into(),
            guard: "last_ok".into(),
        }]);
        bank.record_failure("gen");
        bank.observe("bash", false, "exit 1");
        assert!(bank.next_eligible(None).is_none());
    }

    #[test]
    fn file_exists_guard_uses_workspace() {
        let bank = Bank::new_shared();
        let dir = std::env::temp_dir();
        bank.replace(vec![Node {
            label: "check".into(),
            tool: "bash".into(),
            args: json!({"command": "true"}),
            pred: "".into(),
            guard: format!("file_exists:{}", dir.display()),
        }]);
        bank.observe("write_file", true, "ok");
        assert!(bank.next_eligible(Some(&dir)).is_some());
    }

    #[test]
    fn unknown_guard_defers() {
        let bank = Bank::new_shared();
        bank.replace(vec![Node {
            label: "x".into(),
            tool: "bash".into(),
            args: json!({"command": "true"}),
            pred: "".into(),
            guard: "model_predicts_success".into(),
        }]);
        bank.observe("bash", true, "ok");
        assert!(bank.next_eligible(None).is_none());
    }

    #[test]
    fn replacement_preserves_done() {
        let bank = Bank::new_shared();
        bank.replace(vec![Node {
            label: "a".into(),
            tool: "bash".into(),
            args: json!({"command": "true"}),
            pred: "".into(),
            guard: "last_ok".into(),
        }]);
        bank.observe("bash", true, "ok");
        let _ = bank.next_eligible(None).unwrap();
        // Re-author with a node whose pred is the executed one.
        bank.replace(vec![Node {
            label: "b".into(),
            tool: "bash".into(),
            args: json!({"command": "true"}),
            pred: "a".into(),
            guard: "last_ok".into(),
        }]);
        bank.observe("bash", true, "ok");
        let (n, _) = bank.next_eligible(None).unwrap();
        assert_eq!(n.label, "b");
    }

    #[test]
    fn certificate_display_is_the_ledger_line() {
        let c = Certificate {
            seq: 3,
            label: "run".into(),
            tool: "bash".into(),
            args: "{\"command\":\"true\"}".into(),
            guard: "last_ok".into(),
        };
        let s = c.to_string();
        assert!(s.starts_with("[edg] cert#3"));
        assert!(s.contains("label=run"));
    }

    #[test]
    fn inline_parse_extracts_and_validates() {
        let text = "writing now\n```edg\n[{\"label\":\"run\",\"tool\":\"bash\",\"args\":{\"command\":\"python3 t.py\"},\"pred\":\"\",\"guard\":\"last_ok\"}]\n```\ndone";
        let nodes = parse_inline(text).expect("block should parse");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].tool, "bash");
        let bad = "```edg\n[{\"label\":\"r\",\"tool\":\"bash\",\"args\":{\"command\":\"${cmd}\"}}]\n```";
        assert!(parse_inline(bad).is_none());
        assert!(parse_inline("no block here").is_none());
    }

    #[test]
    fn empty_bank_never_eligible() {
        let bank = Bank::new_shared();
        bank.observe("bash", true, "ok");
        assert!(bank.next_eligible(None).is_none());
    }
}
