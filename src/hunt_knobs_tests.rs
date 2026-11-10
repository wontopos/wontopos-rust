use super::rooms_work_tests::{count, refusal, scripted, Log, KEY};
use super::{Client, LookOpts, MemoryTools, Rooms, ToolFormat};
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
