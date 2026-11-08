use super::{
    caller_fn, skill_handler, AskAction, AskOpts, AskReason, Client, History, ModelCallError, ModelReply, RouterOptions,
    RunSkillOpts, SkillDefinition, SkillKind, SkillMdOptions, WosError,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const KEY: &str = "wos-test-xxxxxxxxxx";

fn refusal<T: std::fmt::Debug>(r: Result<T, WosError>) -> String {
    match r {
        Err(WosError::Api { status: 400, message, .. }) => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn cannot() -> Arc<dyn super::ModelCaller> {
    caller_fn(|_| async { Ok(ModelReply::new(json!({"action": "cannot", "skill": "", "input": "", "note": "", "text": "no"}).to_string())) })
}

fn wos(router: RouterOptions) -> Client {
    Client::new(KEY).with_tz(9.0).unwrap().with_user("u").with_router(router).unwrap()
}

fn code_skill(name: &str, input: Value) -> SkillDefinition {
    SkillDefinition { name: name.into(), description: "Plan it.".into(), kind: SkillKind::Code, input, ..Default::default() }
}

fn lookup() -> SkillDefinition {
    SkillDefinition {
        name: "lookup".into(),
        description: "Look it up.".into(),
        kind: SkillKind::Llm,
        input: json!({"type": "object"}),
        instructions: Some("Answer.".into()),
        ..Default::default()
    }
}

fn no_history() -> AskOpts {
    AskOpts { history: History::None, ..Default::default() }
}

fn plan_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "tags": {"type": "array", "minItems": 1, "maxItems": 2, "items": {"type": "string"}},
            "mode": {"enum": ["fast", {"at": 1, "by": [true, null]}]},
            "fixed": {"const": {"a": 1, "b": "x"}},
            "size": {"anyOf": [{"type": "number", "minimum": 1}, {"type": "null"}]},
            "flag": {"type": "boolean"},
            "extra": {"type": "object", "additionalProperties": {"type": "integer"}}
        },
        "additionalProperties": false
    })
}

#[tokio::test]
async fn a_skills_input_is_checked_against_lists_enums_consts_any_of_types_and_extra_fields() {
    let ran: Arc<Mutex<Vec<Value>>> = Arc::default();
    let seen = ran.clone();
    let plan = skill_handler(move |input, _| {
        seen.lock().unwrap().push(input);
        async { Ok(json!("planned")) }
    });
    let c = wos(RouterOptions { router_caller: Some(cannot()), handlers: HashMap::from([("plan".to_string(), plan)]), ..Default::default() });
    c.add_skill(code_skill("plan", plan_input()), None).await.unwrap();
    let refused = [
        (json!({"tags": []}), "input.tags must hold at least 1 items"),
        (json!({"tags": ["a", "b", "c"]}), "input.tags must hold at most 2 items"),
        (json!({"tags": ["a", 2]}), "input.tags[1] must be string"),
        (json!({"tags": "a"}), "input.tags must be array"),
        (json!({"mode": "slow"}), r#"input.mode must be one of ["fast",{"at":1,"by":[true,null]}]"#),
        (json!({"mode": {"at": 1, "by": [true]}}), r#"input.mode must be one of ["fast",{"at":1,"by":[true,null]}]"#),
        (json!({"fixed": {"a": 1}}), r#"input.fixed must be {"a":1,"b":"x"}"#),
        (json!({"size": 0}), "input.size fits none of the shapes it may take"),
        (json!({"size": "big"}), "input.size fits none of the shapes it may take"),
        (json!({"flag": "yes"}), "input.flag must be boolean"),
        (json!({"extra": {"n": 1.5}}), "input.extra.n must be integer"),
        (json!({"other": 1}), "input.other is not a field this skill takes"),
    ];
    for (input, why) in refused {
        assert_eq!(refusal(c.run_skill("plan", &input, None, &RunSkillOpts::default()).await), format!("plan: {why}."), "{input}");
    }
    assert!(ran.lock().unwrap().is_empty());
    let input = json!({"tags": ["a"], "mode": {"by": [true, null], "at": 1}, "fixed": {"b": "x", "a": 1}, "size": null, "flag": false, "extra": {"n": 2}});
    let r = c.run_skill("plan", &input, None, &RunSkillOpts::default()).await.unwrap();
    assert_eq!((r.ok, r.text.as_str()), (true, "planned"), "{:?}", r.detail);
    assert_eq!(*ran.lock().unwrap(), vec![input]);
    assert!(c.run_skill("plan", &json!({"size": 2.5, "mode": "fast"}), None, &RunSkillOpts::default()).await.unwrap().ok);
}

#[tokio::test]
async fn a_skills_input_schema_is_refused_where_a_list_keyword_holds_no_list_or_a_sub_schema_is_not_one() {
    let plan = skill_handler(|_, _| async { Ok(json!("ok")) });
    let c = wos(RouterOptions { router_caller: Some(cannot()), handlers: HashMap::from([("plan".to_string(), plan)]), ..Default::default() });
    let refused = [
        (json!({"type": "object", "properties": {"tags": {"type": "array", "items": "string"}}}), "input.tags[]: a schema is an object."),
        (json!({"type": "object", "properties": {"mode": {"enum": []}}}), "input.mode: enum must be a non-empty list."),
        (json!({"type": "object", "properties": {"mode": {"enum": "fast"}}}), "input.mode: enum must be a non-empty list."),
        (json!({"type": "object", "properties": {"size": {"anyOf": []}}}), "input.size: anyOf must be a non-empty list of schemas."),
        (
            json!({"type": "object", "properties": {"size": {"anyOf": [{"type": "nope"}]}}}),
            "input.size.anyOf[0]: type must be one of object, array, string, number, integer, boolean, null, or a list of them.",
        ),
        (json!({"type": "object", "additionalProperties": 1}), "input.*: a schema is an object."),
    ];
    for (input, why) in refused {
        assert_eq!(refusal(c.add_skill(code_skill("plan", input), None).await), why);
    }
    assert!(c.list_skills(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn skill_md_frontmatter_skips_comments_and_blank_lines_joins_indented_lines_and_reads_single_quotes() {
    let c = wos(RouterOptions { router_caller: Some(cannot()), ..Default::default() });
    let text = [
        "---", "# kept out", "name: plan-trip", "", "description: Plans a trip", "  over two lines", "", "note: 'It''s fine'",
        "empty: |", "tags: [a, 'b c']", "---", "Plan it.",
    ]
    .join("\n");
    let d = c.import_skill_md(None, &text, &SkillMdOptions::default()).await.unwrap();
    assert_eq!(d.description, "Plans a trip over two lines");
    assert_eq!(d.metadata.unwrap()["frontmatter"], json!({"note": "It's fine", "empty": "", "tags": ["a", "b c"]}));
    let refused = [
        ("---\n  name: a\ndescription: x\n---\nDo it.", "SKILL.md frontmatter, line 2: an indented line with no key above it."),
        (
            "---\nname: a\ndescription: \"quoted\"\n  continued\n---\nDo it.",
            "SKILL.md frontmatter, line 3: a quoted value cannot go on over the indented lines below it.",
        ),
        ("---\nname: a\ndescription: \"bad \\q\"\n---\nDo it.", "SKILL.md frontmatter, line 3: the double-quoted value is not valid."),
    ];
    for (md, why) in refused {
        assert_eq!(refusal(c.import_skill_md(None, md, &SkillMdOptions::default()).await), why);
    }
}

#[tokio::test]
async fn a_code_skills_error_is_a_tool_error_from_run_skill() {
    let fail = skill_handler(|_, _| async { Err("the oven is off".into()) });
    let c = wos(RouterOptions { router_caller: Some(cannot()), handlers: HashMap::from([("fail".to_string(), fail)]), ..Default::default() });
    c.add_skill(code_skill("fail", json!({"type": "object"})), None).await.unwrap();
    let f = c.run_skill("fail", &json!({}), None, &RunSkillOpts::default()).await.unwrap();
    assert_eq!(
        (f.ok, f.reason, f.detail.as_deref(), f.skill.as_deref()),
        (false, Some(AskReason::ToolError), Some("fail returned an error: the oven is off"), Some("fail"))
    );
    assert!(!f.text.is_empty());
}

#[tokio::test]
async fn an_llm_skill_with_no_caller_is_not_configured_and_one_answering_no_text_failed() {
    let lonely = wos(RouterOptions { router_caller: Some(cannot()), ..Default::default() });
    lonely.add_skill(lookup(), None).await.unwrap();
    let r = lonely.run_skill("lookup", &json!({}), None, &RunSkillOpts::default()).await.unwrap();
    assert_eq!(
        (r.ok, r.reason, r.detail.as_deref()),
        (false, Some(AskReason::NotConfigured), Some("an llm skill needs RouterOptions.key, skill_key or skill_caller"))
    );
    let mute = caller_fn(|_| async {
        Ok(ModelReply { text: "  ".into(), stop: "end".into(), usage: Some(json!({"input_tokens": 3, "cache": {"read": 1}, "tier": "standard"})) })
    });
    let c = wos(RouterOptions { router_caller: Some(cannot()), skill_caller: Some(mute), ..Default::default() });
    c.add_skill(lookup(), None).await.unwrap();
    let e = c.run_skill("lookup", &json!({}), None, &RunSkillOpts::default()).await.unwrap();
    assert_eq!((e.ok, e.reason, e.detail.as_deref()), (false, Some(AskReason::Failed), Some("the skill model answered with no text")));
    assert_eq!(e.usage.unwrap().skill, Some(json!({"input_tokens": 3, "cache": {"read": 1}, "tier": "standard"})));
}

#[tokio::test]
async fn a_provider_error_with_a_status_the_router_has_no_reason_for_is_failed() {
    let busy = caller_fn(|_| async { Err(ModelCallError::new(409, "busy elsewhere")) });
    let c = wos(RouterOptions { router_caller: Some(busy), ..Default::default() });
    c.add_skill(lookup(), None).await.unwrap();
    let r = c.ask("q", None, &no_history()).await.unwrap();
    assert_eq!((r.ok, r.reason, r.detail.as_deref()), (false, Some(AskReason::Failed), Some("409: busy elsewhere")));
}

#[tokio::test]
async fn a_router_decision_inside_prose_is_found_past_escaped_quotes_and_escaped_letters() {
    let said = r#"Thinking {"x":"{\"y", z} then {"action":"cannot","skill":"","input":"","note":"a\tb","text":"no \"way\"\n"} done"#;
    let c = wos(RouterOptions { router_caller: Some(caller_fn(move |_| async move { Ok(ModelReply::new(said)) })), ..Default::default() });
    c.add_skill(lookup(), None).await.unwrap();
    let r = c.ask("q", None, &no_history()).await.unwrap();
    assert_eq!((r.ok, r.action, r.text.as_str()), (true, Some(AskAction::Cannot), "no \"way\""), "{:?}", r.detail);
}

fn model_call() -> super::ModelCall {
    super::ModelCall {
        purpose: super::CallPurpose::Router,
        model: "m".into(),
        system: vec!["s".into()],
        user: "u".into(),
        max_tokens: 10,
        effort: None,
        schema: None,
        system_tokens: None,
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
    }
}

const LLM_KEY: &str = "sk-test-0000000000000000";

fn anthropic_at(base: &str) -> super::AnthropicCaller {
    super::anthropic_caller(super::AnthropicCallerOpts { api_key: LLM_KEY.into(), base_url: Some(base.into()), ..Default::default() }).unwrap()
}

fn openai_at(base: &str) -> super::OpenAiCaller {
    super::openai_caller(super::OpenAiCallerOpts { base_url: base.into(), model: "m".into(), api_key: Some(LLM_KEY.into()), ..Default::default() })
        .unwrap()
}

fn endless_answer() -> String {
    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}", "0".repeat(super::MODEL_RESPONSE_MAX_BYTES + 1))
}

#[tokio::test]
async fn a_provider_answer_that_is_not_json_or_too_large_by_its_length_or_its_bytes_is_refused() {
    use super::tests::{http, mock_server};
    use super::ModelCaller;
    let big = super::MODEL_RESPONSE_MAX_BYTES + 1;
    let long = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {big}\r\nConnection: close\r\n\r\n{{}}");
    let (base, _) = mock_server(vec![http("200 OK", "", "not json"), http("200 OK", "", "[1]"), long, endless_answer()]);
    let c = anthropic_at(&base);
    let call = model_call();
    for why in [
        "the provider's answer is not JSON".to_string(),
        "the provider's answer is not a JSON object".to_string(),
        format!("the provider's answer is too large ({big} bytes)"),
        "the provider's answer is too large".to_string(),
    ] {
        let e = c.call(&call).await.unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (200, why.as_str()));
    }
}

#[tokio::test]
async fn a_provider_error_whose_status_has_no_reason_keeps_none_and_its_message_is_found_where_the_server_put_it() {
    use super::tests::{http, mock_server};
    use super::ModelCaller;
    let bodies = [
        (json!({"error": {"message": "conflict"}}).to_string(), "conflict"),
        ("plain words".to_string(), "plain words"),
        ("[1, 2]".to_string(), "[1, 2]"),
        (json!({"error": "said as a string"}).to_string(), "said as a string"),
        (json!({"message": "said at the top"}).to_string(), "said at the top"),
    ];
    let (base, _) = mock_server(bodies.iter().map(|(b, _)| http("409 Conflict", "", b)).collect());
    let c = openai_at(&base);
    let call = model_call();
    for (_, said) in bodies {
        let e = c.call(&call).await.unwrap_err();
        assert_eq!((e.status, e.reason.as_deref(), e.message.as_str()), (409, None, said));
    }
}

fn rfc850(year: i64) -> String {
    let leaps = |n: i64| n.div_euclid(4) - n.div_euclid(100) + n.div_euclid(400);
    let days = (year - 1970) * 365 + leaps(year - 1) - leaps(1969);
    let day = ["Thursday", "Friday", "Saturday", "Sunday", "Monday", "Tuesday", "Wednesday"][days.rem_euclid(7) as usize];
    format!("{day}, 01-Jan-{:02} 00:00:00 GMT", year.rem_euclid(100))
}

fn this_year() -> i64 {
    httpdate::fmt_http_date(std::time::SystemTime::now())[12..16].parse().unwrap()
}

#[tokio::test]
async fn a_retry_after_in_the_old_rfc_850_form_reads_its_two_digit_year_as_at_most_50_years_ahead() {
    use super::tests::{http, mock_server};
    use super::ModelCaller;
    let year = this_year();
    let dates = [rfc850(year + 49), rfc850(year - 49), rfc850(year - 1)];
    let (base, _) = mock_server(dates.iter().map(|d| http("429 Too Many Requests", &format!("Retry-After: {d}\r\n"), "{}")).collect());
    let c = openai_at(&base);
    let call = model_call();
    let ahead = c.call(&call).await.unwrap_err().retry_after;
    assert!(ahead.is_some_and(|w| w.as_secs() > 48 * 365 * 86_400), "{}: {ahead:?}", dates[0]);
    for d in &dates[1..] {
        assert_eq!(c.call(&call).await.unwrap_err().retry_after, Some(std::time::Duration::ZERO), "{d}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_page_cursor_the_service_repeats_as_an_object_ends_the_walk_as_a_repeated_string_cursor_does() {
    use super::tests::{http, mock_server};
    let empty = json!({"memories": [], "next_cursor": {"page": 2}}).to_string();
    let full = json!({"memories": [{"id": "m1", "content": "x"}], "next_cursor": {"page": 2}}).to_string();
    let (base, hits) = mock_server(vec![http("200 OK", "", &empty), http("200 OK", "", &empty), http("200 OK", "", &full), http("200 OK", "", &full)]);
    let mem = Client::with_base_url(KEY, &base).with_retries(0);
    assert!(mem.list_all_memories("u").await.unwrap().is_empty());
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
    let e = mem.list_all_memories("u").await.unwrap_err().to_string();
    assert!(e.contains("the service handed back a cursor it had already given"), "{e}");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 4);
}

#[tokio::test(flavor = "current_thread")]
async fn an_image_answer_with_no_bytes_is_an_error() {
    use super::tests::{http, mock_server};
    let (base, _) = mock_server(vec![http("200 OK", "", "")]);
    let mem = Client::with_base_url(KEY, &base).with_retries(0);
    let e = mem.get_image("u", "11111111-2222-3333-4444-555555555555").await.unwrap_err();
    assert!(e.to_string().contains("empty image body — the service returned no bytes"), "{e}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_sleep_estimate_reads_from_a_given_time_and_a_cursor_repeated_with_memories_is_a_truncated_answer() {
    use super::tests::{http, mock_server_recording};
    let night = json!([{"id": "m-1", "content": "Dana planted roses today.", "created_at": "2026-10-19T09:00:00Z", "speaker": "Dana", "tokens": 8}]);
    let page = |memories: &Value| http("200 OK", "", &json!({"memories": memories, "next_cursor": "c1"}).to_string());
    let files: Vec<Value> = (1..=4).map(|slot| json!({"slot": slot, "title": "", "version": 0, "tokens": 0, "written_at": null, "day": null})).collect();
    let me = json!({"user_id": "u", "core": {"title": "", "version": 0, "tokens": 0, "written_at": null, "day": null, "body": ""}, "files": files, "notes_version": 0});
    let (base, seen) = mock_server_recording(vec![page(&night), page(&json!([])), http("200 OK", "", &me.to_string()), page(&night), page(&night)]);
    let mem = Client::with_base_url(KEY, &base).with_retries(0);
    let opts = super::SleepEstimateOpts { since: Some("2026-10-19T00:00:00Z".into()), ..Default::default() };
    let est = mem.sleep_estimate("u", &opts).await.unwrap();
    assert_eq!(est["memories"], 1);
    let first = seen.lock().unwrap()[0].clone();
    assert!(first.contains("\"since\":\"2026-10-19T00:00:00Z\"") && !first.contains("since_last_sleep"), "{first}");
    assert!(seen.lock().unwrap()[1].contains("\"cursor\":\"c1\""));
    let e = mem.sleep_estimate("u", &super::SleepEstimateOpts::default()).await.unwrap_err().to_string();
    assert!(e.contains("the service returned a page cursor it had already returned. This is a truncated answer, not the whole store."), "{e}");
}

#[tokio::test(flavor = "current_thread")]
async fn an_earlier_exchanges_time_with_an_offset_is_shown_on_the_routers_clock_and_an_offset_past_23_hours_as_a_question_mark() {
    use super::tests::{http, mock_server};
    let exchanges = json!({"exchanges": [
        {"request": "east", "answer": "a", "at": "2026-10-20T09:00:00+09:00"},
        {"request": "west", "answer": "b", "at": "2026-10-20T09:00:00-05:30"},
        {"request": "bad", "answer": "c", "at": "2026-10-20T09:00:00+24:00"}
    ]});
    let (base, _) = mock_server(vec![http("200 OK", "", &exchanges.to_string()), http("200 OK", "", "{\"status\":\"ok\"}")]);
    let users: Arc<Mutex<Vec<String>>> = Arc::default();
    let kept = users.clone();
    let router = caller_fn(move |c| {
        kept.lock().unwrap().push(c.user.clone());
        async { Ok(ModelReply::new(json!({"action": "cannot", "skill": "", "input": "", "note": "", "text": "no"}).to_string())) }
    });
    let c = Client::with_base_url(KEY, &base).with_retries(0).with_tz(9.0).unwrap().with_user("u").with_router(RouterOptions { router_caller: Some(router), ..Default::default() }).unwrap();
    c.add_skill(lookup(), None).await.unwrap();
    let r = c.ask("q", None, &AskOpts::default()).await.unwrap();
    assert!(r.ok, "{:?}", r.detail);
    let shown = "[2026-10-20 09:00:00] request: east\nanswer: a\n[2026-10-20 23:30:00] request: west\nanswer: b\n[?] request: bad\nanswer: c";
    let user = users.lock().unwrap()[0].clone();
    assert!(user.contains(shown), "{user}");
}

fn served_by_path(answer: fn(&str) -> Value) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for sock in listener.incoming() {
            let Ok(mut sock) = sock else { continue };
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let head_end = loop {
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                    match sock.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
                let len: usize = head.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
                while buf.len() < head_end + len {
                    match sock.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let path = head.split(' ').nth(1).unwrap_or("").split('?').next().unwrap_or("").to_string();
                let body = answer(&path).to_string();
                let out = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = sock.write_all(out.as_bytes());
            });
        }
    });
    base
}

#[tokio::test]
async fn a_sleep_refuses_a_rewrite_with_no_section_a_dated_box_section_a_core_remake_and_a_remake_with_no_title() {
    let base = served_by_path(|path| {
        if path.ends_with("/sleep") {
            json!({"user_id": "u", "sleep_id": "s-1", "count": 2, "tokens": 17, "exhausted": true, "next_cursor": null, "memories": [
                {"id": "m-1", "content": "Dana planted roses today.", "created_at": "2026-10-19T09:00:00Z", "speaker": "Dana", "tokens": 8},
                {"id": "m-2", "content": "Dana said the roses need water.", "created_at": "2026-10-19T09:01:00Z", "speaker": "Dana", "tokens": 9}
            ]})
        } else if path.ends_with("/self/notes") {
            json!({"notes": {"proposals": [], "mixed": []}})
        } else {
            let files: Vec<Value> = (1..=4).map(|slot| json!({"slot": slot, "title": "", "version": 0, "tokens": 0, "written_at": null, "day": null})).collect();
            json!({"user_id": "u", "core": {"title": "", "version": 0, "tokens": 0, "written_at": null, "day": null, "body": ""}, "files": files, "notes_version": 0})
        }
    });
    let llm = caller_fn(|c| async move {
        let ev = json!(["e1", "e2"]);
        let said = if c.user.contains("<items>") {
            json!({"notes": [], "edits": [
                {"file": "core", "op": "rewrite", "section": "", "text": "Dana gardens.", "evidence": ev},
                {"file": "1", "op": "rewrite", "section": "2026-10-20", "text": "Roses went in.", "evidence": ev},
                {"file": "core", "op": "remake", "title": "Core", "text": "Everything anew.", "evidence": ev},
                {"file": "1", "op": "remake", "title": "", "text": "## Garden\nRoses.", "evidence": ev}
            ]})
        } else if c.user.contains("<memories>") {
            json!({"items": [{"kind": "person", "topic": "Dana", "text": "Dana planted roses and waters them.", "evidence": ev}], "mixed": []})
        } else {
            json!({"add": [], "op": "none", "drop": [], "text": ""})
        };
        Ok(ModelReply::new(said.to_string()))
    });
    let mem = Client::with_base_url(KEY, &base).with_retries(0);
    let opts = super::SleepOpts { assistant: Some("Kai".into()), today: Some("2026-10-20 09:30".into()), ..Default::default() };
    let res = mem.sleep("u", llm, &opts).await.unwrap();
    let why: Vec<&str> = res["refused"].as_array().unwrap().iter().filter_map(|r| r["why"].as_str()).collect();
    assert_eq!(
        why,
        [
            "rewrite needs a section and text",
            "a box section is a matter, not its state on a date: rewrite the matter's section",
            "the core is rewritten by sections, not remade",
            "remake needs a title and the whole file",
        ]
    );
}

#[test]
fn a_blank_router_prompt_is_refused() {
    for prompt in ["", "  \n"] {
        let made = Client::new(KEY).with_router(RouterOptions { router_caller: Some(cannot()), prompt: Some(prompt.into()), ..Default::default() });
        assert_eq!(refusal(made), "RouterOptions.prompt must be the whole router prompt: a non-blank string.");
    }
}
