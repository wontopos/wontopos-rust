use super::rooms_work_tests::scripted;
use super::tests::{http, mock_server, mock_server_recording};
use super::{
    caller_fn, skill_handler, AskAction, AskOpts, AskReason, Client, History, ModelCallError, ModelCaller, ModelReply, RouterOptions,
    RunSkillOpts, SkillDefinition, SkillKind, WosError,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const KEY: &str = "wos-test-xxxxxxxxxx";
const LLM_KEY: &str = "sk-test-0000000000000000";
const PING: &str = r#"{"status":"ok"}"#;

enum Step {
    Say(String),
    Stall(String, u64),
    Blurt(String),
    Reset,
}

fn read_head(sock: &mut std::net::TcpStream) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match sock.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

fn serve(steps: Vec<Step>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for step in steps {
            let Ok((mut sock, _)) = listener.accept() else { return };
            match step {
                Step::Say(out) => {
                    read_head(&mut sock);
                    let _ = sock.write_all(out.as_bytes());
                }
                Step::Stall(out, ms) => {
                    read_head(&mut sock);
                    let _ = sock.write_all(out.as_bytes());
                    std::thread::sleep(Duration::from_millis(ms));
                }
                Step::Blurt(out) => {
                    let _ = sock.read(&mut [0u8; 4096]);
                    let _ = sock.write_all(out.as_bytes());
                }
                Step::Reset => {
                    std::thread::sleep(Duration::from_millis(100));
                    let _ = sock.read(&mut [0u8; 1]);
                }
            }
        }
    });
    base
}

fn cut_short() -> String {
    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 512\r\nConnection: close\r\n\r\n{\"st".to_string()
}

fn busy() -> String {
    http("503 Service Unavailable", "Retry-After: 0\r\n", r#"{"error":{"type":"overloaded","message":"busy right now"}}"#)
}

fn wos(base: &str) -> Client {
    Client::with_base_url(KEY, base).with_retries(0)
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
        deadline: Instant::now() + Duration::from_secs(30),
    }
}

fn anthropic_at(base: &str) -> super::AnthropicCaller {
    super::anthropic_caller(super::AnthropicCallerOpts { api_key: LLM_KEY.into(), base_url: Some(base.into()), ..Default::default() }).unwrap()
}

fn openai_at(base: &str) -> super::OpenAiCaller {
    super::openai_caller(super::OpenAiCallerOpts { base_url: base.into(), model: "m".into(), api_key: Some(LLM_KEY.into()), ..Default::default() })
        .unwrap()
}

fn network(r: Result<bool, WosError>) -> WosError {
    match r {
        Err(e @ WosError::Network(_)) => e,
        other => panic!("expected a network error, got {other:?}"),
    }
}

#[tokio::test]
async fn an_answer_that_is_not_http_is_a_network_error_with_no_cause_status_code_or_reason() {
    let base = serve(vec![Step::Say("garbage\r\n\r\n".into())]);
    let e = network(wos(&base).ping().await);
    let text = e.to_string();
    assert!(text.starts_with("network error: ") && !text.contains(" ("), "{text}");
    assert_eq!(e.kind(), super::ErrorKind::Connection);
    assert_eq!((e.status(), e.code(), e.conflicts_with(), e.reason(), e.resets_at()), (None, None, None, None, None));
    assert!(std::error::Error::source(&e).is_some());
    assert!(!e.is_gone() && !e.is_auth() && !e.is_rate_limited());
}

#[tokio::test]
async fn a_connection_the_server_resets_is_named_as_reset() {
    let base = serve(vec![Step::Reset]);
    let text = network(wos(&base).ping().await).to_string();
    assert!(text.ends_with("(connection reset)"), "{text}");
}

#[tokio::test]
async fn a_body_that_breaks_its_own_framing_is_an_interrupted_body() {
    let framing = ["Transfer-Encoding", "chunked"];
    let base = serve(vec![Step::Say(format!("HTTP/1.1 200 OK\r\n{}: {}\r\nConnection: close\r\n\r\nzz\r\n", framing[0], framing[1]))]);
    let text = network(wos(&base).ping().await).to_string();
    assert!(text.ends_with("(response body interrupted)"), "{text}");
}

#[tokio::test]
async fn a_connect_the_network_cannot_route_is_a_connect_failure() {
    let text = network(wos("http://255.255.255.255:9").ping().await).to_string();
    assert!(text.ends_with("(connect failed)"), "{text}");
}

#[tokio::test]
async fn https_to_a_server_that_speaks_plain_http_is_a_tls_failure() {
    let base = serve(vec![Step::Blurt(http("400 Bad Request", "", "{}"))]);
    let text = network(wos(&base.replace("http://", "https://")).ping().await).to_string();
    assert!(text.ends_with("(tls)"), "{text}");
}

#[tokio::test]
async fn a_host_that_does_not_resolve_is_a_dns_failure() {
    let text = network(wos("http://wontopos-cov.invalid").ping().await).to_string();
    assert!(text.ends_with("(dns)"), "{text}");
}

#[tokio::test]
async fn a_reqwest_error_becomes_an_api_error_when_it_has_a_status_and_a_network_error_when_not() {
    let (base, _) = mock_server(vec![http("404 Not Found", "", "{}")]);
    let e = reqwest::get(format!("{base}/x?k=secret")).await.unwrap().error_for_status().unwrap_err();
    match WosError::from(e) {
        WosError::Api { status: 404, message } => assert!(message.contains("404") && !message.contains("secret"), "{message}"),
        other => panic!("{other:?}"),
    }
    let e = reqwest::get("http://127.0.0.1:9/").await.unwrap_err();
    assert!(matches!(WosError::from(e), WosError::Network(_)));
    let api = WosError::Api { status: 401, message: "no".into() };
    assert!(std::error::Error::source(&api).is_none());
    assert_eq!(api.kind(), super::ErrorKind::Auth);
    assert!(api.is_auth());
}

#[tokio::test]
async fn a_read_whose_connection_closes_before_an_answer_is_sent_again() {
    let (base, hits) = mock_server(vec![String::new(), http("200 OK", "", PING)]);
    assert!(Client::with_base_url(KEY, &base).with_retries(1).ping().await.unwrap());
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_deadline_that_has_run_out_before_the_first_attempt_sends_nothing() {
    let (base, hits) = mock_server(vec![http("200 OK", "", PING)]);
    let e = Client::with_base_url(KEY, &base).with_deadline(Duration::from_nanos(1)).ping().await.unwrap_err();
    assert_eq!(e.to_string(), "[0] deadline of 1ns exhausted");
    assert_eq!(e.kind(), super::ErrorKind::Connection);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_body_that_drops_when_no_backoff_fits_the_deadline_reports_the_deadline_or_the_answer_before_it() {
    let (base, hits) = mock_server(vec![cut_short()]);
    let e = Client::with_base_url(KEY, &base).with_deadline(Duration::from_millis(300)).ping().await.unwrap_err();
    assert_eq!(e.to_string(), "[0] deadline of 300ms exhausted");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let (base, hits) = mock_server(vec![busy(), cut_short()]);
    let e = Client::with_base_url(KEY, &base).with_deadline(Duration::from_millis(400)).ping().await.unwrap_err();
    assert_eq!((e.status(), e.to_string().as_str()), (Some(503), "[503] busy right now"));
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_body_that_stalls_past_the_deadline_after_a_retried_answer_reports_that_answer() {
    let stalled = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 64\r\n\r\n{\"st".to_string();
    let base = serve(vec![Step::Say(busy()), Step::Stall(stalled, 1500)]);
    let started = Instant::now();
    let e = Client::with_base_url(KEY, &base).with_deadline(Duration::from_millis(400)).ping().await.unwrap_err();
    assert_eq!((e.status(), e.to_string().as_str()), (Some(503), "[503] busy right now"));
    assert!(started.elapsed() < Duration::from_millis(1400), "{:?}", started.elapsed());
}

#[tokio::test]
async fn an_answer_past_the_size_cap_with_no_length_is_refused_as_it_streams() {
    let big = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}", " ".repeat(super::MAX_RESPONSE_BYTES + 1));
    let (base, _) = mock_server(vec![big]);
    let e = wos(&base).ping().await.unwrap_err();
    assert_eq!(e.to_string(), "[200] response too large — refusing to buffer it");
}

#[test]
fn an_rfc_850_date_with_a_wrong_separator_or_a_year_that_is_not_digits_is_no_date() {
    for bad in ["Sunday, 06+Nov-94 08:49:37 GMT", "Sunday, 06-Nov+94 08:49:37 GMT", "Sunday, 06-Nov-9x 08:49:37 GMT"] {
        assert_eq!(super::http_date(bad), None, "{bad}");
    }
    assert!(super::http_date("Sunday, 06-Nov-94 08:49:37 GMT").is_some());
}

#[test]
fn a_write_lock_wait_that_is_negative_or_not_a_number_is_no_wait() {
    for body in [r#"{"error":{"retry_after_ms":-1}}"#, r#"{"error":{"retry_after_ms":"5"}}"#, r#"{"error":"busy"}"#] {
        assert_eq!(super::write_lock_wait(body), None, "{body}");
    }
    assert_eq!(super::write_lock_wait(r#"{"error":{"retry_after_ms":1.2}}"#), Some(Duration::from_millis(2)));
}

#[test]
fn a_model_call_error_reads_as_its_message_its_status_and_its_type() {
    assert_eq!(ModelCallError::new(0, "the call's time ran out").to_string(), "the call's time ran out");
    assert_eq!(ModelCallError::new(503, "busy").to_string(), "503: busy");
    let e = ModelCallError::new(529, "busy").with_request_id("req_1");
    assert_eq!(e.request_id.as_deref(), Some("req_1"));
}

#[test]
fn a_blank_provider_base_url_or_key_is_refused() {
    let blank_base = super::anthropic_caller(super::AnthropicCallerOpts { api_key: LLM_KEY.into(), base_url: Some(" / ".into()), ..Default::default() });
    assert_eq!(
        blank_base.unwrap_err().to_string(),
        "[400] the model provider's base URL must be a URL, e.g. https://api.anthropic.com."
    );
    let blank_key = super::anthropic_caller(super::AnthropicCallerOpts { api_key: " \t".into(), ..Default::default() });
    assert_eq!(blank_key.unwrap_err().to_string(), "[400] api_key is required: the model provider's key.");
}

#[tokio::test]
async fn a_model_call_whose_time_ran_out_before_it_was_sent_sends_nothing() {
    let (base, hits) = mock_server(vec![http("200 OK", "", "{}")]);
    let call = super::ModelCall { deadline: Instant::now(), ..model_call() };
    let e = anthropic_at(&base).call(&call).await.unwrap_err();
    assert_eq!((e.status, e.message.as_str()), (0, "the call's time ran out before it was sent"));
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_anthropic_answer_with_no_stop_reason_ended_and_one_that_is_not_a_string_is_kept_as_written() {
    let bodies = [
        (json!({"content": [{"type": "text", "text": "a"}, {"type": "tool_use", "text": "x"}, {"type": "text"}]}), "a", "end"),
        (json!({"content": [{"type": "text", "text": "b"}], "stop_reason": null}), "b", "end"),
        (json!({"content": [{"type": "text", "text": "c"}], "stop_reason": 5}), "c", "5"),
        (json!({"content": [], "stop_reason": "refusal"}), "", "refusal"),
    ];
    let (base, _) = mock_server(bodies.iter().map(|(b, _, _)| http("200 OK", "", &b.to_string())).collect());
    let c = anthropic_at(&base);
    for (_, text, stop) in bodies {
        let r = c.call(&model_call()).await.unwrap();
        assert_eq!((r.text.as_str(), r.stop.as_str()), (text, stop));
    }
}

#[tokio::test]
async fn an_openai_answer_in_parts_is_joined_and_an_unclosed_think_is_kept() {
    let bodies = [
        (json!({"choices": [{"message": {"content": [{"type": "text", "text": "a"}, {"text": "b"}, {"type": "image"}]}}]}), "ab"),
        (json!({"choices": [{"message": {"content": "<think>still going"}}]}), "<think>still going"),
        (json!({"choices": [{"message": {"content": "  <think>x</think>  done"}}]}), "done"),
    ];
    let (base, _) = mock_server(bodies.iter().map(|(b, _)| http("200 OK", "", &b.to_string())).collect());
    let c = openai_at(&base);
    for (_, text) in bodies {
        assert_eq!(c.call(&model_call()).await.unwrap().text, text);
    }
}

#[test]
fn a_timestamp_with_a_month_and_no_day_is_the_first_and_an_hour_with_no_minutes_is_no_time() {
    assert_eq!(super::epoch_ms("2026-10"), super::epoch_ms("2026-10-01T00:00:00Z"));
    assert_eq!(super::epoch_ms("2026-10-20T09"), None);
    assert_eq!(super::epoch_ms("2026-10-20 09"), None);
}

#[test]
fn an_instant_with_a_fraction_needs_a_digit_after_the_point() {
    assert!(super::rfc3339_shaped("2026-10-20T09:00:00.123Z"));
    assert!(super::rfc3339_shaped("2026-10-20T09:00:00.5+09:00"));
    assert!(!super::rfc3339_shaped("2026-10-20T09:00:00.Z"));
    assert!(super::check_instant("2026-10-20T09:00:00.Z", "since").is_err());
}

#[test]
fn an_image_data_url_with_no_comma_is_kept_whole_without_its_spaces() {
    assert_eq!(super::normalize_image_b64(" data:ab c "), "data:abc");
    assert_eq!(super::normalize_image_b64("data:image/png;base64,QU J D"), "QUJD");
}

#[test]
fn a_blank_store_id_is_never_recorded_as_a_collision() {
    super::warn_if_store_id_collapses("");
    if let Some(m) = super::WARNED_STORE_IDS.get() {
        assert!(!m.lock().unwrap_or_else(|e| e.into_inner()).iter().any(String::is_empty));
    }
}

#[test]
fn a_warning_set_past_its_cap_forgets_an_older_key_and_stays_at_the_cap() {
    static SET: super::WarnSet = std::sync::OnceLock::new();
    for i in 0..=super::WARNED_KEYS_MAX {
        assert!(super::first_sighting(&SET, &format!("k{i}")));
    }
    assert_eq!(SET.get().unwrap().lock().unwrap().len(), super::WARNED_KEYS_MAX);
    assert!(!super::first_sighting(&SET, &format!("k{}", super::WARNED_KEYS_MAX)));
}

#[test]
fn rooms_written_as_all_are_every_room() {
    assert_eq!(super::rooms_from(" all "), super::Rooms::All);
}

#[tokio::test]
async fn a_look_with_rooms_none_sends_rooms_false_and_no_clock() {
    let (base, seen) = mock_server_recording(vec![http("200 OK", "", r#"{"work_id":"w-1"}"#)]);
    let opts = super::LookOpts { rooms: Some(super::Rooms::None), tz: Some(9.0), ..Default::default() };
    wos(&base).open_work("where", "u", &opts).await.unwrap();
    let sent = seen.lock().unwrap()[0].clone();
    assert!(sent.contains(r#""rooms":false"#) && !sent.contains("tz_offset_minutes"), "{sent}");
}

#[tokio::test]
async fn a_sleep_read_carries_its_now() {
    let (base, seen) = mock_server_recording(vec![http("200 OK", "", r#"{"memories":[]}"#)]);
    let opts = super::SleepReadOpts { tz: Some(0.0), days_ago: Some(0), now: Some("2026-10-20T10:00:00Z".into()), ..Default::default() };
    wos(&base).sleep_read("u", &opts).await.unwrap();
    let sent = seen.lock().unwrap()[0].clone();
    assert!(sent.contains(r#""now":"2026-10-20T10:00:00Z""#), "{sent}");
}

#[tokio::test]
async fn around_without_held_and_retrievals_without_options_send_only_what_they_were_given() {
    let (base, seen) = mock_server_recording(vec![http("200 OK", "", r#"{"pieces":[]}"#), http("200 OK", "", r#"{"entries":[]}"#)]);
    let c = wos(&base);
    c.around("m-1", "u", &super::AroundOpts::default()).await.unwrap();
    c.retrievals("u", &super::RetrievalsOpts::default()).await.unwrap();
    let seen = seen.lock().unwrap();
    assert!(seen[0].ends_with(r#"{"user_id":"u","memory_id":"m-1"}"#), "{}", seen[0]);
    assert!(seen[1].ends_with(r#"{"user_id":"u"}"#), "{}", seen[1]);
}

#[tokio::test]
async fn a_page_keeps_the_count_the_service_gave_and_search_keeps_memories_and_images_with_no_id() {
    let page = json!({"memories": [{"id": "m-1", "content": "x"}], "count": 7, "next_cursor": "c2"}).to_string();
    let found = json!({"memories": [{"content": "a"}], "self_memories": [{"content": "b"}], "images": [{"content": "c"}]}).to_string();
    let (base, _) = mock_server(vec![http("200 OK", "", &page), http("200 OK", "", &found)]);
    let c = wos(&base);
    let p = c.list_memories("u", 10, None).await.unwrap();
    assert_eq!((p["count"].clone(), p["next_cursor"].clone()), (json!(7), json!("c2")));
    let got: Vec<String> = c.search("q", "u", 10).await.unwrap().into_iter().map(|m| m.content).collect();
    assert_eq!(got, ["a", "b", "c"]);
}

fn says(d: Value) -> Arc<dyn ModelCaller> {
    caller_fn(move |_| {
        let d = d.clone();
        async move { Ok(ModelReply::new(d.to_string())) }
    })
}

fn decide(action: &str, skill: &str, input: &str, text: &str) -> Arc<dyn ModelCaller> {
    says(json!({"action": action, "skill": skill, "input": input, "note": "", "text": text}))
}

fn routed(router: RouterOptions) -> Client {
    Client::new(KEY).with_tz(9.0).unwrap().with_user("u").with_router(router).unwrap()
}

fn plan(input: Value) -> SkillDefinition {
    SkillDefinition { name: "plan".into(), description: "Plan it.".into(), kind: SkillKind::Code, input, ..Default::default() }
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

fn quiet() -> AskOpts {
    AskOpts { history: History::None, ..Default::default() }
}

fn handlers(f: super::SkillHandler) -> HashMap<String, super::SkillHandler> {
    HashMap::from([("plan".to_string(), f)])
}

fn refusal<T: std::fmt::Debug>(r: Result<T, WosError>) -> String {
    match r {
        Err(WosError::Api { status: 400, message }) => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn router_options_refuse_a_bad_caller_or_handler_name_and_a_blank_name_or_time_zone() {
    let ok = || skill_handler(|_, _| async { Ok(json!("ok")) });
    let made = |o: RouterOptions| refusal(Client::new(KEY).with_router(RouterOptions { router_caller: Some(decide("cannot", "", "", "no")), ..o }));
    assert_eq!(
        made(RouterOptions { callers: HashMap::from([("Bad-Name".to_string(), decide("cannot", "", "", ""))]), ..Default::default() }),
        "RouterOptions.callers: \"Bad-Name\" is not a name (1-32 of [a-z0-9_], a letter first)."
    );
    assert_eq!(
        made(RouterOptions { handlers: HashMap::from([("9x".to_string(), ok())]), ..Default::default() }),
        "RouterOptions.handlers: \"9x\" is not a skill name (1-32 of [a-z0-9_], a letter first)."
    );
    assert_eq!(made(RouterOptions { name: Some(" \n".into()), ..Default::default() }), "RouterOptions.name must be a non-blank string.");
    assert_eq!(
        made(RouterOptions { time_zone: Some("  ".into()), ..Default::default() }),
        "RouterOptions.time_zone must be an IANA time zone name such as \"Asia/Seoul\"."
    );
    assert_eq!(AskReason::RateLimit.to_string(), "rate_limit");
}

#[tokio::test]
async fn the_routers_own_prompt_and_output_budget_reach_its_call() {
    let calls: Arc<Mutex<Vec<(String, u32)>>> = Arc::default();
    let kept = calls.clone();
    let router = caller_fn(move |c| {
        kept.lock().unwrap().push((c.system[0].clone(), c.max_tokens));
        async { Ok(ModelReply::new(json!({"action": "cannot", "skill": "", "input": "", "note": "", "text": "no"}).to_string())) }
    });
    let c = routed(RouterOptions { router_caller: Some(router), prompt: Some("Route it.".into()), max_tokens: Some(4000), ..Default::default() });
    c.add_skill(lookup(), None).await.unwrap();
    assert!(c.ask("q", None, &quiet()).await.unwrap().ok);
    assert_eq!(*calls.lock().unwrap(), [("Route it.".to_string(), 4000)]);
}

#[tokio::test]
async fn a_provider_reason_the_router_does_not_know_falls_back_to_the_status() {
    let busy = caller_fn(|_| async { Err(ModelCallError::new(503, "busy").with_reason("weird")) });
    let c = routed(RouterOptions { router_caller: Some(busy), ..Default::default() });
    c.add_skill(lookup(), None).await.unwrap();
    let r = c.ask("q", None, &quiet()).await.unwrap();
    assert_eq!((r.ok, r.reason, r.detail.as_deref()), (false, Some(AskReason::Overloaded), Some("503: busy")));
}

#[tokio::test]
async fn an_action_the_router_does_not_have_is_no_decision() {
    let c = routed(RouterOptions { router_caller: Some(decide("dance", "", "", "la")), ..Default::default() });
    c.add_skill(lookup(), None).await.unwrap();
    let r = c.ask("q", None, &quiet()).await.unwrap();
    assert_eq!((r.ok, r.reason, r.detail.as_deref()), (false, Some(AskReason::BadChoice), Some("the router's answer holds no decision")));
}

#[tokio::test]
async fn an_explain_with_no_words_lists_the_skills() {
    let c = routed(RouterOptions { router_caller: Some(decide("explain", "", "", " ")), ..Default::default() });
    c.add_skill(lookup(), None).await.unwrap();
    let r = c.ask("q", None, &quiet()).await.unwrap();
    assert_eq!((r.ok, r.action, r.text), (true, Some(AskAction::Explain), format!("{} lookup", super::router_text::EXPLAIN_FALLBACK)));
}

#[tokio::test]
async fn a_skill_chosen_with_blank_input_gets_an_empty_object_and_a_null_result_reads_as_finished_without_one() {
    let got: Arc<Mutex<Vec<Value>>> = Arc::default();
    let seen = got.clone();
    let f = skill_handler(move |input, _| {
        seen.lock().unwrap().push(input);
        async { Ok(Value::Null) }
    });
    let c = routed(RouterOptions { router_caller: Some(decide("skill", "plan", "  ", "")), handlers: handlers(f), ..Default::default() });
    c.add_skill(plan(json!({"type": "object"})), None).await.unwrap();
    let r = c.ask("q", None, &quiet()).await.unwrap();
    assert_eq!((r.ok, r.action, r.text.as_str()), (true, Some(AskAction::Skill), super::router_text::EMPTY), "{:?}", r.detail);
    assert_eq!(*got.lock().unwrap(), [json!({})]);
    let tools = c.router_tools(None, &quiet()).unwrap();
    let said: Value = serde_json::from_str(&tools.run_arguments("ask", r#"{"request": "q"}"#).await.unwrap()).unwrap();
    assert_eq!(said["action"], "skill");
    assert_eq!(refusal(tools.run_arguments("other", "{}").await), "not the ask tool: \"other\"");
}

#[tokio::test]
async fn a_skills_usage_with_a_fraction_is_kept_as_a_fraction() {
    let skill = caller_fn(|_| async { Ok(ModelReply { text: "found".into(), stop: "end".into(), usage: Some(json!({"cost": 0.25, "input_tokens": 3})) }) });
    let c = routed(RouterOptions { router_caller: Some(decide("skill", "lookup", "{}", "")), skill_caller: Some(skill), ..Default::default() });
    c.add_skill(lookup(), None).await.unwrap();
    let r = c.ask("q", None, &quiet()).await.unwrap();
    assert!(r.ok, "{:?}", r.detail);
    assert_eq!(r.usage.unwrap().skill, Some(json!({"cost": 0.25, "input_tokens": 3})));
}

#[tokio::test]
async fn a_speaker_over_256_bytes_a_skill_that_is_not_an_object_and_a_blank_copy_target_are_refused() {
    let c = routed(RouterOptions { router_caller: Some(decide("cannot", "", "", "no")), ..Default::default() });
    c.add_skill(lookup(), None).await.unwrap();
    let long = AskOpts { speaker: Some("x".repeat(257)), ..quiet() };
    assert_eq!(refusal(c.ask("q", None, &long).await), "speaker is at most 256 bytes.");
    assert_eq!(
        refusal(c.import_skills(r#"{"wontopos_skills": 1, "skills": [7]}"#, None).await),
        "skills[0]: a skill is an object: { name, description, kind, input, ... }. Nothing was imported."
    );
    assert_eq!(refusal(c.copy_skill("lookup", "u", &["a", " "]).await), "to holds \" \", which is not a store id.");
}

#[tokio::test]
async fn a_skill_schema_deeper_than_32_levels_is_refused_and_a_list_of_types_or_a_list_with_no_items_schema_is_checked() {
    let f = || skill_handler(|_, _| async { Ok(json!("ok")) });
    let c = routed(RouterOptions { router_caller: Some(decide("cannot", "", "", "no")), handlers: handlers(f()), ..Default::default() });
    let mut deep = json!({"type": "string"});
    for _ in 0..33 {
        deep = json!({"type": "array", "items": deep});
    }
    let e = refusal(c.add_skill(plan(json!({"type": "object", "properties": {"d": deep}})), None).await);
    assert!(e.ends_with(": the schema nests deeper than 32 levels."), "{e}");
    let input = json!({"type": "object", "properties": {"v": {"type": ["string", "null"]}, "all": {"type": "array"}}});
    c.add_skill(plan(input), None).await.unwrap();
    let run = |input: Value| {
        let c = &c;
        async move { c.run_skill("plan", &input, None, &RunSkillOpts::default()).await }
    };
    assert!(refusal(run(json!({"v": 1})).await).starts_with("plan: input.v must be "));
    assert!(run(json!({"v": null, "all": [1, "a", null]})).await.unwrap().ok);
    assert!(run(json!({"v": "x"})).await.unwrap().ok);
}

#[tokio::test]
async fn a_history_read_that_outlasts_the_asks_time_ends_the_ask_before_the_router_is_called() {
    let base = serve(vec![Step::Stall(http("200 OK", "", r#"{"exchanges":[]}"#).replace("Content-Length", "X-Length"), 1500)]);
    let calls: Arc<Mutex<usize>> = Arc::default();
    let n = calls.clone();
    let router = caller_fn(move |_| {
        *n.lock().unwrap() += 1;
        async { Ok(ModelReply::new("{}")) }
    });
    let limits = super::RouterLimits { time: Some(Duration::from_millis(300)), ..Default::default() };
    let c = Client::with_base_url(KEY, &base).with_retries(0).with_tz(9.0).unwrap().with_user("u");
    let c = c.with_router(RouterOptions { router_caller: Some(router), limits, ..Default::default() }).unwrap();
    c.add_skill(lookup(), None).await.unwrap();
    let r = c.ask("q", None, &AskOpts::default()).await.unwrap();
    assert_eq!((r.ok, r.reason), (false, Some(AskReason::Timeout)), "{:?}", r.detail);
    assert_eq!(*calls.lock().unwrap(), 0);
}

struct SlowCount;

impl ModelCaller for SlowCount {
    fn call<'a>(&'a self, _: &'a super::ModelCall) -> futures_util::future::BoxFuture<'a, Result<ModelReply, ModelCallError>> {
        Box::pin(async { Ok(ModelReply::new("{}")) })
    }
    fn count_tokens<'a>(&'a self, _: &'a super::ModelCall) -> Option<futures_util::future::BoxFuture<'a, Result<u64, ModelCallError>>> {
        Some(Box::pin(async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok(1)
        }))
    }
}

#[tokio::test]
async fn a_token_count_that_outlasts_the_asks_time_ends_the_ask() {
    let limits = super::RouterLimits { time: Some(Duration::from_millis(200)), ..Default::default() };
    let c = routed(RouterOptions { router_caller: Some(Arc::new(SlowCount)), limits, ..Default::default() });
    c.add_skill(lookup(), None).await.unwrap();
    let r = c.ask("q", None, &quiet()).await.unwrap();
    assert_eq!((r.ok, r.reason), (false, Some(AskReason::Timeout)), "{:?}", r.detail);
}

#[tokio::test]
async fn an_answer_whose_time_is_used_up_refuses_its_next_ask() {
    let f = skill_handler(|_, _| async {
        tokio::time::sleep(Duration::from_millis(600)).await;
        Ok(json!("late"))
    });
    let limits = super::RouterLimits { time: Some(Duration::from_millis(200)), ..Default::default() };
    let c = routed(RouterOptions { router_caller: Some(decide("skill", "plan", "{}", "")), handlers: handlers(f), limits, ..Default::default() });
    c.add_skill(plan(json!({"type": "object"})), None).await.unwrap();
    let turn = AskOpts { turn: Some("t1".into()), ..quiet() };
    let first = c.ask("q", None, &turn).await.unwrap();
    assert_eq!(first.reason, Some(AskReason::Timeout), "{:?}", first.detail);
    let next = c.ask("q", None, &turn).await.unwrap();
    assert_eq!((next.reason, next.detail.as_deref()), (Some(AskReason::Limit), Some("this answer has used its 200 ms")));
}

fn files(core_day: Option<&str>, box_one: Value) -> String {
    let mut boxes: Vec<Value> = (2..=4).map(|slot| json!({"slot": slot, "title": "", "version": 0, "tokens": 0, "written_at": null, "day": null})).collect();
    boxes.insert(0, box_one);
    json!({"user_id": "u", "core": {"title": "", "version": 1, "tokens": 3, "written_at": null, "day": core_day, "body": "## Who\nDana."}, "files": boxes, "notes_version": 0})
        .to_string()
}

#[tokio::test(flavor = "current_thread")]
async fn a_box_opened_before_any_turn_reads_the_files_first_and_takes_the_title_and_day_it_listed() {
    let listed = json!({"slot": 1, "title": "Garden", "version": 2, "tokens": 9, "written_at": null, "day": "2026-10-01"});
    let opened = json!({"slot": 1, "title": "", "version": 2, "tokens": 9, "day": null, "body": "## Roses\nPlanted."}).to_string();
    let (base, seen) = scripted(vec![(200, &files(None, listed)), (200, &opened)]);
    let c = Client::with_base_url(KEY, &base).with_model("scroll-2");
    let tools = c.memory_tools_preset("u", None, "S-2").unwrap().with_sleep(true).with_today("2026-10-20").unwrap();
    let text = tools.run("open_box", &json!({"box": "1"})).await.unwrap();
    assert!(text.contains("Garden") && text.contains("Roses") && text.contains("2026-10-01"), "{text}");
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn a_seen_line_with_no_state_or_one_this_client_does_not_name_says_it_as_given_and_another_failure_is_refused() {
    let window = r#"{"turns":[{"role":"assistant","content":"Sure.","timestamp":"2026-10-20T09:00:05Z","seen":2}]}"#;
    let held = r#"{"count":2,"seen":[{"id":"m-1","status":"","content":"A."},{"id":"m-2","status":"archived","content":"B."}]}"#;
    let (base, _) = scripted(vec![(200, window), (200, held), (500, r#"{"error":{"message":"down"}}"#)]);
    let tools = Client::with_base_url(KEY, &base).with_retries(0).with_model("scroll-2").memory_tools_preset("u", None, "S-2").unwrap().with_seen(true);
    tools.run("memory_recent", &json!({})).await.unwrap();
    let text = tools.run("memory_seen", &json!({"n": 1})).await.unwrap();
    assert!(text.ends_with("- unknown: A.\n- archived: B."), "{text}");
    let failed = tools.run("memory_seen", &json!({"n": 1})).await.unwrap();
    assert!(failed.starts_with("Refused: ") && failed.contains("down"), "{failed}");
}

#[tokio::test]
async fn a_skill_with_a_model_name_that_is_not_one_is_refused_under_its_own_name() {
    let c = routed(RouterOptions { router_caller: Some(decide("cannot", "", "", "no")), ..Default::default() });
    let e = refusal(c.add_skill(SkillDefinition { model: Some("bad model".into()), ..lookup() }, None).await);
    assert!(e.starts_with("lookup: model"), "{e}");
    assert!(c.list_skills(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_skill_md_value_holding_a_line_or_paragraph_separator_is_refused() {
    let c = routed(RouterOptions { router_caller: Some(decide("cannot", "", "", "no")), ..Default::default() });
    for cut in ['\u{2028}', '\u{2029}'] {
        let md = format!("---\nname: a\ndescription: one{cut}two\n---\nDo it.");
        let e = refusal(c.import_skill_md(None, &md, &super::SkillMdOptions::default()).await);
        assert!(e.starts_with("SKILL.md frontmatter, line 3"), "{e}");
    }
}

struct CountsSlowlyPastTheStablePart;

impl ModelCaller for CountsSlowlyPastTheStablePart {
    fn call<'a>(&'a self, _: &'a super::ModelCall) -> futures_util::future::BoxFuture<'a, Result<ModelReply, ModelCallError>> {
        Box::pin(async { Ok(ModelReply::new("{}")) })
    }
    fn count_tokens<'a>(&'a self, c: &'a super::ModelCall) -> Option<futures_util::future::BoxFuture<'a, Result<u64, ModelCallError>>> {
        let quick = c.user == "-";
        Some(Box::pin(async move {
            if !quick {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Ok(10)
        }))
    }
}

#[tokio::test]
async fn a_long_request_whose_exact_count_outlasts_the_asks_time_ends_the_ask() {
    let limits = super::RouterLimits { time: Some(Duration::from_millis(300)), ..Default::default() };
    let c = routed(RouterOptions { router_caller: Some(Arc::new(CountsSlowlyPastTheStablePart)), limits, ..Default::default() });
    c.add_skill(lookup(), None).await.unwrap();
    let r = c.ask(&"y".repeat(100_000), None, &quiet()).await.unwrap();
    assert_eq!((r.ok, r.reason), (false, Some(AskReason::Timeout)), "{:?}", r.detail);
}

const ENV_KEY: &str = "wos-live-envenvenvenvenv1234";

#[test]
fn alone_with_the_debug_log_on_the_log_names_each_attempt_and_never_a_key() {
    if std::env::var_os("WONTOPOS_COV_ALONE").is_some() {
        return;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["cov_rs_tests::alone::", "--test-threads=1", "--nocapture"])
        .env("WONTOPOS_COV_ALONE", "1")
        .env("WONTOPOS_LOG", "debug")
        .env("WONTOPOS_API_KEY", " \t")
        .env("WOS_API_KEY", ENV_KEY)
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&out.stderr);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && said.contains("4 passed"), "{said}\n{log}");
    for line in [
        "wontopos: GET /api/v1/memory/ping -> 503 — retrying in 0ms (attempt 1/2)",
        "wontopos: GET /api/v1/memory/ping: connect error — retrying in ",
        "wontopos: GET /api/v1/memory/ping: body dropped — retrying in ",
        "(attempt 2/2)",
        "response_format json_schema refused (400); asking with json_object",
        "response_format json_object refused (400); asking with none",
    ] {
        assert!(log.contains(line), "{line:?} is not in\n{log}");
    }
    assert!(log.lines().any(|l| l.starts_with("wontopos: GET /api/v1/memory/ping -> 404 in ") && l.ends_with("ms (attempt 1/1)")), "{log}");
    for key in [KEY, LLM_KEY, ENV_KEY] {
        assert!(!log.contains(key) && !said.contains(key), "{key} is in the log");
    }
}

mod alone {
    use super::*;

    fn alone() -> bool {
        std::env::var_os("WONTOPOS_COV_ALONE").is_some()
    }

    #[test]
    fn from_env_passes_over_a_blank_key_to_the_next_name() {
        if !alone() {
            return;
        }
        let c = Client::from_env().unwrap();
        assert!(format!("{c:?}").contains("***1234"), "{c:?}");
    }

    #[test]
    fn resetting_the_warning_state_empties_every_set() {
        if !alone() {
            return;
        }
        super::super::_reset_warning_state();
        super::super::warn_if_store_id_collapses("Dana@Example.com");
        for set in [&super::super::WARNED_FILTER_KEYS, &super::super::WARNED_METADATA_KEYS, &super::super::WARNED_RETIREMENTS] {
            assert!(super::super::first_sighting(set, "k"));
        }
        super::super::_reset_warning_state();
        assert!(super::super::WARNED_STORE_IDS.get().unwrap().lock().unwrap().is_empty());
        for set in [&super::super::WARNED_FILTER_KEYS, &super::super::WARNED_METADATA_KEYS, &super::super::WARNED_RETIREMENTS] {
            assert!(set.get().unwrap().lock().unwrap().is_empty());
        }
    }

    #[test]
    fn the_plain_http_warnings_past_their_cap_forget_the_oldest() {
        if !alone() {
            return;
        }
        let url = |i: usize| format!("http://h{i}.example.com");
        for i in 0..=super::super::WARNED_PLAIN_HTTP_MAX {
            assert!(super::super::plain_http_once("base_url", &url(i)));
        }
        assert!(!super::super::plain_http_once("base_url", &url(1)));
        assert!(super::super::plain_http_once("base_url", &url(0)));
    }

    #[tokio::test]
    async fn the_debug_log_runs() {
        if !alone() {
            return;
        }
        let (base, _) = mock_server(vec![busy(), http("200 OK", "", PING), http("404 Not Found", "", r#"{"error":{"message":"none"}}"#)]);
        assert!(Client::with_base_url(KEY, &base).with_retries(1).ping().await.unwrap());
        assert!(wos(&base).ping().await.is_err());
        assert!(Client::with_base_url(KEY, "http://127.0.0.1:9").with_retries(1).ping().await.is_err());
        let (base, _) = mock_server(vec![cut_short(), http("200 OK", "", PING)]);
        assert!(Client::with_base_url(KEY, &base).with_retries(1).ping().await.unwrap());
        let refused = |what: &str| http("400 Bad Request", "", &json!({"error": {"message": format!("response_format {what} is not supported")}}).to_string());
        let answer = json!({"choices": [{"message": {"content": "{}"}}]}).to_string();
        let (base, _) = mock_server(vec![refused("json_schema"), refused("json_object"), http("200 OK", "", &answer)]);
        let call = super::super::ModelCall { schema: Some(json!({"type": "object"})), ..model_call() };
        assert_eq!(openai_at(&base).call(&call).await.unwrap().text, "{}");
    }
}

type Served = Arc<Mutex<Vec<(String, Value)>>>;

fn sleep_service(answer: fn(&str, &Value) -> Option<Value>) -> (String, Served) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen: Served = Arc::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        for sock in listener.incoming() {
            let Ok(mut sock) = sock else { continue };
            let log = log.clone();
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
                let body: Value = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
                log.lock().unwrap().push((path.clone(), body.clone()));
                let said = answer(&path, &body).unwrap_or_else(|| usual(&path));
                let status = said.get("status_for_the_stand_in").and_then(Value::as_u64).unwrap_or(200);
                let said = said.get("body_for_the_stand_in").unwrap_or(&said).to_string();
                let out = format!("HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{said}", said.len());
                let _ = sock.write_all(out.as_bytes());
            });
        }
    });
    (base, seen)
}

fn night() -> Value {
    json!({"user_id": "u", "sleep_id": "s-1", "count": 2, "tokens": 17, "exhausted": true, "next_cursor": null, "memories": [
        {"id": "m-1", "content": "Dana planted roses today.", "created_at": "2026-10-19T09:00:00Z", "speaker": "Dana", "tokens": 8},
        {"id": "m-2", "content": "Dana said the roses need water.", "created_at": "2026-10-19T09:01:00Z", "speaker": "Dana", "tokens": 9}
    ]})
}

fn usual(path: &str) -> Value {
    if path.ends_with("/sleep") {
        night()
    } else if path.ends_with("/self/notes") {
        json!({"notes": {"proposals": [], "mixed": []}})
    } else {
        let files: Vec<Value> = (1..=4).map(|slot| json!({"slot": slot, "title": "", "version": 0, "tokens": 0, "written_at": null, "day": null})).collect();
        json!({"user_id": "u", "core": {"title": "", "version": 0, "tokens": 0, "written_at": null, "day": null, "body": ""}, "files": files, "notes_version": 0})
    }
}

fn sleep_opts() -> super::SleepOpts {
    super::SleepOpts { assistant: Some("Kai".into()), today: Some("2026-10-20 09:30".into()), ..Default::default() }
}

fn stand_in(read: Value, merge: Value) -> Arc<dyn ModelCaller> {
    let users: Arc<Mutex<Vec<String>>> = Arc::default();
    stand_in_seen(read, merge, users)
}

fn stand_in_seen(read: Value, merge: Value, users: Arc<Mutex<Vec<String>>>) -> Arc<dyn ModelCaller> {
    caller_fn(move |c| {
        users.lock().unwrap().push(c.user.clone());
        let said = if c.user.contains("<items>") {
            merge.clone()
        } else if c.user.contains("<memories>") {
            read.clone()
        } else {
            json!({"add": "not a list", "op": "none", "drop": [], "text": ""})
        };
        async move { Ok(ModelReply::new(said.to_string())) }
    })
}

#[test]
fn sleep_options_print_what_they_hold_and_never_the_prompt_or_the_callbacks_and_a_sleep_error_keeps_its_source() {
    let opts = super::SleepOpts { prompt: "secret prompt".into(), count_tokens: Some(Arc::new(|s: &str| s.len() as u64)), ..sleep_opts() };
    let shown = format!("{opts:?}");
    assert!(shown.starts_with("SleepOpts { merge_llm: false, being: ") && shown.contains("prompt: true") && shown.contains("count_tokens: true"), "{shown}");
    assert!(!shown.contains("secret prompt"), "{shown}");
    let est = format!("{:?}", super::SleepEstimateOpts::default());
    assert!(est.starts_with("SleepEstimateOpts { input_price: ") && est.ends_with("count_tokens: false }"), "{est}");
    let wos = super::SleepError::Wos(WosError::Api { status: 409, message: "changed".into() });
    let model = super::SleepError::Model(ModelCallError::new(529, "busy"));
    assert_eq!(std::error::Error::source(&wos).unwrap().to_string(), "[409] changed");
    assert_eq!(std::error::Error::source(&model).unwrap().to_string(), "529: busy");
}

#[tokio::test]
async fn a_reading_with_no_sleep_id_writes_nothing_and_a_clients_clock_goes_with_the_reading() {
    let (base, seen) = sleep_service(|path, _| path.ends_with("/sleep").then(|| {
        let mut page = night();
        page.as_object_mut().unwrap().remove("sleep_id");
        page
    }));
    let mem = Client::with_base_url(KEY, &base).with_retries(0).with_tz(9.0).unwrap();
    let e = mem.sleep("u", stand_in(json!({}), json!({})), &sleep_opts()).await.unwrap_err();
    assert_eq!(e.to_string(), "[200] the reading came back without a sleep_id: nothing could be written for it");
    let seen = seen.lock().unwrap();
    let read = seen.iter().find(|(p, _)| p.ends_with("/sleep")).unwrap();
    assert_eq!(read.1["tz_offset_minutes"], 540);
    assert!(seen.iter().all(|(p, _)| !p.contains("/self/") || p.ends_with("/self") || p.ends_with("/self/notes")), "{:?}", seen.iter().map(|s| &s.0).collect::<Vec<_>>());
}

#[tokio::test]
async fn an_items_text_given_as_a_list_reads_as_python_prints_it_and_bad_edits_and_proposals_are_left_out() {
    let (base, _) = sleep_service(|path, _| path.ends_with("/sleep").then(|| {
        let mut page = night();
        page["exhausted"] = json!(1);
        page
    }));
    let ev = json!(["e1", "e2"]);
    let read = json!({"items": [
        {"kind": "person", "topic": "Dana", "text": ["a\\b\n\r\t", 18_446_744_073_709_551_615u64, -0.0, {"k": true}], "evidence": ev}
    ], "mixed": []});
    let merge = json!({"notes": [], "proposals": [1, {"text": " "}], "edits": [
        {"file": "7", "op": "rewrite", "section": "Who", "text": "Dana gardens.", "evidence": ev},
        {"file": "core", "op": "title", "text": "Core", "evidence": ev},
        {"file": "1", "op": "title", "text": "2026-10-20", "evidence": ev}
    ]});
    let users: Arc<Mutex<Vec<String>>> = Arc::default();
    let mem = Client::with_base_url(KEY, &base).with_retries(0);
    let res = mem.sleep("u", stand_in_seen(read, merge, users.clone()), &sleep_opts()).await.unwrap();
    let why: Vec<&str> = res["refused"].as_array().unwrap().iter().filter_map(|r| r["why"].as_str()).collect();
    assert_eq!(
        why,
        [
            "unknown file or op",
            "the core has no title; a title needs text",
            "a box is a matter, not its state on a date: name it for the matter"
        ]
    );
    let printed = serde_json::to_string(r"['a\\b\n\r\t', 1.8446744073709552e+19, 0, {'k': True}]").unwrap();
    let users = users.lock().unwrap();
    let merged = users.iter().find(|u| u.contains("<items>")).unwrap();
    assert!(merged.contains(&printed[1..printed.len() - 1]), "{merged}");
}

#[tokio::test]
async fn a_read_answer_whose_items_or_mixed_are_not_lists_is_read_as_python_iterates_them() {
    let (base, _) = sleep_service(|_, _| None);
    let mem = Client::with_base_url(KEY, &base).with_retries(0);
    for read in [json!({"items": "ab", "mixed": {"k": 1}}), json!({"items": {"k": 1}, "mixed": "ab"})] {
        let res = mem.sleep("u", stand_in(read.clone(), json!({"edits": [], "notes": []})), &sleep_opts()).await.unwrap();
        assert!(res.is_object(), "{read}: {res}");
    }
}

fn read_dana() -> Value {
    json!({"items": [{"kind": "person", "topic": "Dana", "text": "Dana planted roses and waters them.", "evidence": ["e1", "e2"]}], "mixed": []})
}

#[tokio::test]
async fn a_service_clock_this_client_cannot_read_leaves_the_night_on_the_system_clock() {
    static LOCAL_NOW: Mutex<String> = Mutex::new(String::new());
    let (base, _) = sleep_service(|path, _| {
        path.ends_with("/sleep").then(|| {
            let mut page = night();
            page["clock"] = json!({"local_now": *LOCAL_NOW.lock().unwrap()});
            page
        })
    });
    let mem = Client::with_base_url(KEY, &base).with_retries(0);
    let arabic = "\u{662}\u{660}\u{662}\u{666}-\u{661}\u{660}-\u{662}\u{660} \u{660}\u{669}:\u{663}\u{660}";
    for local_now in ["2026/10/20 09:30", "2026-10/20 09:30", arabic] {
        *LOCAL_NOW.lock().unwrap() = local_now.to_string();
        let opts = super::SleepOpts { today: None, ..sleep_opts() };
        let res = mem.sleep("u", stand_in(read_dana(), json!({"edits": [], "notes": []})), &opts).await;
        assert!(res.is_ok(), "{local_now}: {res:?}");
    }
}

#[tokio::test]
async fn a_written_box_takes_its_new_version_from_the_answers_versions() {
    let (base, seen) = sleep_service(|path, _| path.ends_with("/self/write").then(|| json!({"versions": {"1": 2, "x": 5, "2": "three"}})));
    let ev = json!(["e1", "e2"]);
    let merge = json!({"notes": [], "edits": [{"file": "1", "op": "remake", "title": "Garden", "text": "## Roses\nDana planted roses.", "evidence": ev}]});
    let mem = Client::with_base_url(KEY, &base).with_retries(0);
    let res = mem.sleep("u", stand_in(read_dana(), merge), &sleep_opts()).await.unwrap();
    assert_eq!(res["refused"], json!([]), "{res}");
    assert!(seen.lock().unwrap().iter().any(|(p, b)| p.ends_with("/self/write") && b["writes"][0]["slot"] == 1));
}

#[tokio::test]
async fn a_no_room_refusal_with_no_message_is_made_room_for_once_then_the_box_stays_as_it_was() {
    let (base, seen) = sleep_service(|path, _| {
        path.ends_with("/self/write")
            .then(|| json!({"status_for_the_stand_in": 400, "body_for_the_stand_in": {"error": {"code": "self_no_room", "slot": 1, "tokens": 30000, "cap": 20000}}}))
    });
    let ev = json!(["e1", "e2"]);
    let merge = json!({"notes": [], "edits": [{"file": "1", "op": "remake", "title": "Garden", "text": "## Roses\nDana planted roses.", "evidence": ev}]});
    let mem = Client::with_base_url(KEY, &base).with_retries(0);
    let res = mem.sleep("u", stand_in(read_dana(), merge), &sleep_opts()).await.unwrap();
    let why: Vec<&str> = res["refused"].as_array().unwrap().iter().filter_map(|r| r["why"].as_str()).collect();
    assert!(why.iter().any(|w| w.starts_with("no room: the service counts it over its cap")), "{res}");
    assert_eq!(res["rooms"], json!([{"file": 1, "before": 7, "after": null}]), "{res}");
    assert_eq!(seen.lock().unwrap().iter().filter(|(p, _)| p.ends_with("/self/write")).count(), 1);
}

#[tokio::test]
async fn a_proposal_the_being_makes_again_counts_one_more_sleep_and_one_made_twice_in_a_night_counts_once() {
    let (base, seen) = sleep_service(|path, _| {
        if path.ends_with("/sleep") {
            let mut page = night();
            page["memories"].as_array_mut().unwrap().push(
                json!({"id": "m-3", "content": "I think roses suit Dana.", "created_at": "2026-10-19T09:02:00Z", "speaker": "Kai", "tokens": 7}),
            );
            Some(page)
        } else if path.ends_with("/self/notes") {
            let held = |id: Value, text: &str, sleeps: Value, span_end: &str| {
                json!({"id": id, "text": text, "sleeps": sleeps, "first": 1_792_000_000.0, "first_day": "2026-10-14 09:00", "span_end": span_end})
            };
            Some(json!({"notes": {"proposals": [
                held(json!("p1"), "Kai likes roses.", json!(2.0), "m-0"),
                held(json!(5), "Kai is calm.", json!(1), "m-3"),
                held(json!(true), "Kai is kind.", json!(1), "m-3")
            ], "mixed": "not a list"}, "version": 3}))
        } else {
            None
        }
    });
    let read = json!({"items": [{"kind": "person", "topic": "Dana", "text": "Dana planted roses.", "evidence": ["e1", "e2"]}], "mixed": []});
    let mine = json!(["e3"]);
    let merge = json!({"notes": [], "edits": [], "proposals": [
        {"text": "Kai likes roses a lot.", "evidence": mine, "same_as": "p1"},
        {"text": "Kai is calm.", "evidence": mine},
        {"text": "Kai is calm.", "evidence": mine},
        {"text": "Kai is kind.", "evidence": mine},
        {"text": "Kai is kind.", "evidence": mine}
    ]});
    let mem = Client::with_base_url(KEY, &base).with_retries(0);
    mem.sleep("u", stand_in(read, merge), &sleep_opts()).await.unwrap();
    let seen = seen.lock().unwrap();
    let (_, wrote) = seen.iter().find(|(p, _)| p.ends_with("/self/notes/write")).expect("the notes are written");
    let held: Vec<(Value, Value)> = wrote["notes"]["proposals"].as_array().unwrap().iter().map(|p| (p["id"].clone(), p["sleeps"].clone())).collect();
    assert_eq!(held, [(json!("p1"), json!(3)), (json!(5), json!(1)), (json!(true), json!(1))], "{wrote}");
}

#[tokio::test]
async fn a_core_over_its_cap_moves_what_its_room_names_and_a_box_a_move_overfills_stays_as_it_was() {
    let (base, _) = sleep_service(|path, _| {
        if path.ends_with("/self/open") {
            return Some(json!({"slot": 1, "title": "Garden", "version": 3, "tokens": 9, "day": "2026-10-01", "body": "## Roses\n- Dana grows big roses."}));
        }
        (!path.ends_with("/sleep") && !path.contains("/self/") && !path.contains("/done")).then(|| {
            let mut me = usual(path);
            me["core"] = json!({"title": "Core", "version": 1, "tokens": 9, "written_at": null, "day": "2026-10-01",
                "body": "## Dana\n- Dana likes tea.\n- Dana lives by the sea.\n- Dana has a cat.\n## Mo\n- Mo is Dana's brother."});
            me["files"][0] = json!({"slot": 1, "title": "Garden", "version": 3, "tokens": 9, "written_at": null, "day": "2026-10-01"});
            me
        })
    });
    let users: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = users.clone();
    let llm = caller_fn(move |c| {
        seen.lock().unwrap().push(c.user.clone());
        let ev = json!(["e1", "e2"]);
        let said = if c.user.contains("<items>") {
            json!({"notes": [], "edits": [{"file": "core", "op": "rewrite", "section": "Dana", "text": "- Dana gardens.", "evidence": ev}]})
        } else if c.user.contains("<memories>") {
            read_dana()
        } else if c.user.contains("<as it stood>") {
            json!({"add": "not a list", "op": "none", "drop": [], "text": ""})
        } else if c.user.starts_with("<core>") {
            json!({"text": "## Dana\n- Dana gardens.", "moved": [{"section": "Mo", "to": "1"}, {"section": "Nobody", "to": "2"}, {"section": "Dana", "to": "9"}, "x"]})
        } else {
            json!({"add": [], "op": "none", "drop": [], "text": ""})
        };
        async move { Ok(ModelReply::new(said.to_string())) }
    });
    let count: super::CountTokens = Arc::new(|s: &str| {
        if s.contains("Mo is") {
            80_000
        } else if s.contains("big roses") {
            30_000
        } else {
            (s.len() / 4) as u64
        }
    });
    let opts = super::SleepOpts { count_tokens: Some(count), ..sleep_opts() };
    let res = Client::with_base_url(KEY, &base).with_retries(0).sleep("u", llm, &opts).await.unwrap();
    let why: Vec<&str> = res["refused"].as_array().unwrap().iter().filter_map(|r| r["why"].as_str()).collect();
    assert!(why.contains(&"over its cap after a move; the box stays as it was"), "{res}");
    assert!(users.lock().unwrap().iter().any(|u| u.contains("<as it stood>")), "no settle call: {res}");
}

#[tokio::test]
async fn a_reader_that_numbers_its_part_from_e1_is_renumbered_and_one_past_its_part_is_left_as_written() {
    let (base, _) = sleep_service(|path, _| {
        path.ends_with("/sleep").then(|| {
            let mut page = night();
            let rows: Vec<Value> = (1..=4)
                .map(|i| json!({"id": format!("m-{i}"), "content": format!("Dana said thing {i}."), "created_at": format!("2026-10-19T09:0{i}:00Z"), "speaker": "Dana", "tokens": 60_000}))
                .collect();
            page["memories"] = json!(rows);
            page
        })
    });
    let llm = caller_fn(|c| {
        let said = if c.user.contains("e2") {
            json!({"items": [{"kind": "person", "topic": "Dana", "text": "Dana said thing 2.", "evidence": ["e5"]}], "mixed": []})
        } else if c.user.contains("e3") {
            json!({"items": [{"kind": "person", "topic": "Dana", "text": "Dana said thing 3.", "evidence": ["e1"]}, {"text": "no evidence"}], "mixed": {"k": 1}})
        } else if c.user.contains("e4") {
            json!({"items": "none", "mixed": [{"kind": "echo", "line": "e1", "source": null}]})
        } else if c.user.contains("<memories>") {
            json!({"items": [], "mixed": []})
        } else {
            json!({"edits": [], "notes": [], "add": [], "op": "none", "drop": [], "text": ""})
        };
        async move { Ok(ModelReply::new(said.to_string())) }
    });
    let res = Client::with_base_url(KEY, &base).with_retries(0).sleep("u", llm, &sleep_opts()).await.unwrap();
    let renumbered: Vec<&Value> = res["log"].as_array().unwrap().iter().filter(|x| x["renumbered"] == true).collect();
    assert_eq!(
        renumbered,
        [&json!({"call": "read 3", "renumbered": true, "part": ["e3", "e3"]}), &json!({"call": "read 4", "renumbered": true, "part": ["e4", "e4"]})],
        "{res}"
    );
}

fn endless(page: fn(usize) -> Value) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let served = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    std::thread::spawn(move || {
        for sock in listener.incoming() {
            let Ok(mut sock) = sock else { continue };
            let served = served.clone();
            std::thread::spawn(move || {
                let mut buf: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
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
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    buf.drain(..head_end + len);
                    let body = page(served.fetch_add(1, Ordering::SeqCst)).to_string();
                    let out = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
                    if sock.write_all(out.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    base
}

fn stopped_after_every_page(e: impl std::fmt::Display) {
    assert_eq!(e.to_string(), "[200] stopped after 20000 pages — the store did not end. This is a truncated answer, not the whole store.");
}

#[tokio::test]
async fn a_memory_walk_that_never_ends_stops_after_20000_pages() {
    let base = endless(|n| json!({"memories": [{"id": format!("m{n}"), "content": "x"}], "next_cursor": format!("c{n}")}));
    stopped_after_every_page(wos(&base).list_all_memories("u").await.unwrap_err());
}

#[tokio::test]
async fn an_image_walk_that_never_ends_stops_after_20000_pages() {
    let base = endless(|n| json!({"images": [{"id": format!("i{n}")}], "has_more": true, "next_before": format!("b{n}")}));
    stopped_after_every_page(wos(&base).list_all_images("u", None).await.unwrap_err());
}

#[tokio::test]
async fn a_sleep_reading_that_never_ends_stops_after_20000_pages() {
    let base = endless(|n| {
        json!({"user_id": "u", "sleep_id": "s-1", "exhausted": false, "next_cursor": format!("c{n}"), "memories": [
            {"id": format!("m-{n}"), "content": "Dana planted roses.", "created_at": "2026-10-19T09:00:00Z", "speaker": "Dana", "tokens": 5}
        ]})
    });
    let e = wos(&base).sleep("u", stand_in(read_dana(), json!({})), &sleep_opts()).await.unwrap_err();
    assert_eq!(e.to_string(), "[200] the sleep's reading passed its page limit. This is a truncated answer, not the whole store.");
}

#[tokio::test(flavor = "current_thread")]
async fn a_box_listed_with_version_0_and_a_blank_body_is_empty() {
    let listed = json!({"slot": 1, "title": "", "version": 0, "tokens": 0, "written_at": null, "day": null, "body": " \n"});
    let (base, seen) = scripted(vec![(200, &files(Some("2026-10-01"), listed))]);
    let c = Client::with_base_url(KEY, &base).with_model("scroll-2");
    let tools = c.memory_tools_preset("u", None, "S-2").unwrap().with_sleep(true).with_today("2026-10-20").unwrap();
    assert_eq!(tools.run("open_box", &json!({"box": "1"})).await.unwrap(), "Box 1 is empty.");
    assert_eq!(seen.lock().unwrap().len(), 1);
}
