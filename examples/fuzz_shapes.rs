//! Fuzz the Rust SDK against hostile 2xx bodies (run with the scratchpad mock server).
//!
//! PASS = the call returns Ok, or returns a typed `WosError`.
//! BUG  = a hostile shape makes a call fail that should have degraded gracefully
//!        (e.g. serde hard-erroring on `null` and dropping an entire batch), or a panic.
//!
//!   cargo run --example fuzz_shapes -- 18777
//!
//! Not part of the published test suite; a scratch harness for release bug hunts.

use wontopos::Client;

const CASES: &[&str] = &[
    "null", "array", "string", "number", "true",
    "mem_null", "mem_str", "mem_obj", "mem_mixed", "results_key",
    "self_null", "self_str", "self_obj", "self_only",
    "lt_null", "lt_str", "lt_mem_null", "lt_mem_str", "st_turns_bad",
    "engram_bad", "stats_null", "get_null",
    "page_bad", "models_bad", "stores_bad", "spk_bad",
    "broken", "empty", "ws",
];

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let port: u16 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(18777);
    let mut ran = 0usize;
    let mut errs: Vec<(String, String, String)> = Vec::new();

    for kase in CASES {
        let base = format!("http://127.0.0.1:{port}/{kase}");
        let mem = Client::with_base_url("wos-test-key-1234567890", &base).with_retries(0);

        macro_rules! probe {
            ($label:expr, $call:expr) => {{
                ran += 1;
                match $call.await {
                    Ok(_) => {}
                    Err(e) => {
                        let msg = format!("{e}");
                        // Record everything; the report distinguishes graceful typed
                        // errors from shape-driven hard failures.
                        errs.push(($label.to_string(), kase.to_string(), msg));
                    }
                }
            }};
        }

        probe!("search", mem.search("q", "u", 10));
        probe!("search_self", mem.search_self("q", "u", 10));
        probe!("recall", mem.recall("q", "u"));
        probe!("recall_with", mem.recall_with("q", "u", serde_json::json!({"form":"memoir","tz":-300})));
        probe!("engram", mem.engram("deep_recall", "q", "u"));
        probe!("engram_with", mem.engram_with("deep_recall", "q", "u", serde_json::json!({"form":"archive"})));
        probe!("history", mem.history("u"));
        probe!("stats", mem.stats("u"));
        probe!("get", mem.get("u", "mem-id"));
        probe!("list_memories", mem.list_memories("u", 50, None));
        probe!("list_all_memories", mem.list_all_memories("u"));
        probe!("list_models", mem.list_models());
        probe!("list_stores", mem.list_stores());
        probe!("list_speakers", mem.list_speakers("u"));
        probe!("add", mem.add("content", "u", serde_json::json!({})));
        probe!("ping", mem.ping());
    }

    println!("\nrust: {ran} calls, {} returned Err", errs.len());

    // Classify. A body that is not an object at all SHOULD be a typed error
    // ("expected a JSON object in the response") — Python/TS do the same, so that
    // is graceful, not a bug. The real defect is a WELL-FORMED object whose
    // memory ARRAY holds a bad element: Python/TS skip/coerce it and return the
    // good memories, while Rust hard-errors and drops the whole batch.
    let mut graceful = 0;
    let mut hard: Vec<&(String, String, String)> = Vec::new();
    for e in &errs {
        if e.2.contains("expected a JSON object in the response") {
            graceful += 1;
        } else {
            hard.push(e);
        }
    }
    println!("rust: {graceful} graceful typed errors (non-object body — correct)");
    println!("rust: {} HARD failures on well-formed objects (BUG candidates):", hard.len());
    for (label, kase, msg) in &hard {
        println!("   {label:18} case={kase:12} -> {msg}");
    }
}
