use super::rooms_work_tests::{count, refusal, scripted, Log, KEY};
use super::{Client, LookOpts, MemoryTools, RecallOpts, Rooms, ToolFormat};
use serde_json::json;

const OPEN: &str =
    r#"{"work_id":"w-1","steps_left":5,"deeper_left":3,"memories":[{"n":1,"id":"a","content":"Theo visits Friday."}]}"#;
const DEEPER: &str = r#"{"work_id":"w-1","memories":[{"n":2,"id":"b","content":"Milo brings the cake."}],"deeper_left":2}"#;
const SEARCH: &str = r#"{"work_id":"w-1","memories":[],"none":true,"steps_left":4}"#;
const PATHS: [&str; 3] = ["/api/v1/memory/work/open", "/api/v1/memory/work/deeper", "/api/v1/memory/work/search"];
const PLAIN: &str = r#"{"user_id":"u","question":"what did we plan"}"#;
const OTHERS: &str = r#""own_words":false"#;
const DIVERSITIES: [&str; 4] = ["none", "mmr", "temporal", "full"];

fn plus(parts: &[&str]) -> String {
    let mut out = PLAIN[..PLAIN.len() - 1].to_string();
    for p in parts {
        out.push(',');
        out.push_str(p);
    }
    out.push('}');
    out
}

fn tools(base: &str) -> MemoryTools {
    Client::with_base_url(KEY, base).with_model("scroll-2").with_tz(9.0).unwrap().memory_tools_preset("u", None, "S-2").unwrap()
}

fn sent(seen: &Log) -> Vec<String> {
    seen.lock().unwrap().iter().map(|s| s.body.to_string()).collect()
}

async fn turn(make: impl FnOnce(&str) -> MemoryTools) -> Vec<String> {
    let (base, seen) = scripted(vec![(200, OPEN), (200, DEEPER), (200, SEARCH)]);
    let mem = make(&base);
    mem.begin("what did we plan").await.unwrap();
    mem.run("memory_deeper", &json!({})).await.unwrap();
    mem.run("memory_search", &json!({"query": "the cake"})).await.unwrap();
    let paths: Vec<String> = seen.lock().unwrap().iter().map(|s| s.path.clone()).collect();
    assert_eq!(paths, PATHS);
    sent(&seen)
}

#[tokio::test(flavor = "current_thread")]
async fn defaults_open_the_work_with_the_same_bytes_as_before() {
    let plain = turn(tools).await;
    assert_eq!(plain[0], PLAIN);
    assert_eq!(turn(|b| tools(b).with_own_words(true)).await, plain);
    let (base, seen) = scripted(vec![(200, OPEN)]);
    let c = Client::with_base_url(KEY, &base).with_model("scroll-2").with_tz(9.0).unwrap();
    c.memory_tools_preset("u", Some(Rooms::All), "S-2").unwrap().with_images(true).begin("what did we plan").await.unwrap();
    assert_eq!(
        sent(&seen)[0],
        r#"{"user_id":"u","question":"what did we plan","rooms":"all","tz_offset_minutes":540,"max_images":3}"#
    );
}

#[tokio::test(flavor = "current_thread")]
async fn own_words_off_is_sent_as_own_words_false_and_later_looks_send_nothing_new() {
    let plain = turn(tools).await;
    let off = turn(|b| tools(b).with_own_words(false)).await;
    assert_eq!(off[0], plus(&[OTHERS]));
    let body: serde_json::Value = serde_json::from_str(&off[0]).unwrap();
    assert_eq!(body["own_words"], json!(false));
    assert!(body.get("filters").is_none());
    assert_eq!(off[1..], plain[1..]);
}

#[tokio::test(flavor = "current_thread")]
async fn each_diversity_goes_through_and_anything_else_is_refused_before_a_call() {
    let plain = turn(tools).await;
    for d in DIVERSITIES {
        let got = turn(|b| tools(b).with_diversity(d).unwrap()).await;
        assert_eq!(got[0], plus(&[&format!(r#""diversity":"{d}""#)]), "{d}");
        assert_eq!(got[1..], plain[1..], "{d}");
    }
    let (base, seen) = scripted(vec![]);
    for bad in ["nope", "MMR", " mmr", "mmr ", ""] {
        assert_eq!(
            refusal(tools(&base).with_diversity(bad)),
            format!(r#"diversity must be "none", "mmr", "temporal" or "full", got {}."#, json!(bad)),
            "{bad:?}"
        );
    }
    assert_eq!(count(&seen), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn both_together() {
    let plain = turn(tools).await;
    let both = turn(|b| tools(b).with_own_words(false).with_diversity("temporal").unwrap()).await;
    assert_eq!(both[0], plus(&[OTHERS, r#""diversity":"temporal""#]));
    assert_eq!(both[1..], plain[1..]);
}

#[tokio::test(flavor = "current_thread")]
async fn open_work_takes_both_as_the_memory_tools_do() {
    let (base, seen) = scripted(vec![(200, OPEN), (200, OPEN), (200, OPEN)]);
    let mem = Client::with_base_url(KEY, &base).with_model("scroll-2");
    let set = LookOpts { own_words: Some(false), diversity: Some("temporal".into()), ..Default::default() };
    mem.open_work("what did we plan", "u", &set).await.unwrap();
    mem.open_work("what did we plan", "u", &LookOpts { own_words: Some(true), ..Default::default() }).await.unwrap();
    mem.open_work("what did we plan", "u", &LookOpts::default()).await.unwrap();
    assert_eq!(sent(&seen), [plus(&[OTHERS, r#""diversity":"temporal""#]), PLAIN.to_string(), PLAIN.to_string()]);
    let (base, seen) = scripted(vec![]);
    let mem = Client::with_base_url(KEY, &base).with_model("scroll-2");
    for bad in ["nope", "MMR", " mmr", "mmr ", ""] {
        let opts = LookOpts { diversity: Some(bad.into()), ..Default::default() };
        assert_eq!(
            refusal(mem.open_work("q", "u", &opts).await),
            format!(r#"diversity must be "none", "mmr", "temporal" or "full", got {}."#, json!(bad)),
            "{bad:?}"
        );
    }
    for (opts, name) in [
        (LookOpts { own_words: Some(true), ..Default::default() }, "own_words"),
        (LookOpts { diversity: Some("full".into()), ..Default::default() }, "diversity"),
    ] {
        assert_eq!(
            refusal(mem.work_search("w", "q", "u", &opts).await),
            format!("{name} goes on open_work: the work keeps it for every look after.")
        );
    }
    assert_eq!(count(&seen), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_with_copy_and_tools_on_another_models_client_keep_them() {
    let want = plus(&[r#""max_images":3"#, OTHERS, r#""diversity":"mmr""#]);
    let (base, seen) = scripted(vec![(200, OPEN), (200, OPEN), (200, OPEN)]);
    let first = tools(&base).with_own_words(false).with_diversity("mmr").unwrap().with_dates(true).with_usage(true);
    first.with_images(true).begin("what did we plan").await.unwrap();
    let then = tools(&base).with_images(true).with_diversity("mmr").unwrap().with_seen(true).with_own_words(false);
    then.begin("what did we plan").await.unwrap();
    let t2 = Client::with_base_url(KEY, &base).with_model("tablet-2").with_tz(9.0).unwrap().with_user("x");
    let on_t2 = t2.memory_tools_preset("u", None, "S-2").unwrap().with_own_words(false).with_diversity("mmr").unwrap();
    on_t2.with_images(true).begin("what did we plan").await.unwrap();
    assert_eq!(sent(&seen), [want.clone(), want.clone(), want]);
}

#[test]
fn nothing_the_model_sees_changes() {
    let (base, seen) = scripted(vec![]);
    let flagged = |on: bool| tools(&base).with_speakers(on).with_dates(on).with_images(on).with_seen(on).with_usage(on);
    for on in [false, true] {
        let plain = flagged(on);
        for set in [
            flagged(on).with_own_words(false),
            flagged(on).with_diversity("full").unwrap(),
            flagged(on).with_own_words(false).with_diversity("none").unwrap(),
        ] {
            assert_eq!(set.note(), plain.note());
            for format in [ToolFormat::Anthropic, ToolFormat::OpenAi] {
                assert_eq!(set.definitions(format), plain.definitions(format));
            }
        }
    }
    assert_eq!(count(&seen), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn recall_asks_for_50_lines_of_the_recent_conversation_where_the_model_keeps_a_window() {
    let (base, seen) = scripted(vec![]);
    let mem = Client::with_base_url(KEY, &base).with_model("scroll-2");
    let lines = |n: usize| RecallOpts { short_term_limit: Some(n), ..Default::default() };
    mem.recall_opts("q", "u", &RecallOpts::default()).await.unwrap();
    mem.recall_opts("q", "u", &lines(0)).await.unwrap();
    mem.recall_opts("q", "u", &lines(100)).await.unwrap();
    mem.with_model("tablet-2").recall_opts("q", "u", &RecallOpts::default()).await.unwrap();
    let plain = Client::with_base_url(KEY, &base);
    plain.recall_opts("q", "u", &RecallOpts::default()).await.unwrap();
    plain.with_model("scroll-2").recall_opts("q", "u", &RecallOpts::default()).await.unwrap();
    plain.with_model("scroll-2").recall_opts("q", "u", &lines(7)).await.unwrap();
    // The plain call and the one that takes the body's fields as they are ask the same.
    mem.recall("q", "u").await.unwrap();
    mem.recall_with("q", "u", json!({})).await.unwrap();
    mem.recall_with("q", "u", json!({"short_term_limit": null})).await.unwrap();
    mem.recall_with("q", "u", json!({"short_term_limit": 3})).await.unwrap();
    plain.recall("q", "u").await.unwrap();
    plain.recall_with("q", "u", json!({"short_term_limit": 3})).await.unwrap();
    plain.recall_with("q", "u", json!({"short_term_limit": null})).await.unwrap();
    let asked: Vec<serde_json::Value> = seen.lock().unwrap().iter().map(|s| s.body["short_term_limit"].clone()).collect();
    assert_eq!(json!(asked), json!([50, 0, 100, null, null, 50, 7, 50, 50, 50, 3, null, 3, null]));
    // A number not given is no key in the body, a null among them.
    assert!(!seen.lock().unwrap().iter().any(|s| matches!(s.body.get("short_term_limit"), Some(v) if v.is_null())));
    for bad in [101usize, 500] {
        let says = format!("short_term_limit must be an integer between 0 and 100, got {bad}.");
        assert_eq!(refusal(mem.recall_opts("q", "u", &lines(bad)).await), says);
        assert_eq!(refusal(mem.recall_with("q", "u", json!({"short_term_limit": bad})).await), says);
        assert_eq!(refusal(plain.recall_with("q", "u", json!({"short_term_limit": bad})).await), says);
    }
    for bad in [json!("50"), json!(1.5), json!(-1), json!(true)] {
        let m = refusal(mem.recall_with("q", "u", json!({"short_term_limit": bad})).await);
        assert!(m.starts_with("short_term_limit must be an integer between 0 and 100, got "), "{m}");
    }
    assert_eq!(count(&seen), 14);
}

#[tokio::test(flavor = "current_thread")]
async fn memory_recent_shows_the_newest_50_lines_or_as_many_as_recent_turns_says() {
    let turns: Vec<serde_json::Value> = (0..120)
        .map(|i| {
            json!({"role": if i % 2 == 0 { "user" } else { "assistant" }, "content": format!("line {i}"),
                   "timestamp": format!("2026-10-20T09:{:02}:{:02}Z", i / 60, i % 60)})
        })
        .collect();
    let window = json!({"turns": turns}).to_string();
    let (base, seen) = scripted(vec![(200, window.as_str()); 4]);
    let seeing = || tools(&base).with_seen(true);
    let shown = |text: String| -> Vec<String> { text.split('\n').skip(1).map(str::to_string).collect() };
    let lines = shown(seeing().run("memory_recent", &json!({})).await.unwrap());
    assert_eq!((lines.len(), lines[0].as_str(), lines[49].as_str()), (50, "#1 you: line 119", "#50 user: line 70"));
    let lines = shown(seeing().with_recent_turns(100).unwrap().run("memory_recent", &json!({})).await.unwrap());
    assert_eq!((lines.len(), lines[99].as_str()), (100, "#100 user: line 20"));
    let lines = shown(seeing().with_recent_turns(1).unwrap().run("memory_recent", &json!({})).await.unwrap());
    assert_eq!(lines, ["#1 you: line 119"]);
    let off = seeing().with_recent_turns(0).unwrap();
    assert_eq!(off.run("memory_recent", &json!({})).await.unwrap(), "The recent conversation is not shown here.");
    assert_eq!(off.run("memory_seen", &json!({"n": 1})).await.unwrap(), "Refused: #1 is not a line memory_recent gave.");
    assert_eq!(count(&seen), 3, "with no recent turns the window is not read");
    let few = seeing().with_recent_turns(3).unwrap();
    assert_eq!(few.run("memory_recent", &json!({})).await.unwrap().split('\n').count(), 4);
    assert_eq!(few.run("memory_seen", &json!({"n": 4})).await.unwrap(), "Refused: #4 is not a line memory_recent gave.");
    assert_eq!(count(&seen), 4, "a line that was not shown is not looked up");
    assert_eq!(
        refusal(seeing().with_recent_turns(101).map(|_| ())),
        "recent_turns must be an integer between 0 and 100, got 101."
    );
}
