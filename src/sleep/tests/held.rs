use super::*;

struct Deaf(Mutex<usize>);

impl ModelCaller for Deaf {
    fn call<'a>(&'a self, _c: &'a ModelCall) -> BoxFuture<'a, Result<ModelReply, ModelCallError>> {
        *self.0.lock().unwrap() += 1;
        Box::pin(std::future::pending())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_call_that_ignores_its_deadline_is_raced_against_it_and_tried_twice_more() {
    let deaf = Arc::new(Deaf(Mutex::new(0)));
    let asker = Asker { caller: deaf.clone(), model: "m".into(), max_tokens: 100, call_time: std::time::Duration::from_millis(50) };
    let t0 = std::time::Instant::now();
    let e = tokio::time::timeout(std::time::Duration::from_secs(10), asker.ask("s", "u"))
        .await
        .expect("a call that ignores its deadline holds the sleep")
        .unwrap_err();
    assert_eq!((e.status, e.reason.as_deref(), e.message.as_str()), (0, Some("timeout"), "the sleep's model call ran past 0.05 s"));
    assert_eq!(*deaf.0.lock().unwrap(), 3, "raced at its deadline, and tried twice more");
    assert!(t0.elapsed() < std::time::Duration::from_secs(4));
}

#[test]
fn a_box_is_picked_by_its_number_or_whole_name_before_part_of_one_and_never_by_a_blank() {
    let mut view: SleepView = Default::default();
    for (s, title) in [(1, "Notes on work"), (2, "Work"), (4, "WORK")] {
        view[s].title = title.into();
    }
    for (which, want) in [
        ("", None),
        ("   ", None),
        ("None", None),
        ("Work", Some(2)),
        ("work", Some(2)),
        ("WORK", Some(4)),
        ("notes", Some(1)),
        ("on wo", Some(1)),
        ("2", Some(2)),
        (" 4 ", Some(4)),
        ("3", Some(3)),
        ("5", None),
        ("Recipes", None),
    ] {
        assert_eq!(sleep_box_pick(&view, which), want, "{which:?}");
    }
}

#[test]
fn the_fenced_json_is_found_in_one_pass_with_the_same_result_as_before() {
    for (text, want) in [
        ("Here: ```json\n{\"a\": {\"b\": 2}}\n``` done", Some(json!({"a": {"b": 2}}))),
        ("```\n{\"a\": 1}\n``` and ```json {\"b\": 2} ```", None),
        ("````{\"a\": 1}```", Some(json!({"a": 1}))),
        ("```json\n{\"a\": 1}\n``` then {\"b\": 2}", Some(json!({"a": 1}))),
        ("```{\"a\": 1} no fence after it, {\"b\": 2}", None),
        ("x {\"a\": \"}```\"} y", Some(json!({"a": "}```"}))),
    ] {
        assert_eq!(sleep_parse_json(text).map(Value::Object), want, "{text}");
    }
    let t0 = std::time::Instant::now();
    assert_eq!(sleep_parse_json(&"```{}x".repeat(50_000)), None);
    assert!(t0.elapsed() < std::time::Duration::from_millis(200), "{:?}", t0.elapsed());
}

#[test]
fn a_proposal_the_sleep_cannot_read_is_left_out_of_its_work_and_kept_in_its_notes_as_it_was() {
    let good = json!({"id": "p1", "text": "short answers", "sleeps": 2.0, "first": 1.5, "first_day": "2026-10-01"});
    let bad = json!([{"id": "p2", "text": "x", "first_day": "d", "first": 1}, {"id": "p3", "text": "x", "sleeps": 1, "first": 1},
                     {"id": "p4", "text": "x", "sleeps": "2", "first": 1, "first_day": "d"},
                     {"id": "p5", "text": "x", "sleeps": 0, "first": 1, "first_day": "d"},
                     {"id": "p6", "text": "x", "sleeps": 1, "first": true, "first_day": "d"}, "p7", null]);
    let all: Vec<Value> = [vec![good.clone()], bad.as_array().unwrap().clone()].concat();
    let (version, notes) = sleep_notes_of(&json!({"version": 3, "notes": {"proposals": all, "mixed": []}}));
    assert_eq!(version, 3);
    let mut normal = good;
    normal["sleeps"] = json!(2);
    assert_eq!(notes["proposals"], json!([vec![normal.clone()], bad.as_array().unwrap().clone()].concat()));
    let readable: Vec<Value> = notes["proposals"].as_array().unwrap().iter().filter(|p| sleep_readable(p)).cloned().collect();
    assert_eq!(readable, [normal]);
    assert!(notes["proposals"][0]["sleeps"].is_i64());
}

#[test]
fn over_their_cap_the_notes_drop_the_oldest_applied_proposals_then_the_oldest_marks_never_an_open_one() {
    let p = |id: &str, text: String, applied: bool| {
        let mut p = json!({"id": id, "text": text, "sleeps": 1, "first": 1, "first_day": "d"});
        if applied {
            p["applied"] = json!("d");
        }
        p
    };
    let m = |id: &str, why: String| json!({"memory": id, "why": why});
    let cases = [
        (vec![p("p1", "a".into(), true)], vec![m("m-1", "w".into())], vec!["p1"], vec!["m-1"]),
        (
            vec![
                p("p1", "a".repeat(60_000), true),
                p("p2", "b".repeat(60_000), false),
                p("p3", "c".repeat(60_000), true),
                p("p4", "d".into(), true),
                json!({"id": "p5", "text": "e".repeat(60_000), "applied": "d"}),
            ],
            vec![m("m-1", "x".repeat(20_000)), m("m-2", "y".into())],
            vec!["p2", "p4", "p5"],
            vec!["m-1", "m-2"],
        ),
        (
            vec![p("p1", "a".repeat(60_000), true), p("p2", "b".repeat(150_000), false)],
            vec![m("m-1", "x".repeat(60_000)), m("m-2", "y".into())],
            vec!["p2"],
            vec!["m-2"],
        ),
        (vec![p("p1", "b".repeat(250_000), false)], vec![m("m-1", "y".into()), m("m-2", "z".into())], vec!["p1"], vec![]),
    ];
    for (proposals, mixed, kept, marks) in cases {
        let mut notes = json!({"proposals": proposals, "mixed": mixed}).as_object().unwrap().clone();
        sleep_notes_fit(&mut notes);
        let ids = |k: &str, f: &str| -> Vec<String> { notes[k].as_array().unwrap().iter().map(|x| text_of(&x[f])).collect() };
        let want: (Vec<String>, Vec<String>) =
            (kept.iter().map(|s| s.to_string()).collect(), marks.iter().map(|s| s.to_string()).collect());
        assert_eq!((ids("proposals", "id"), ids("mixed", "memory")), want);
    }
}
