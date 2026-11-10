use super::*;
use crate::{anthropic_caller, AnthropicCallerOpts};

fn fix() -> Value {
    serde_json::from_str(include_str!("../../testdata/sleep_attacks.json")).unwrap()
}

fn expand(v: &Value) -> Value {
    match v {
        Value::Object(o) if o.contains_key("$repeat") => {
            let prefix = o.get("prefix").and_then(Value::as_str).unwrap_or("");
            let times = o["times"].as_u64().unwrap() as usize;
            Value::String(format!("{prefix}{}", o["$repeat"].as_str().unwrap().repeat(times)))
        }
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), expand(x))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(expand).collect()),
        Value::String(t) => Value::String(t.replace("@@ASSISTANT@@", "Kai")),
        other => other.clone(),
    }
}

fn answer_for(fix: &Value, answers: &Value, system: &str, user: &str) -> String {
    let mut a = fix["defaults"]["answers"].as_object().unwrap().clone();
    a.extend(answers.as_object().unwrap().clone());
    let got = if system.contains(" you read part ") {
        let k: usize = between(system, "you read part ", " of").parse().unwrap();
        a.get("read_parts").map_or_else(|| a["read"].clone(), |p| p[k - 1].clone())
    } else if system.contains("Several readers each read one part") {
        a.get("combine").cloned().unwrap_or_else(|| json!({"items": serde_json::from_str::<Value>(user).unwrap()}))
    } else if system.contains(", finishing.") {
        a["merge"].clone()
    } else if system.contains("Tonight you rewrote how") {
        a["settle"].clone()
    } else if system.contains("lines that are only a note of doubt") {
        a["tidy"].clone()
    } else if system.contains("has grown:") {
        a["fold"].clone()
    } else if system.contains("there is no room") {
        a["room"].clone()
    } else {
        panic!("a call the fixture does not script: {}", system.chars().take(80).collect::<String>());
    };
    match expand(&got) {
        Value::String(s) => s,
        v => v.to_string(),
    }
}

struct FixtureModel {
    fix: Value,
    answers: Value,
    base: Option<String>,
    calls: Mutex<Vec<(String, String)>>,
}

impl FixtureModel {
    fn new(fix: &Value, answers: &Value, base: Option<String>) -> Arc<Self> {
        Arc::new(FixtureModel { fix: fix.clone(), answers: answers.clone(), base, calls: Mutex::new(Vec::new()) })
    }

    fn called(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl ModelCaller for FixtureModel {
    fn call<'a>(&'a self, c: &'a ModelCall) -> BoxFuture<'a, Result<ModelReply, ModelCallError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push((c.system[0].clone(), c.user.clone()));
            let text = answer_for(&self.fix, &self.answers, &c.system[0], &c.user);
            let usage = json!({"input_tokens": sleep_tokens(&c.user), "output_tokens": sleep_tokens(&text)});
            Ok(ModelReply { text, stop: "end".into(), usage: Some(usage) })
        })
    }

    fn base_url(&self) -> Option<&str> {
        self.base.as_deref()
    }
}

fn night_rows(n: usize, night: &Value) -> Vec<Value> {
    let day = &night["today"].as_str().unwrap()[..10];
    night["memories"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let text = m[1].as_str().unwrap();
            json!({"id": format!("m{}-{}", n + 1, i + 1), "content": text, "created_at": format!("{day}T09:{i:02}:00Z"),
                   "speaker": m.get(2).and_then(Value::as_str).unwrap_or(if m[0] == "user" { "Ana" } else { "Kai" }),
                   "tokens": sleep_tokens(text)})
        })
        .collect()
}

fn section(body: &str, heading: &str) -> Option<String> {
    let mut out: Vec<(String, Vec<&str>)> = Vec::new();
    for ln in body.split('\n') {
        if let Some(h) = ln.strip_prefix("## ") {
            out.push((h.trim().to_string(), Vec::new()));
        } else if let Some(last) = out.last_mut() {
            last.1.push(ln);
        }
    }
    out.into_iter().find(|(h, _)| h == heading).map(|(_, b)| b.join("\n").trim().to_string())
}

fn num(v: &Value) -> String {
    match v.as_f64() {
        Some(f) if f.fract() == 0.0 => format!("{}", f as i64),
        _ => text_of(v),
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn label(e: &Value) -> String {
    let named = [&e["section"], &e["title"]].into_iter().map(text_of).find(|s| !s.is_empty()).unwrap_or_default();
    let head = format!("{} {}", text_of(&e["file"]), text_of(&e["op"]));
    if named.is_empty() {
        head
    } else {
        format!("{head} {named}")
    }
}

fn settings_of(fix: &Value, scenario: &Value) -> SleepSettings {
    let mut s = SleepSettings::default();
    for given in [&fix["defaults"]["settings"], &scenario["settings"]] {
        for (k, v) in given.as_object().into_iter().flatten() {
            let v = v.as_u64().unwrap();
            match k.as_str() {
                "core_tokens" => s.core_tokens = v,
                "box_tokens" => s.box_tokens = v,
                "part_tokens" => s.part_tokens = v,
                "readers" => s.readers = v,
                "group" => s.group = v,
                "sleepy_tokens" => s.sleepy_tokens = v,
                other => panic!("a setting the fixture names and the test does not know: {other}"),
            }
        }
    }
    s
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map_or_else(Vec::new, |a| a.iter().map(text_of).collect())
}

fn served() -> (Svc, Client, String) {
    let svc = Arc::new(Mutex::new(Service::default()));
    let base = serve(svc.clone());
    let mem = Client::with_base_url(KEY, &base);
    (svc, mem, base)
}

#[allow(clippy::too_many_arguments)]
fn check_night(
    scenario: &Value, n: usize, night: &Value, res: &Value, svc: &Svc, start: usize, reads: &[String], calls: &[String],
) {
    let where_ = format!("{} night {}", scenario["name"].as_str().unwrap(), n + 1);
    let want = &night["expect"];
    for s in strings(&want["read_has"]) {
        assert!(reads.iter().any(|r| r.contains(&s)), "{where_}: a reader was sent {s:?}");
    }
    for s in strings(&want["read_lacks"]) {
        assert!(!reads.iter().any(|r| r.contains(&s)), "{where_}: no reader was sent {s:?}");
    }
    for s in strings(&want["calls_have"]) {
        assert!(calls.iter().any(|c| c.contains(&s)), "{where_}: a model call held {s:?}");
    }
    for s in strings(&want["calls_lack"]) {
        assert!(!calls.iter().any(|c| c.contains(&s)), "{where_}: no model call held {s:?}");
    }
    let st = svc.lock().unwrap();
    let latest: Vec<String> = (0..5).filter_map(|k| st.latest(k).map(|v| v.body.clone())).collect();
    for s in strings(&want["files_have"]) {
        assert!(latest.iter().any(|f| f.contains(&s)), "{where_}: a file holds {s:?}");
    }
    let core = st.latest(0).map(|v| v.body.clone()).unwrap_or_default();
    let files: Vec<String> = st.slots.iter().flatten().map(|v| format!("{}\n{}", v.body, v.title)).collect();
    let notes = st.notes.clone().unwrap_or(json!({})).to_string();
    let has = |k: &str| want.get(k).is_some();
    if has("refused") {
        let got: Vec<String> = res["refused"].as_array().unwrap().iter().map(|r| text_of(&r["why"])).collect();
        assert_eq!(got, strings(&want["refused"]), "{where_}");
    }
    if has("applied") {
        let got: Vec<String> = res["applied"].as_array().unwrap().iter().map(label).collect();
        assert_eq!(got, strings(&want["applied"]), "{where_}");
    }
    if has("items") {
        let got: Vec<String> = res["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|it| format!("{} {} {}", text_of(&it["kind"]), text_of(&it["topic"]), strings(&it["evidence"]).join(",")))
            .collect();
        assert_eq!(got, strings(&want["items"]), "{where_}");
    }
    if has("dropped") {
        assert_eq!(res["items_dropped_by_check"], want["dropped"], "{where_}");
    }
    if has("me") {
        assert_eq!(section(&core, "Me").map(Value::String).unwrap_or(Value::Null), want["me"], "{where_}");
    }
    for s in strings(&want["core_has"]) {
        assert!(core.contains(&s), "{where_}: the core has {s:?}");
    }
    for s in strings(&want["core_lacks"]) {
        assert!(!core.contains(&s), "{where_}: the core lacks {s:?}");
    }
    for s in strings(&want["files_lack"]) {
        assert!(files.iter().all(|f| !f.contains(&s)), "{where_}: no file has {s:?}");
    }
    if has("mixed") {
        let got: Vec<String> = res["mixed"].as_array().unwrap().iter().map(|m| text_of(&m["kind"])).collect();
        assert_eq!(got, strings(&want["mixed"]), "{where_}");
    }
    for s in strings(&want["notes_have"]) {
        assert!(notes.contains(&s), "{where_}: the notes have {s:?}");
    }
    for s in strings(&want["notes_lack"]) {
        assert!(!notes.contains(&s), "{where_}: the notes lack {s:?}");
    }
    if has("proposals") {
        let got: Vec<String> = res["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let applied = text_of(&p["applied"]);
                format!("{} {} {}", text_of(&p["id"]), num(&p["sleeps"]), if applied.is_empty() { "-".into() } else { applied })
            })
            .collect();
        assert_eq!(got, strings(&want["proposals"]), "{where_}");
    }
    if has("ripe") {
        assert_eq!(res["ripe"], want["ripe"], "{where_}");
    }
    if has("written") {
        let mut got: Vec<u64> = res["written"].as_object().unwrap().keys().map(|k| k.parse().unwrap()).collect();
        got.sort_unstable();
        assert_eq!(json!(got), want["written"], "{where_}");
    }
    if has("rooms") {
        let got: Vec<String> = res["rooms"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| format!("{} {}", num(&r["file"]), if r["after"].is_null() { "kept" } else { "after" }))
            .collect();
        assert_eq!(got, strings(&want["rooms"]), "{where_}");
    }
    let handed: HashSet<String> = night_rows(n, night).iter().map(|r| text_of(&r["id"])).collect();
    for x in &st.seen[start..] {
        if x.path.ends_with("/self/write") {
            for w in x.body["writes"].as_array().unwrap() {
                for e in strings(&w["evidence"]) {
                    assert!(handed.contains(&e), "{where_}: {e} was handed out");
                }
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn planted_instructions_meet_the_rules_as_the_fixture_says() {
    let fix = fix();
    assert_eq!(fix["sleep_version"], SLEEP_VERSION, "the fixture is for this sleep");
    for scenario in fix["scenarios"].as_array().unwrap() {
        let (svc, mem, _) = served();
        if let Some(p) = scenario["before"].get("proposals") {
            svc.lock().unwrap().notes = Some(json!({"proposals": p, "mixed": []}));
        }
        if let Some(files) = scenario["before"].get("files").and_then(Value::as_object) {
            for (k, f) in files {
                let body = text_of(&f["body"]);
                svc.lock().unwrap().slots[k.parse::<usize>().unwrap()].push(Version {
                    version: 1,
                    title: f.get("title").map(text_of).unwrap_or_default(),
                    tokens: sleep_tokens(&body) as i64,
                    body,
                    evidence: json!([]),
                    day: f.get("day").cloned().unwrap_or(Value::Null),
                    sleep_id: Value::Null,
                });
            }
        }
        for (n, night) in scenario["nights"].as_array().unwrap().iter().enumerate() {
            give(&svc, night_rows(n, night));
            let start = svc.lock().unwrap().seen.len();
            let model = FixtureModel::new(&fix, &night["answers"], None);
            let opts = SleepOpts {
                prompt: text_of(&scenario["prompt"]),
                today: Some(text_of(&night["today"])),
                settings: settings_of(&fix, scenario),
                ..Default::default()
            };
            let res = mem.sleep("u", model.clone(), &opts).await.unwrap_or_else(|e| panic!("{}: {e}", scenario["name"]));
            let reads: Vec<String> =
                model.calls.lock().unwrap().iter().filter(|(s, _)| s.contains(" you read part ")).map(|(_, u)| u.clone()).collect();
            let calls: Vec<String> = model.calls.lock().unwrap().iter().map(|(s, u)| format!("{s}\n{u}")).collect();
            check_night(scenario, n, night, &res, &svc, start, &reads, &calls);
        }
    }
}

#[test]
fn the_rule_of_the_by_name_scenario_gives_the_length_each_match_needs() {
    let fix = fix();
    let scenarios = fix["scenarios"].as_array().unwrap();
    let scenario = scenarios.iter().find(|s| s["name"] == "a-person-is-spoken-of-by-name-not-by-a-piece-of-one").unwrap();
    let rule = text_of(&scenario["rule"]);
    for says in [
        "its name with something joined to its end when the name has two letters or more",
        "the first three letters or more of its name",
        "a longer topic that starts with a one-letter name",
    ] {
        assert!(rule.contains(says), "the rule says {says:?}: {rule}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_caller_that_sends_its_key_to_wontopos_is_refused_before_anything_is_sent() {
    let fix = fix();
    let night = &fix["provider_night"];
    for c in fix["provider_hosts"].as_array().unwrap() {
        let refused = c["refused"].as_bool().unwrap();
        for slot in ["llm", "merge_llm"] {
            let (svc, mem, base) = served();
            give(&svc, night_rows(0, night));
            let url = if c["url"] == "$client" { base.clone() } else { text_of(&c["url"]) };
            let where_ = format!("{url} as {slot}");
            let hostile = FixtureModel::new(&fix, &night["answers"], Some(url));
            let plain = FixtureModel::new(&fix, &night["answers"], None);
            let mut opts = SleepOpts { today: Some(text_of(&night["today"])), ..Default::default() };
            let h: Arc<dyn ModelCaller> = hostile.clone();
            let llm: Arc<dyn ModelCaller> = if slot == "llm" {
                h
            } else {
                opts.merge_llm = Some(h);
                plain.clone() as Arc<dyn ModelCaller>
            };
            let r = mem.sleep("u", llm, &opts).await;
            if refused {
                let message = wos(r).to_string();
                assert!(message.contains("Wontopos host") || message.contains("this client's base URL"), "{where_}: {message}");
                assert!(paths(&svc).is_empty(), "{where_}: nothing was sent");
                assert_eq!(hostile.called() + plain.called(), 0, "{where_}: no model was called");
            } else {
                let res = r.unwrap_or_else(|e| panic!("{where_}: {e}"));
                assert_eq!(res["written"], json!({"0": 1}), "{where_}");
                assert!(hostile.called() > 0, "{where_}");
            }
        }
        if c["url"] != "$client" {
            let url = text_of(&c["url"]);
            let key = text_of(&fix["keys"]["provider"]);
            let made = [
                anthropic_caller(AnthropicCallerOpts { api_key: key.clone(), base_url: Some(url.clone()), ..Default::default() }).err(),
                openai_caller(OpenAiCallerOpts { base_url: url.clone(), model: "m".into(), api_key: Some(key), ..Default::default() }).err(),
            ];
            for e in made {
                match (refused, e) {
                    (true, Some(e)) => assert!(e.to_string().contains("Wontopos host"), "{url}: {e}"),
                    (false, None) => {}
                    (_, e) => panic!("{url}: refused {refused}, the caller answered {e:?}"),
                }
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_caller_that_sends_its_key_where_this_client_sends_its_own_is_refused() {
    let fix = fix();
    let night = &fix["provider_night"];
    for c in fix["client_bases"].as_array().unwrap() {
        for slot in ["llm", "merge_llm"] {
            let mem = Client::with_base_url(KEY, &text_of(&c["client"]));
            let where_ = format!("{} as {slot}", c["url"]);
            let hostile = FixtureModel::new(&fix, &night["answers"], Some(text_of(&c["url"])));
            let plain = FixtureModel::new(&fix, &night["answers"], None);
            let mut opts = SleepOpts { today: Some(text_of(&night["today"])), ..Default::default() };
            let h: Arc<dyn ModelCaller> = hostile.clone();
            let llm: Arc<dyn ModelCaller> = if slot == "llm" {
                h
            } else {
                opts.merge_llm = Some(h);
                plain.clone() as Arc<dyn ModelCaller>
            };
            let message = wos(mem.sleep("u", llm, &opts).await).to_string();
            assert!(message.contains("this client's base URL"), "{where_}: {message}");
            assert_eq!(hostile.called() + plain.called(), 0, "{where_}: no model was called");
        }
    }
}

#[derive(Clone)]
struct Heard {
    headers: HashMap<String, String>,
    raw: String,
}

#[derive(Clone)]
enum Answer {
    Script,
    Fail(u16, Value),
    Redirect(String),
    Raw(String),
}

fn provider(fix: &Value, answers: &Value, anthropic: bool, how: Answer) -> (Arc<Mutex<Vec<Heard>>>, Arc<dyn ModelCaller>) {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let key = text_of(&fix["keys"]["provider"]);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (fix, answers, kept) = (fix.clone(), answers.clone(), heard.clone());
    std::thread::spawn(move || {
        while let Ok((mut sock, _)) = listener.accept() {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 65536];
            let head_end = loop {
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(i + 4);
                }
                match sock.read(&mut chunk) {
                    Ok(0) | Err(_) => break None,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            };
            let Some(head_end) = head_end else { continue };
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let mut headers = HashMap::new();
            for line in head.lines().skip(1) {
                if let Some((k, v)) = line.split_once(':') {
                    headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                }
            }
            let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
            while buf.len() < head_end + len {
                match sock.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
            kept.lock().unwrap().push(Heard { headers: headers.clone(), raw: format!("{body}{head}") });
            let (status, payload, extra) = match &how {
                Answer::Redirect(to) => (307, json!({}), format!("Location: {to}\r\n")),
                Answer::Fail(status, payload) => (*status, payload.clone(), String::new()),
                Answer::Script | Answer::Raw(_) => {
                    let b: Value = serde_json::from_str(&body).unwrap();
                    let (system, user) = if anthropic {
                        let system: String = b["system"].as_array().unwrap().iter().map(|x| text_of(&x["text"])).collect();
                        (system, text_of(&b["messages"][0]["content"]))
                    } else {
                        (text_of(&b["messages"][0]["content"]), text_of(&b["messages"][1]["content"]))
                    };
                    let text = match &how {
                        Answer::Raw(t) => t.clone(),
                        _ => answer_for(&fix, &answers, &system, &user),
                    };
                    let (i, o) = (sleep_tokens(&user), sleep_tokens(&text));
                    let payload = if anthropic {
                        json!({"content": [{"type": "text", "text": text}], "stop_reason": "end_turn",
                               "usage": {"input_tokens": i, "output_tokens": o}})
                    } else {
                        json!({"choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
                               "usage": {"prompt_tokens": i, "completion_tokens": o}})
                    };
                    (200, payload, String::new())
                }
            };
            let payload = payload.to_string();
            let _ = sock.write_all(
                format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{payload}",
                    payload.len()
                )
                .as_bytes(),
            );
        }
    });
    let caller: Arc<dyn ModelCaller> = if anthropic {
        let opts = AnthropicCallerOpts { api_key: key, model: Some("m".into()), base_url: Some(base), ..Default::default() };
        Arc::new(anthropic_caller(opts).unwrap()) as Arc<dyn ModelCaller>
    } else {
        let opts = OpenAiCallerOpts { base_url: format!("{base}/v1"), model: "m".into(), api_key: Some(key), ..Default::default() };
        Arc::new(openai_caller(opts).unwrap()) as Arc<dyn ModelCaller>
    };
    (heard, caller)
}

fn assert_quiet(fix: &Value, progress: &[String], log: &[Value], errors: &[String]) {
    let night = &fix["provider_night"];
    let said = progress.iter().cloned().chain(log.iter().map(Value::to_string)).chain(errors.iter().cloned());
    let secrets = [text_of(&fix["keys"]["provider"]), text_of(&fix["keys"]["wontopos"]), text_of(&night["memory_marker"]),
                   text_of(&night["answer_marker"])];
    for s in said {
        for secret in &secrets {
            assert!(!s.contains(secret.as_str()), "{secret:?} in {s:?}");
        }
    }
}

fn hearing() -> (Arc<Mutex<Vec<String>>>, SleepProgress) {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let kept = heard.clone();
    (heard, Arc::new(move |line: &str| kept.lock().unwrap().push(line.to_string())))
}

#[tokio::test(flavor = "current_thread")]
async fn the_provider_key_goes_to_the_provider_and_the_wontopos_key_to_wontopos() {
    let fix = fix();
    let night = &fix["provider_night"];
    let (provider_key, key) = (text_of(&fix["keys"]["provider"]), text_of(&fix["keys"]["wontopos"]));
    for anthropic in [true, false] {
        let (svc, mem, _) = served();
        let (seen, caller) = provider(&fix, &night["answers"], anthropic, Answer::Script);
        give(&svc, night_rows(0, night));
        let (heard, progress) = hearing();
        let opts = SleepOpts { today: Some(text_of(&night["today"])), progress: Some(progress), ..Default::default() };
        let res = mem.sleep("u", caller, &opts).await.unwrap();
        assert_eq!(res["written"], json!({"0": 1}));
        let seen = seen.lock().unwrap().clone();
        assert!(seen.len() >= 3, "read, merge and tidy went to the provider");
        for x in &seen {
            if anthropic {
                assert_eq!(x.headers.get("x-api-key"), Some(&provider_key));
                assert!(!x.headers.contains_key("authorization"));
            } else {
                assert_eq!(x.headers.get("authorization"), Some(&format!("Bearer {provider_key}")));
                assert!(!x.headers.contains_key("x-api-key"));
            }
            assert!(!x.raw.contains(&key), "the Wontopos key never goes to the provider");
        }
        let st = svc.lock().unwrap();
        assert!(!st.seen.is_empty());
        for x in &st.seen {
            assert_eq!(x.headers.get("x-api-key"), Some(&key));
            let all = format!("{:?}{}", x.headers, x.body);
            assert!(!all.contains(&provider_key), "the provider key never goes to Wontopos");
        }
        let heard = heard.lock().unwrap().clone();
        assert_quiet(&fix, &heard, res["log"].as_array().unwrap(), &[]);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_redirect_from_the_provider_is_not_followed_with_the_key() {
    let fix = fix();
    let night = &fix["provider_night"];
    let provider_key = text_of(&fix["keys"]["provider"]);
    let (svc, mem, base) = served();
    let (seen, caller) = provider(&fix, &night["answers"], true, Answer::Redirect(format!("{base}/api/v1/memory/self")));
    give(&svc, night_rows(0, night));
    let opts = SleepOpts { today: Some(text_of(&night["today"])), ..Default::default() };
    match mem.sleep("u", caller, &opts).await {
        Err(SleepError::Model(e)) => assert!(e.message.contains("redirect refused"), "{e}"),
        other => panic!("expected the provider's redirect refused, got {other:?}"),
    }
    assert_eq!(seen.lock().unwrap().len(), 1, "not tried again");
    let st = svc.lock().unwrap();
    for x in &st.seen {
        assert!(!format!("{:?}{}", x.headers, x.body).contains(&provider_key));
        assert!(!x.path.ends_with("/self/write"));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn progress_the_log_and_errors_carry_no_key_and_no_memory_outside_a_failed_answer() {
    let fix = fix();
    let night = &fix["provider_night"];
    let today = Some(text_of(&night["today"]));
    let (svc, mem, _) = served();
    let fail = json!({"error": {"type": "invalid_request_error", "message": "bad request"}});
    let (_, caller) = provider(&fix, &night["answers"], true, Answer::Fail(400, fail));
    give(&svc, night_rows(0, night));
    let (heard, progress) = hearing();
    let opts = SleepOpts { today: today.clone(), progress: Some(progress), ..Default::default() };
    let e = match mem.sleep("u", caller, &opts).await {
        Err(e @ SleepError::Model(_)) => e,
        other => panic!("expected the provider's refusal, got {other:?}"),
    };
    assert_quiet(&fix, &heard.lock().unwrap(), &[], &[e.to_string(), format!("{e:?}")]);
    let (svc, mem, _) = served();
    let (_, caller) = provider(&fix, &night["answers"], true, Answer::Script);
    give(&svc, night_rows(0, night));
    svc.lock().unwrap().fail.push((
        "/api/v1/memory/self/write".into(),
        400,
        json!({"error": {"code": "self_evidence_unknown", "message": "evidence the sleep was not handed"}}),
    ));
    let (heard, progress) = hearing();
    let opts = SleepOpts { today: today.clone(), progress: Some(progress), ..Default::default() };
    let e = match mem.sleep("u", caller, &opts).await {
        Err(e @ SleepError::Wos(_)) => e,
        other => panic!("expected the service's refusal, got {other:?}"),
    };
    assert_quiet(&fix, &heard.lock().unwrap(), &[], &[e.to_string(), format!("{e:?}")]);
    let (svc, mem, _) = served();
    let unparsable = text_of(&night["unparsable"]);
    let (_, caller) = provider(&fix, &night["answers"], true, Answer::Raw(unparsable.clone()));
    give(&svc, night_rows(0, night));
    let (heard, progress) = hearing();
    let opts = SleepOpts { today, progress: Some(progress), ..Default::default() };
    let res = mem.sleep("u", caller, &opts).await.unwrap();
    let log = res["log"].as_array().unwrap();
    let (failed, rest): (Vec<Value>, Vec<Value>) = log.iter().cloned().partition(|x| x["failed"] == true);
    let calls: Vec<String> = failed.iter().map(|x| text_of(&x["call"])).collect();
    assert_eq!(calls, ["read 1", "read 1 again 2", "read 1 again 3"]);
    assert!(failed.iter().all(|x| x["head"] == unparsable.as_str()));
    assert_quiet(&fix, &heard.lock().unwrap(), &rest, &[]);
    for x in &failed {
        for k in ["provider", "wontopos"] {
            assert!(!x.to_string().contains(&text_of(&fix["keys"][k])));
        }
    }
}

#[test]
fn the_texts_as_the_publish_gate_reads_them() {
    let texts = json!({
        "SLEEP_VERSION": SLEEP_VERSION,
        "_SLEEP_DIGEST": SLEEP_DIGEST,
        "_SLEEP_COMBINE": SLEEP_COMBINE,
        "_SLEEP_MERGE": SLEEP_MERGE,
        "_SLEEP_SHORTER": SLEEP_SHORTER,
        "_SLEEP_SELF_RULE": SLEEP_SELF_RULE,
        "_SLEEP_SETTLE": SLEEP_SETTLE,
        "_SLEEP_FOLD_SECTION": SLEEP_FOLD_SECTION,
        "_SLEEP_TIDY": SLEEP_TIDY,
        "_SLEEP_ROOM": SLEEP_ROOM,
        "_SLEEP_NOT_JSON": SLEEP_NOT_JSON,
        "_SLEEP_READER_NOTE": SLEEP_READER_NOTE,
        "_SLEEP_OPEN_BOX": open_box_tool(),
        "_SLEEP_MANY_PEOPLE": {"digest": SLEEP_MANY_PEOPLE.digest, "merge": SLEEP_MANY_PEOPLE.merge},
    });
    if let Ok(path) = std::env::var("WONTOPOS_SLEEP_TEXTS") {
        std::fs::write(path, texts.to_string()).unwrap();
    }
}
