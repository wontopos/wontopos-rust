use super::rooms_work_tests::{count, refusal, scripted, Log, KEY};
use super::tests::{http, mock_server_recording};
use super::{Client, MemoryTools, ToolFormat};
use serde_json::json;

const Q: &str = "what did we plan";
const OPEN: &str =
    r#"{"work_id":"w-1","steps_left":5,"deeper_left":3,"memories":[{"n":1,"id":"a","content":"Theo visits Friday."}]}"#;
const DEEPER: &str = r#"{"work_id":"w-1","memories":[{"n":2,"id":"b","content":"Milo brings the cake."}],"deeper_left":2}"#;
const SEARCH: &str = r#"{"work_id":"w-1","memories":[],"none":true,"steps_left":4}"#;
const HISTORY: &str = "/api/v1/memory/history";
const OPENED: &str = "/api/v1/memory/work/open";
const WORK_PATHS: [&str; 3] = ["/api/v1/memory/work/open", "/api/v1/memory/work/deeper", "/api/v1/memory/work/search"];
const RESULTS: [&str; 3] = [
    "[01] Theo visits Friday.\n(steps left 5 \u{b7} deeper left 3)",
    "[02] Milo brings the cake.\n(deeper left 2)",
    "Nothing new for that.\n(steps left 4)",
];
const PLAIN: &str = r#"{"user_id":"u","question":"what did we plan"}"#;
const LATER: [&str; 2] = [r#"{"user_id":"u","work_id":"w-1"}"#, r#"{"user_id":"u","work_id":"w-1","query":"the cake"}"#];
const OTHERS: &str = r#","own_words":false"#;
const FULL: &str = r#","diversity":"full""#;

fn iso(ms: i64) -> String {
    format!("{}:{:02}.{:03}Z", super::iso_minute(ms), ms.rem_euclid(60_000) / 1000, ms.rem_euclid(1000))
}

fn history(lines: &[(&str, i64)]) -> String {
    let now = super::now_ms();
    let turns: Vec<serde_json::Value> = lines
        .iter()
        .enumerate()
        .map(|(i, (content, minutes))| {
            json!({"role": if i % 2 == 1 { "assistant" } else { "user" }, "content": content, "timestamp": iso(now - minutes * 60_000)})
        })
        .collect();
    json!({ "turns": turns }).to_string()
}

fn asked(question: &str, tail: &str) -> String {
    format!(r#"{{"user_id":"u","question":{}{tail}}}"#, json!(question))
}

fn live(question: &str) -> String {
    asked(question, &format!("{OTHERS}{FULL}"))
}

fn client(base: &str, model: &str) -> Client {
    Client::with_base_url(KEY, base).with_model(model).with_tz(9.0).unwrap()
}

fn paths(seen: &Log) -> Vec<String> {
    seen.lock().unwrap().iter().map(|s| s.path.clone()).collect()
}

fn bodies(seen: &Log) -> Vec<String> {
    seen.lock().unwrap().iter().map(|s| s.body.to_string()).collect()
}

async fn turn(mem: &MemoryTools) -> Vec<String> {
    vec![
        mem.begin(Q).await.unwrap(),
        mem.run("memory_deeper", &json!({})).await.unwrap(),
        mem.run("memory_search", &json!({"query": "the cake"})).await.unwrap(),
    ]
}

async fn first_look(lines: &[(&str, i64)], question: &str, make: impl FnOnce(MemoryTools) -> MemoryTools) -> String {
    let said = history(lines);
    let (base, seen) = scripted(vec![(200, &said), (200, OPEN)]);
    let mem = make(client(&base, "scroll-2").memory_tools("u", None).unwrap());
    assert_eq!(mem.begin(question).await.unwrap(), RESULTS[0]);
    assert_eq!(paths(&seen), [HISTORY, OPENED]);
    assert_eq!(bodies(&seen)[0], r#"{"user_id":"u"}"#);
    bodies(&seen)[1].clone()
}

#[tokio::test(flavor = "current_thread")]
async fn s2_named_sends_and_answers_byte_for_byte_as_before() {
    for model in ["scroll-2", "tablet-2"] {
        let (base, seen) = scripted(vec![(200, OPEN), (200, DEEPER), (200, SEARCH)]);
        let mem = client(&base, model).memory_tools_preset("u", None, "S-2").unwrap();
        assert_eq!(mem.preset(), "S-2");
        assert_eq!(turn(&mem).await, RESULTS);
        assert_eq!(paths(&seen), WORK_PATHS);
        assert_eq!(bodies(&seen), [PLAIN, LATER[0], LATER[1]]);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn no_preset_on_scroll_2_is_s2_live() {
    let (base, seen) = scripted(vec![]);
    assert_eq!(client(&base, "scroll-2").memory_tools("u", None).unwrap().preset(), "S-2-live");
    assert_eq!(client(&base, "tablet-2").memory_tools_preset("u", None, "S-2-live").unwrap().preset(), "S-2-live");
    assert_eq!(count(&seen), 0);
    assert_eq!(first_look(&[], Q, |m| m).await, live(Q));
    let (base, raw) = mock_server_recording(vec![http("200 OK", "", &history(&[])), http("200 OK", "", OPEN)]);
    client(&base, "tablet-2").memory_tools_preset("u", None, "S-2-live").unwrap().begin(Q).await.unwrap();
    let models: Vec<String> = raw
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.to_lowercase().lines().find_map(|l| l.strip_prefix("x-wos-model:")).unwrap_or("").trim().to_string())
        .collect();
    assert_eq!(models, ["scroll-2", "scroll-2"]);
}

#[tokio::test(flavor = "current_thread")]
async fn the_first_look_reads_the_last_two_lines_of_the_last_five_minutes() {
    let cases: [(&[(&str, i64)], String); 6] = [
        (&[], Q.to_string()),
        (&[("we booked the cabin", 1)], format!("we booked the cabin\n{Q}")),
        (&[("we booked the cabin", 2), ("Theo drives up Friday", 1)], format!("we booked the cabin\nTheo drives up Friday\n{Q}")),
        (&[("first", 3), ("second", 2), ("third", 1)], format!("second\nthird\n{Q}")),
        (&[("last week", 60 * 24 * 7), ("too early", 6), ("just now", 4)], format!("just now\n{Q}")),
        (&[("we booked the cabin", 2), ("Theo drives up Friday", 1), (Q, 0)], format!("we booked the cabin\nTheo drives up Friday\n{Q}")),
    ];
    for (lines, want) in cases {
        assert_eq!(first_look(lines, Q, |m| m).await, live(&want), "{lines:?}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_long_line_is_cut_from_its_start_and_a_long_question_goes_whole_alone() {
    let line = format!("start {}", "x".repeat(600));
    let body = first_look(&[("Theo drives up Friday", 2), (line.as_str(), 1)], Q, |m| m).await;
    let sent = serde_json::from_str::<serde_json::Value>(&body).unwrap()["question"].as_str().unwrap().to_string();
    assert_eq!(sent, format!("{}\n{Q}", &line[line.len() - (500 - Q.len() - 1)..]));
    assert_eq!(sent.chars().count(), 500);
    assert_eq!(body, live(&sent));
    let cut = "x".repeat(500 - Q.len() - 2);
    let spaced = format!("start {cut}");
    assert_eq!(first_look(&[(spaced.as_str(), 1)], Q, |m| m).await, live(&format!("{cut}\n{Q}")));
    let question = "y".repeat(600);
    assert_eq!(first_look(&[("we booked the cabin", 1)], &question, |m| m).await, live(&question));
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_recent_read_opens_with_the_question_alone() {
    let (base, seen) = scripted(vec![(500, r#"{"error":"down"}"#), (200, OPEN)]);
    let mem = client(&base, "scroll-2").with_retries(0).memory_tools("u", None).unwrap();
    assert_eq!(mem.begin(Q).await.unwrap(), RESULTS[0]);
    assert_eq!(paths(&seen), [HISTORY, OPENED]);
    assert_eq!(bodies(&seen)[1], live(Q));
}

#[tokio::test(flavor = "current_thread")]
async fn the_filter_and_diversity_go_on_the_open_and_later_looks_send_nothing_new() {
    let said = history(&[("we booked the cabin", 1)]);
    let (base, seen) = scripted(vec![(200, &said), (200, OPEN), (200, DEEPER), (200, SEARCH)]);
    assert_eq!(turn(&client(&base, "scroll-2").memory_tools("u", None).unwrap()).await, RESULTS);
    assert_eq!(paths(&seen), [HISTORY, WORK_PATHS[0], WORK_PATHS[1], WORK_PATHS[2]]);
    let sent = bodies(&seen);
    let opened: serde_json::Value = serde_json::from_str(&sent[1]).unwrap();
    assert_eq!(opened["own_words"], json!(false));
    assert!(opened.get("filters").is_none());
    assert_eq!(opened["diversity"], "full");
    assert_eq!(sent[1], live(&format!("we booked the cabin\n{Q}")));
    assert_eq!(sent[2..], LATER);
}

#[tokio::test(flavor = "current_thread")]
async fn own_words_and_diversity_set_on_top_of_the_preset_win() {
    assert_eq!(first_look(&[], Q, |m| m.with_own_words(true)).await, asked(Q, FULL));
    assert_eq!(
        first_look(&[], Q, |m| m.with_diversity("mmr").unwrap()).await,
        asked(Q, &format!(r#"{OTHERS},"diversity":"mmr""#))
    );
    assert_eq!(
        first_look(&[], Q, |m| m.with_own_words(true).with_diversity("none").unwrap()).await,
        asked(Q, r#","diversity":"none""#)
    );
    let on_s2 = [
        (Box::new(|m: MemoryTools| m.with_own_words(false)) as Box<dyn FnOnce(MemoryTools) -> MemoryTools>, OTHERS),
        (Box::new(|m: MemoryTools| m.with_diversity("full").unwrap()), FULL),
        (Box::new(|m: MemoryTools| m), ""),
    ];
    for (make, tail) in on_s2 {
        let (base, seen) = scripted(vec![(200, OPEN)]);
        make(client(&base, "scroll-2").memory_tools_preset("u", None, "S-2").unwrap()).begin(Q).await.unwrap();
        assert_eq!(bodies(&seen), [asked(Q, tail)], "{tail}");
    }
}

#[test]
fn refusals_name_both_presets_and_the_model_sees_the_same_tools() {
    let (base, seen) = scripted(vec![]);
    let mem = client(&base, "scroll-2");
    assert_eq!(
        refusal(mem.memory_tools_preset("u", None, "S-3")),
        r#"unknown memory tools preset "S-3": the presets are "S-2", "S-2-live"."#
    );
    assert_eq!(
        refusal(mem.with_model("tablet-2").memory_tools("u", None)),
        r#"model "tablet-2" has no memory tools preset: pass preset "S-2" (scroll-2) or "S-2-live" (scroll-2)."#
    );
    for on in [false, true] {
        let flagged = |m: MemoryTools| m.with_speakers(on).with_dates(on).with_images(on).with_seen(on).with_usage(on);
        let live = flagged(mem.memory_tools("u", None).unwrap());
        let s2 = flagged(mem.memory_tools_preset("u", None, "S-2").unwrap());
        assert_eq!(live.note(), s2.note());
        for format in [ToolFormat::Anthropic, ToolFormat::OpenAi] {
            assert_eq!(live.definitions(format), s2.definitions(format));
        }
    }
    assert_eq!(count(&seen), 0);
}
