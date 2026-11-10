use super::rooms_work_tests::{count, refusal, scripted, KEY};
use super::{Client, WriteOpts};
use serde_json::json;

const AT: &str = "2024-03-01T10:00:00Z";

#[tokio::test(flavor = "current_thread")]
async fn add_takes_created_at_and_sends_it_as_metadata_created_at() {
    let (base, seen) = scripted(vec![]);
    let mem = Client::with_base_url(KEY, &base);
    let dated = WriteOpts { created_at: Some(AT.into()), ..Default::default() };
    mem.add_opts("we met in Busan", "u", json!({}), &dated).await.unwrap();
    mem.add("we met in Busan", "u", json!({"created_at": AT})).await.unwrap();
    mem.add_opts("we met in Busan", "u", json!({"speaker": "Mina"}), &dated).await.unwrap();
    mem.add_opts("we met in Busan", "u", serde_json::Value::Null, &dated).await.unwrap();
    let seen = seen.lock().unwrap();
    let got: Vec<_> = seen.iter().map(|s| s.body["metadata"].clone()).collect();
    assert_eq!(
        got,
        [
            json!({"created_at": AT}),
            json!({"created_at": AT}),
            json!({"speaker": "Mina", "created_at": AT}),
            json!({"created_at": AT}),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn add_refuses_a_created_at_that_is_not_an_instant() {
    let (base, seen) = scripted(vec![]);
    let mem = Client::with_base_url(KEY, &base);
    for bad in ["2024-03-01", "2024-03-01T10:00:00", "yesterday", ""] {
        let o = WriteOpts { created_at: Some(bad.into()), ..Default::default() };
        assert!(refusal(mem.add_opts("x", "u", json!({}), &o).await).contains("created_at"), "{bad}");
    }
    assert!(refusal(mem.add("x", "u", json!({"created_at": "2024-03-01"})).await).contains("created_at must be RFC3339"));
    assert!(refusal(mem.add("x", "u", json!({"created_at": 5})).await).contains("created_at"));
    assert_eq!(count(&seen), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn add_refuses_the_two_sides_of_a_turn() {
    let (base, seen) = scripted(vec![]);
    let mem = Client::with_base_url(KEY, &base);
    let user = WriteOpts { user_speaker: Some("Mina".into()), ..Default::default() };
    let assistant = WriteOpts { assistant_speaker: Some("Kai".into()), ..Default::default() };
    for (o, name) in [(user, "user_speaker"), (assistant, "assistant_speaker")] {
        let want = format!("{name} is for add_turn_opts: it names a side of a turn. On add, name who said it in metadata.speaker.");
        assert_eq!(refusal(mem.add_opts("x", "u", json!({}), &o).await), want);
        let mut md = serde_json::Map::new();
        md.insert(name.to_string(), json!("Mina"));
        assert_eq!(refusal(mem.add("x", "u", serde_json::Value::Object(md)).await), want);
    }
    assert_eq!(count(&seen), 0);
}

const FAR_OPEN: &str = "{\"work_id\":\"w-1\",\"memories\":[\
{\"n\":1,\"id\":\"m-1\",\"content\":\"The far day.\",\"created_at\":\"9999-12-31T23:59:59Z\"},\
{\"n\":2,\"id\":\"m-2\",\"content\":\"one\\u000btwo\\u000cthree\\u001cfour\\u001dfive\\u001esix\",\"created_at\":\"2024-03-01T10:00:00Z\"},\
{\"n\":3,\"id\":\"m-3\",\"content\":\"Year five.\",\"created_at\":\"0005-06-01T10:00:00Z\"}]}";
const KEPT: &str = "[02] one\n    two\n    three\n    four\n    five\n    six";

#[tokio::test(flavor = "current_thread")]
async fn a_far_date_is_whole_and_rare_breaks_are_indented() {
    let (base, _seen) = scripted(vec![(200, FAR_OPEN)]);
    let mem = Client::with_base_url(KEY, &base).with_model("scroll-2").with_retries(0).with_tz(9.0).unwrap();
    let tools = mem.memory_tools_preset("u", None, "S-2").unwrap().with_dates(true);
    assert_eq!(tools.begin("q").await.unwrap(), format!("[01] The far day.\n{KEPT}\n[03] Year five."));
    assert_eq!(
        tools.run("memory_timeline", &json!({})).await.unwrap(),
        format!("In the order they happened, oldest first:\n0005-06-01\n19:00 [03] Year five.\n2024-03-01\n19:00 {KEPT}\n+010000-01-01\n08:59 [01] The far day.")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_recall_hands_out_the_memories_from_the_rooms() {
    let both = r#"{"long_term":{"memories":[{"id":"l1","content":"an old trip"}]},"rooms":{"memories":[{"id":"r1","content":"this morning"},{"id":"r2","content":"last night"}],"count":2}}"#;
    let plain = r#"{"long_term":{"memories":[{"id":"l1","content":"an old trip"}]}}"#;
    let (base, _seen) = scripted(vec![(200, both), (200, plain)]);
    let mem = Client::with_base_url(KEY, &base).with_retries(0);
    let r = mem.recall("q", "u").await.unwrap();
    let said: Vec<String> = r.room_memories().into_iter().map(|m| m.content).collect();
    assert_eq!(said, ["this morning", "last night"]);
    assert_eq!(r.memories().len(), 1);
    assert!(mem.recall("q", "u").await.unwrap().room_memories().is_empty());
}
