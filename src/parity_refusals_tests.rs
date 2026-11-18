//! What the Rust client refuses before sending, against the fixture Python and TypeScript share.
use crate::*;
use serde_json::json;

fn said<T: std::fmt::Debug>(r: Result<T, WosError>) -> String {
    match r {
        Err(WosError::Api { message, .. }) => message,
        Err(e) => format!("(other) {e}"),
        Ok(v) => format!("(accepted) {v:?}"),
    }
}

/// Each fixture case as a call here; `None` for the cases listed in `NOT_HERE`.
async fn rust_says(c: &Client, case: &str) -> Option<String> {
    let ids: Vec<String> = vec!["m".into(); 1001];
    let empty: [&str; 0] = [];
    Some(match case {
        "add: blank store" => said(c.add("x", "  ", json!({})).await),
        "add: created_at a plain date" => said(c.add_opts("x", "u", json!({}), &WriteOpts { created_at: Some("2024-03-01".into()), ..Default::default() }).await),
        "add: user_speaker" => said(c.add("x", "u", json!({"user_speaker": "A"})).await),
        "add: unknown metadata type" => said(c.add("x", "u", json!("a")).await),
        "add_turn: both empty" => said(c.add_turn("", "", "u").await),
        "update: no id" => said(c.update("", "x", "u").await),
        "update: empty content" => said(c.update("m1", "", "u").await),
        "search: limit 0" => said(c.search("q", "u", 0).await),
        "search: limit 1000" => said(c.search("q", "u", 1000).await),
        "recall: limit 0" => said(c.recall_opts("q", "u", &RecallOpts { limit: Some(0), ..Default::default() }).await),
        "recall: limit 1000" => said(c.recall_opts("q", "u", &RecallOpts { limit: Some(1000), ..Default::default() }).await),
        "get: no id" => said(c.get("u", "").await),
        "delete: no id" => said(c.delete("u", "").await),
        "delete_all: no store" => said(c.delete_all("").await),
        "delete_store: no store" => said(c.delete_store("").await),
        "list_memories: limit 0" => said(c.list_memories("u", 0, None).await),
        "list_memories: limit 5000" => said(c.list_memories("u", 5000, None).await),
        "add_speaker: empty" => said(c.add_speaker("", "u").await),
        "add_speaker: line break" => said(c.add_speaker("A\nB", "u").await),
        "remove_speaker: empty" => said(c.remove_speaker("", "u").await),
        "relabel_speaker: 1001 ids" => said(c.relabel_speaker("A", "B", "u", Some(&ids)).await),
        "forget_speaker: empty list" => said(c.forget_speaker(empty, "u", false).await),
        "forget_speaker: no store" => said(c.forget_speaker(["A"], "", false).await),
        "by_speaker: empty" => said(c.by_speaker("", "u", None, None, None).await),
        "set_settings: nothing" => said(c.set_settings("u", &SettingsOpts::default()).await),
        "open_work: empty question" => said(c.open_work("", "u", &LookOpts::default()).await),
        "open_work: diversity bad" => said(c.open_work("q", "u", &LookOpts { diversity: Some("lots".into()), ..Default::default() }).await),
        "open_work: max_images 99" => said(c.open_work("q", "u", &LookOpts { max_images: Some(99), ..Default::default() }).await),
        "work_search: no work" => said(c.work_search("", "q", "u", &LookOpts::default()).await),
        "work_deeper: no work" => said(c.work_deeper("", "u").await),
        "close_work: no work" => said(c.close_work("", "u", None).await),
        "revisions: include bad" => said(c.revisions_page("u", "all", None, None, None).await),
        "usage: days 0" => said(c.usage(0).await),
        "usage: days 400" => said(c.usage(400).await),
        "forget_image: no id" => said(c.forget_image("u", "", false).await),
        "get_image: no id" => said(c.get_image("u", "").await),
        "recall: rooms bad" => said(c.recall_rooms("q", "u", "kitchen", &RecallOpts::default()).await),
        "why: no id" => said(c.why("", "u").await),
        "around: no id" => said(c.around("", "u", &AroundOpts::default()).await),
        "lineage: no id" => said(c.lineage("u", "").await),
        _ => return None,
    })
}

/// The cases whose argument the signatures here take in another shape: a store or a count given as text, a fraction,
/// and a time zone on one recall (set on the client here).
const NOT_HERE: [&str; 4] = ["add: store not text", "search: limit 2.5", "search: limit text", "recall: tz 99"];

#[tokio::test(flavor = "current_thread")]
async fn each_refusal_says_what_python_and_typescript_say() {
    let c = Client::with_base_url("wos-test-xxxxxxxxxxxxxxxx", "http://127.0.0.1:9").with_retries(0);
    let fx: serde_json::Value = serde_json::from_str(include_str!("testdata/parity_refusals.json")).unwrap();
    let cases = fx["cases"].as_array().unwrap();
    assert!(cases.len() >= 40);
    let mut checked = 0;
    for case in cases {
        let name = case["case"].as_str().unwrap();
        let want = case.get("rust_says").or(case.get("python_says")).and_then(|v| v.as_str()).unwrap();
        match rust_says(&c, name).await {
            None => assert!(NOT_HERE.contains(&name), "{name}: no call for it here, and it is not in NOT_HERE"),
            Some(got) => {
                assert_eq!(got, want, "{name}");
                checked += 1;
            }
        }
    }
    assert_eq!(checked, cases.len() - NOT_HERE.iter().filter(|n| cases.iter().any(|c| c["case"] == **n)).count());
}

#[test]
fn python_reads_the_same_bytes_when_it_is_here() {
    let py = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../python/tests/parity_refusals.json");
    if let Ok(bytes) = std::fs::read(py) {
        assert_eq!(bytes, include_bytes!("testdata/parity_refusals.json"));
    }
}
