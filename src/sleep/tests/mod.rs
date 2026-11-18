use super::*;
use crate::{caller_fn, openai_caller, BoxFuture, ErrorKind, ModelReply, OpenAiCallerOpts, SleepFileWrite, SleepReadOpts, ToolFormat};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::Mutex;

const KEY: &str = "wos-test-xxxxxxxxxx";

fn sha256(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
        0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
        0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
        0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] =
        [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for (a, b) in h.iter_mut().zip(v) {
            *a = a.wrapping_add(b);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

fn py_dumps(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) if n.is_f64() => float_repr(n.as_f64().unwrap_or(0.0)),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).unwrap(),
        Value::Array(a) => format!("[{}]", a.iter().map(py_dumps).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            let fields: Vec<String> =
                keys.iter().map(|k| format!("{}: {}", serde_json::to_string(k).unwrap(), py_dumps(&o[*k]))).collect();
            format!("{{{}}}", fields.join(", "))
        }
    }
}

fn digest(v: &Value) -> String {
    sha256(py_dumps(v).as_bytes())
}

#[test]
fn sha256_is_sha256() {
    assert_eq!(sha256(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(sha256(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    let long = "a".repeat(1_000);
    assert_eq!(sha256(long.as_bytes()), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
}

const PROMPT: &str = "I am the assistant in these conversations with this user.\n## Style\nShort answers.";
const DAYS: [&str; 4] = ["2026-09-01", "2026-09-15", "2026-10-01", "2026-10-05"];

fn filler() -> String {
    "the garden path and the morning light ".repeat(46)
}

fn big() -> String {
    format!("- {}", "a long line of plant names ".repeat(1700))
}

fn settings() -> SleepSettings {
    SleepSettings { box_tokens: 10_000, part_tokens: 50_000, readers: 2, group: 2, ..Default::default() }
}

fn night_rows(n: usize, day: &str) -> Vec<Value> {
    (0..if n == 0 { 300 } else { 20 })
        .map(|i: usize| {
            let secs = 9 * 3600 + i * 20 + (i / 50) * 3600;
            let words = if n == 0 { filler() } else { "short words about the day".to_string() };
            let text = format!("Night {} line {i}: {words}", n + 1);
            json!({"id": format!("m{}-{i:04}", n + 1), "content": text,
                   "created_at": format!("{day}T{:02}:{:02}:{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60),
                   "speaker": (["Ana", "Kai"][i % 2]),
                   "tokens": sleep_tokens(&text)})
        })
        .collect()
}

const SUMMARY_KEYS: [&str; 16] = [
    "seeded", "readers", "combine_rounds", "items", "items_dropped_by_check", "applied", "refused", "written", "ripe",
    "notes", "folds", "settled", "deduped", "tidied", "rooms", "mixed",
];

fn night_summary(res: &Value) -> Value {
    Value::Object(SUMMARY_KEYS.iter().map(|k| (k.to_string(), res.get(*k).cloned().unwrap_or(Value::Null))).collect())
}

const REFERENCE_CALLS: usize = 27;
const REFERENCE_CALLS_DIGEST: &str = "66cc5bf1dbde699a22947dc8e1b55991ec0fd5f410bcf6bcc27f3f171144cf4a";
const REFERENCE_NIGHTS_DIGEST: &str = "bbedfa7a042245922190822e61443b41086ee03466b8f61da790cae7bb0184b7";
const REFERENCE_STATE_DIGEST: &str = "e3ebd0a08e7d1eae4e93b0a26bc7cdfec5c2cd5677d46e634ad367351117033b";

fn between<'a>(text: &'a str, from: &str, to: &str) -> &'a str {
    let rest = &text[text.find(from).unwrap() + from.len()..];
    &rest[..rest.find(to).unwrap()]
}

fn lines_of(user: &str) -> Vec<(String, String, String)> {
    between(user, "<memories>\n", "\n</memories>")
        .split('\n')
        .map(|ln| {
            let p: Vec<&str> = ln.splitn(4, " · ").collect();
            (p[0].to_string(), p[1].to_string(), p[2].to_string())
        })
        .collect()
}

fn section_in(user: &str) -> (String, String) {
    let heading = between(user, "<section heading=\"", "\">\n").to_string();
    let body = user.split_once('\n').unwrap().1;
    (heading, body[..body.rfind("\n</section>").unwrap()].to_string())
}

fn read_answer(day: &str, k: usize, lines: &[(String, String, String)], first: bool) -> String {
    let users: Vec<&str> = lines.iter().filter(|l| l.2 == "user").map(|l| l.0.as_str()).collect();
    let mine: Vec<&str> = lines.iter().filter(|l| l.2 != "user").map(|l| l.0.as_str()).collect();
    let v = if day == DAYS[0] {
        match k {
            1 => json!({"items": [
                {"kind": "person", "topic": "Jamie", "text": "Jamie is a close friend who likes gardening.",
                 "evidence": [users[0], users[1]]},
                {"kind": "work", "topic": "Garden project", "text": "The garden project is planting tomatoes.",
                 "evidence": [users[0], mine[0]]},
                {"kind": "self", "topic": "me", "text": "I like short answers.", "evidence": [mine[0], mine[1]]},
                {"kind": "self", "topic": "me", "text": "Not mine to say.", "evidence": [users[0], users[1]]},
                {"kind": "work", "topic": "Seeds", "text": "One line is not enough.", "evidence": [users[2]]},
                {"kind": "mood", "topic": "x", "text": "Not a kind.", "evidence": [users[0], users[1]]},
            ], "mixed": [
                {"line": users[1], "kind": "echo", "source": mine[0], "why": "gave back my words as theirs"},
                {"line": users[2], "kind": "planted", "why": "never said by me"},
                {"line": mine[2], "kind": "speaker", "why": "reads as the person's"},
                {"line": users[3], "kind": "wrong_fact", "why": "a mistake, not who said it"},
            ]}),
            2 => json!({"items": [{"kind": "person", "topic": "Jamie", "text": "Jamie waters the garden on weekends.",
                                   "evidence": ["e1", "e3"]}],
                        "mixed": [{"line": "e2", "kind": "speaker", "why": "reads as the person's"}]}),
            _ if first => return "I could not read this part.".into(),
            _ => json!({"items": [{"kind": "work", "topic": "Garden project", "text": "Tomatoes are planted; watering daily.",
                                   "evidence": [users[0], mine[0]]}]}),
        }
    } else if day == DAYS[1] {
        json!({"items": [
            {"kind": "person", "topic": "Jamie", "text": "Jamie prefers flowers now.", "evidence": [users[0], users[1]]},
            {"kind": "person", "topic": "Alex", "text": "Alex is Jamie's sibling.", "evidence": [users[0], users[2]]},
            {"kind": "work", "topic": "Garden project", "text": "The fence is built.", "evidence": [users[1], mine[0]]},
            {"kind": "self", "topic": "me", "text": "I keep answers short.", "evidence": [mine[0], mine[1]]},
        ], "mixed": []})
    } else if day == DAYS[2] {
        json!({"items": [
            {"kind": "person", "topic": "Alex", "text": "Alex visits every month.", "evidence": [users[0], users[1]]},
            {"kind": "work", "topic": "Big list", "text": "A long list of plants.", "evidence": [users[1], users[2]]},
            {"kind": "self", "topic": "me", "text": "I still keep answers short.", "evidence": [mine[0], mine[1]]},
        ], "mixed": []})
    } else {
        json!({"items": [], "mixed": []})
    };
    v.to_string()
}

fn combine_answer(flat: &[Value]) -> String {
    let mut merged: Vec<((String, String), Value)> = Vec::new();
    for it in flat {
        let key = (it["kind"].as_str().unwrap().to_string(), it["topic"].as_str().unwrap().to_string());
        match merged.iter_mut().find(|(k, _)| *k == key) {
            Some((_, m)) => {
                m["text"] = it["text"].clone();
                let mut ev: Vec<String> =
                    m["evidence"].as_array().unwrap().iter().chain(it["evidence"].as_array().unwrap()).map(|e| e.as_str().unwrap().to_string()).collect();
                ev.sort_by_key(|e| eid_number(e));
                ev.dedup();
                m["evidence"] = json!(ev);
            }
            None => merged.push((key, json!({"kind": it["kind"], "topic": it["topic"], "text": it["text"],
                                             "evidence": it["evidence"]}))),
        }
    }
    json!({"items": merged.into_iter().map(|(_, v)| v).collect::<Vec<_>>()}).to_string()
}

fn merge_answer(day: &str, user: &str) -> String {
    let items: Vec<Value> = serde_json::from_str(between(user, "<items>\n", "\n</items>")).unwrap();
    let ev = |kind: &str, topic: &str| -> Value {
        items.iter().find(|it| it["kind"] == kind && it["topic"] == topic).map_or(json!([]), |it| it["evidence"].clone())
    };
    let v = if day == DAYS[0] {
        let (person, work, me) = (ev("person", "Jamie"), ev("work", "Garden project"), ev("self", "me"));
        json!({"edits": [
            {"file": "core", "op": "rewrite", "section": "Jamie", "evidence": person,
             "text": "- Jamie is a close friend.\n- Jamie likes gardening.\n- Jamie waters the garden on weekends."},
            {"file": "core", "op": "rewrite", "section": "Robin", "text": "Robin is new.", "evidence": person},
            {"file": "core", "op": "rewrite", "section": "Me", "text": "I am different now.", "evidence": person},
            {"file": "1", "op": "title", "text": "Garden"},
            {"file": "1", "op": "rewrite", "section": "Garden project", "evidence": work,
             "text": "- Planting tomatoes.\n- Watering daily.\n- Status: not yet confirmed.\n- Next: build a fence.\n- Soil is clay.\n- Bought seeds."},
            {"file": "2", "op": "remake", "title": "Trip 2026-09-01", "text": "## Trip\n- Plan the route.", "evidence": work},
            {"file": "3", "op": "remake", "title": "Reading list", "text": "Books to read next.", "evidence": work},
            {"file": "core", "op": "move_section", "section": "Nobody", "to": "2"},
            {"file": "core", "op": "delete"},
            {"file": "1", "op": "rewrite", "section": "Other", "text": "x", "evidence": ["e99999"]},
        ], "proposals": [
            {"text": "I prefer short answers.", "same_as": null, "evidence": me},
            {"text": "From the person only.", "same_as": null, "evidence": person},
        ], "notes": ["1: what the garden needs"]})
    } else if day == DAYS[1] {
        json!({"edits": [
            {"file": "core", "op": "rewrite", "section": "Jamie", "evidence": ev("person", "Jamie"),
             "text": "- Jamie is my oldest friend.\n- Jamie prefers flowers now.\n- Jamie waters on Sundays."},
            {"file": "core", "op": "rewrite", "section": "Alex", "text": "- Alex is Jamie's sibling.",
             "evidence": ev("person", "Alex")},
            {"file": "1", "op": "rewrite", "section": "Garden project", "text": "- Tomatoes planted.\n- Fence built.",
             "evidence": ev("work", "Garden project")},
        ], "proposals": [{"text": "I prefer short answers, always.", "same_as": "p1", "evidence": ev("self", "me")}],
           "notes": []})
    } else if day == DAYS[2] {
        json!({"edits": [
            {"file": "core", "op": "move_section", "section": "Alex", "to": "3"},
            {"file": "4", "op": "rewrite", "section": "Big list", "text": big(), "evidence": ev("work", "Big list")},
            {"file": "4", "op": "title", "text": "Big list"},
            {"file": "2", "op": "remake", "title": "People notes", "text": "## Jamie\n- Dup.\n## Neighbors\n- Kind people.",
             "evidence": ev("work", "Big list")},
            {"file": "core", "op": "rewrite", "section": "Jamie", "text": "- Changed.", "evidence": ev("person", "Alex")},
        ], "proposals": [{"text": "I prefer short answers, still.", "same_as": "p1", "evidence": ev("self", "me")}],
           "notes": []})
    } else if !user.contains("Your last answer was too long") {
        return r#"{"edits": [{"file": "core", "op": "rewrite", "text": "cut off"#.into();
    } else {
        json!({"edits": [{"file": "core", "op": "rewrite", "section": "Me", "ripe": "p1",
                          "text": "I am the assistant in these conversations with this user.\nI keep answers short."}],
               "proposals": [], "notes": []})
    };
    v.to_string()
}

fn scripted(system: &str, user: &str, first: bool) -> String {
    if system.contains(" you read part ") {
        let k: usize = between(system, "you read part ", " of").parse().unwrap();
        let lines = lines_of(user);
        let day = lines[0].1[..10].to_string();
        return read_answer(&day, k, &lines, first);
    }
    if system.contains("Several readers each read one part") {
        let flat: Vec<Value> = serde_json::from_str(user).unwrap();
        return combine_answer(&flat);
    }
    if system.contains(", finishing.") {
        let day = &between(system, "Today is ", ".")[..10];
        return merge_answer(day, user);
    }
    if system.contains("Tonight you rewrote how") {
        return json!({"add": ["Jamie prefers flowers now."], "op": "replace", "line": 2,
                      "text": "Jamie likes flowers more than vegetables."})
        .to_string();
    }
    if system.contains("lines that are only a note of doubt") {
        let drop: Vec<u64> = section_in(user)
            .1
            .split('\n')
            .filter_map(|ln| ln.split_once(": "))
            .filter(|(_, t)| t.contains("not yet confirmed"))
            .map(|(n, _)| n.parse().unwrap())
            .collect();
        return json!({"drop": drop}).to_string();
    }
    if system.contains("has grown:") {
        let (heading, text) = section_in(user);
        let folded = if heading == "Big list" {
            format!("{text} and more")
        } else {
            text.split('\n').take(3).collect::<Vec<_>>().join("\n")
        };
        return json!({"text": folded}).to_string();
    }
    if system.contains("there is no room") {
        return json!({"text": "## Big list\n- Trimmed to what matters."}).to_string();
    }
    panic!("a call the script does not know: {}", &system[..80]);
}

#[derive(Default)]
struct Model {
    state: Mutex<ModelState>,
    named: Option<&'static str>,
    settle: Option<String>,
}

#[derive(Default)]
struct ModelState {
    seen: HashMap<String, usize>,
    calls: Vec<String>,
    texts: Vec<(String, String)>,
    models: Vec<String>,
    fail: Vec<ModelCallError>,
}

impl Model {
    fn failing(fail: Vec<ModelCallError>) -> Arc<Self> {
        let m = Model::default();
        m.state.lock().unwrap().fail = fail;
        Arc::new(m)
    }

    fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }

    fn texts(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().texts.clone()
    }

    fn models(&self) -> Vec<String> {
        self.state.lock().unwrap().models.clone()
    }

    fn digest(&self) -> String {
        let mut calls = self.calls();
        calls.sort();
        sha256(calls.join("\n").as_bytes())
    }
}

impl ModelCaller for Model {
    fn call<'a>(&'a self, c: &'a ModelCall) -> BoxFuture<'a, Result<ModelReply, ModelCallError>> {
        Box::pin(async move {
            assert!(c.purpose == CallPurpose::Sleep && c.system.len() == 1 && c.max_tokens == 16_000, "{c:?}");
            let (system, user) = (c.system[0].clone(), c.user.clone());
            let first = {
                let mut st = self.state.lock().unwrap();
                let key = sha256(format!("{system}\u{0}{user}").as_bytes());
                let seen = st.seen.entry(key.clone()).or_insert(0);
                *seen += 1;
                let first = *seen == 1;
                st.calls.push(key);
                st.texts.push((system.clone(), user.clone()));
                st.models.push(c.model.clone());
                if !st.fail.is_empty() {
                    return Err(st.fail.remove(0));
                }
                first
            };
            let text = match &self.settle {
                Some(s) if system.contains("Tonight you rewrote how") => s.clone(),
                _ => scripted(&system, &user, first),
            };
            let usage = json!({"input_tokens": sleep_tokens(&user), "output_tokens": sleep_tokens(&text)});
            Ok(ModelReply { text, stop: "end".into(), usage: Some(usage) })
        })
    }

    fn model(&self) -> Option<&str> {
        self.named
    }
}

struct Seen {
    path: String,
    body: Value,
    headers: HashMap<String, String>,
}

#[derive(Clone)]
struct Version {
    version: i64,
    title: String,
    body: String,
    tokens: i64,
    evidence: Value,
    day: Value,
    sleep_id: Value,
}

struct SleepSpan {
    since: usize,
    until: usize,
    handed: HashSet<String>,
}

struct Service {
    rows: Vec<Value>,
    last: usize,
    sleeps: BTreeMap<String, SleepSpan>,
    slots: [Vec<Version>; 5],
    notes: Option<Value>,
    notes_version: i64,
    caps: [i64; 5],
    scale: i64,
    room_message: bool,
    fail: Vec<(String, u16, Value)>,
    seen: Vec<Seen>,
    local_now: String,
    assistant: Value,
}

impl Default for Service {
    fn default() -> Self {
        Service {
            rows: Vec::new(),
            last: 0,
            sleeps: BTreeMap::new(),
            slots: Default::default(),
            notes: None,
            notes_version: 0,
            caps: [70_000, 20_000, 20_000, 20_000, 20_000],
            scale: 1,
            room_message: true,
            fail: Vec::new(),
            seen: Vec::new(),
            local_now: "2026-10-20T09:30:00+09:00".into(),
            assistant: json!("Kai"),
        }
    }
}

fn coded(status: u16, code: &str, message: &str) -> (u16, Value) {
    (status, json!({"error": {"code": code, "message": message}}))
}

impl Service {
    fn count(&self, text: &str) -> i64 {
        sleep_tokens(text) as i64 * self.scale
    }

    fn latest(&self, k: usize) -> Option<&Version> {
        self.slots[k].last()
    }

    fn state(&self) -> Value {
        let slots: Map<String, Value> = (0..5)
            .filter(|k| !self.slots[*k].is_empty())
            .map(|k| {
                let vs = self.slots[k]
                    .iter()
                    .map(|v| json!({"version": v.version, "title": v.title, "body": v.body, "evidence": v.evidence, "day": v.day}))
                    .collect();
                (k.to_string(), Value::Array(vs))
            })
            .collect();
        let notes = self.notes.clone().unwrap_or(json!({}));
        json!({"slots": slots, "proposals": notes.get("proposals").cloned().unwrap_or(json!([])),
               "mixed": notes.get("mixed").cloned().unwrap_or(json!([]))})
    }

    fn entry(&self, k: usize, v: Option<&Version>) -> Value {
        match v {
            None => json!({"slot": k, "title": "", "version": 0, "tokens": 0, "written_at": null, "day": null}),
            Some(v) => json!({"slot": k, "title": v.title, "version": v.version, "tokens": v.tokens,
                              "written_at": "2026-10-20T00:00:00Z", "day": v.day}),
        }
    }

    fn handle(&mut self, path: &str, body: Value, headers: HashMap<String, String>) -> (u16, Value) {
        self.seen.push(Seen { path: path.to_string(), body: body.clone(), headers });
        if let Some(i) = self.fail.iter().position(|(p, _, _)| p == path) {
            let (_, status, payload) = self.fail.remove(i);
            return (status, payload);
        }
        let route = path.trim_start_matches("/api/v1/memory/");
        if let Some(query) = route.strip_prefix("settings?") {
            let user = query.strip_prefix("user_id=").unwrap_or("default");
            return (200, json!({"user_id": user, "time_zone": null, "default_speakers": {"user": "Ana", "assistant": self.assistant},
                                "speakers": ["Ana"], "history": []}));
        }
        let takes: Option<&[&str]> = match route {
            "sleep/done" => Some(&["user_id", "sleep_id"]),
            "self" => Some(&["user_id"]),
            "self/open" => Some(&["user_id", "slot", "version"]),
            "self/history" => Some(&["user_id", "slot"]),
            "self/write" => Some(&["user_id", "sleep_id", "writes"]),
            "self/notes" => Some(&["user_id"]),
            "self/notes/write" => Some(&["user_id", "base_version", "notes"]),
            _ => None,
        };
        let b = body.as_object().cloned().unwrap_or_default();
        let mut extra: Vec<String> =
            takes.map_or(Vec::new(), |t| b.keys().filter(|k| !t.contains(&k.as_str())).cloned().collect());
        if route == "self/write" {
            let known = ["slot", "base_version", "title", "body", "evidence", "day"];
            for w in b["writes"].as_array().into_iter().flatten() {
                extra.extend(w.as_object().unwrap().keys().filter(|k| !known.contains(&k.as_str())).cloned());
            }
        }
        if route == "self/notes/write" {
            extra.extend(b["notes"].as_object().into_iter().flatten().map(|(k, _)| k.clone()).filter(|k| k != "proposals" && k != "mixed"));
        }
        if !extra.is_empty() {
            return coded(400, "invalid_request", &format!("unknown fields {extra:?}"));
        }
        let user = b["user_id"].clone();
        match route {
            "sleep" => {
                let sid = match b.get("sleep_id").and_then(Value::as_str) {
                    Some(s) => s.to_string(),
                    None => {
                        let sid = format!("s-{}", self.sleeps.len() + 1);
                        self.sleeps.insert(sid.clone(), SleepSpan { since: self.last, until: self.rows.len(), handed: HashSet::new() });
                        sid
                    }
                };
                let (since, until) = (self.sleeps[&sid].since, self.sleeps[&sid].until);
                let mut i = b.get("cursor").and_then(|c| c.get("after")).and_then(Value::as_u64).map_or(since, |x| x as usize);
                let budget = b.get("budget_tokens").and_then(Value::as_i64).unwrap_or(5000);
                let (mut page, mut used) = (Vec::new(), 0);
                while i < until {
                    let t = self.rows[i]["tokens"].as_i64().unwrap();
                    if !page.is_empty() && used + t > budget {
                        break;
                    }
                    page.push(self.rows[i].clone());
                    used += t;
                    i += 1;
                }
                let span = self.sleeps.get_mut(&sid).unwrap();
                span.handed.extend(page.iter().map(|x| x["id"].as_str().unwrap().to_string()));
                let exhausted = i >= until;
                (200, json!({"user_id": user, "sleep_id": sid, "count": page.len(), "memories": page, "tokens": used,
                             "exhausted": exhausted, "next_cursor": if exhausted { Value::Null } else { json!({"after": i}) },
                             "clock": {"local_now": self.local_now}}))
            }
            "sleep/done" => {
                let sid = b["sleep_id"].as_str().unwrap();
                let until = self.sleeps[sid].until;
                let changed = until > self.last;
                self.last = self.last.max(until);
                (200, json!({"user_id": user, "sleep_id": sid, "changed": changed, "last_sleep": {"until": self.last}}))
            }
            "self" => {
                let mut core = self.entry(0, self.latest(0));
                core.as_object_mut().unwrap().remove("slot");
                core["body"] = json!(self.latest(0).map_or("", |v| v.body.as_str()));
                let files: Vec<Value> = (1..5).map(|k| self.entry(k, self.latest(k))).collect();
                (200, json!({"user_id": user, "core": core, "files": files, "notes_version": self.notes_version}))
            }
            "self/open" => {
                let k = b["slot"].as_u64().unwrap() as usize;
                let vs = &self.slots[k];
                let v = match b.get("version").and_then(Value::as_i64) {
                    Some(n) if n < 1 || n as usize > vs.len() => return coded(404, "self_version_not_found", "no such version"),
                    Some(n) => Some(&vs[n as usize - 1]),
                    None => vs.last(),
                };
                let mut out = self.entry(k, v);
                let o = out.as_object_mut().unwrap();
                o.insert("user_id".into(), user);
                o.insert("latest_version".into(), json!(vs.len()));
                o.insert("body".into(), json!(v.map_or("", |v| v.body.as_str())));
                o.insert("evidence".into(), v.map_or(json!([]), |v| v.evidence.clone()));
                o.insert("sleep_id".into(), v.map_or(Value::Null, |v| v.sleep_id.clone()));
                (200, out)
            }
            "self/history" => {
                let k = b["slot"].as_u64().unwrap() as usize;
                let vs = &self.slots[k];
                let versions: Vec<Value> = vs
                    .iter()
                    .map(|v| {
                        let mut e = self.entry(k, Some(v));
                        e["evidence_count"] = json!(v.evidence.as_array().map_or(0, Vec::len));
                        e["sleep_id"] = v.sleep_id.clone();
                        e
                    })
                    .collect();
                (200, json!({"user_id": user, "slot": k, "latest_version": vs.len(), "count": vs.len(), "versions": versions}))
            }
            "self/write" => {
                let sid = b["sleep_id"].as_str().unwrap_or("").to_string();
                let Some(span) = self.sleeps.get(&sid) else {
                    return coded(404, "sleep_not_found", "no sleep of this store has that sleep_id");
                };
                let writes = b["writes"].as_array().cloned().unwrap_or_default();
                for w in &writes {
                    let ev = w.get("evidence").and_then(Value::as_array).cloned().unwrap_or_default();
                    if !ev.iter().all(|e| span.handed.contains(e.as_str().unwrap())) {
                        return coded(400, "self_evidence_unknown", "evidence the sleep was not handed");
                    }
                    let k = w["slot"].as_u64().unwrap() as usize;
                    if self.latest(k).map_or(0, |v| v.version) != w["base_version"].as_i64().unwrap() {
                        return coded(409, "self_version_conflict", "the file changed since it was read");
                    }
                    let n = self.count(w["body"].as_str().unwrap());
                    if n > self.caps[k] {
                        let named = if k == 0 { "slot 0 (the core)".to_string() } else { format!("slot {k} (file {k})") };
                        let message = if self.room_message {
                            format!(
                                "{named} would hold {n} tokens, over its cap of {}: there is no room. Make room (fold, or \
                                     move from the core to a file) and write again. Nothing was written.",
                                self.caps[k]
                            )
                        } else {
                            "there is no room".to_string()
                        };
                        return coded(400, "self_no_room", &message);
                    }
                }
                let mut out = Vec::new();
                for w in &writes {
                    let k = w["slot"].as_u64().unwrap() as usize;
                    let body = w["body"].as_str().unwrap().to_string();
                    let v = Version {
                        version: w["base_version"].as_i64().unwrap() + 1,
                        title: w.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
                        tokens: self.count(&body),
                        body,
                        evidence: w.get("evidence").cloned().unwrap_or(json!([])),
                        day: w.get("day").cloned().unwrap_or(Value::Null),
                        sleep_id: json!(sid),
                    };
                    let mut e = self.entry(k, Some(&v));
                    e["evidence_count"] = json!(v.evidence.as_array().map_or(0, Vec::len));
                    e["sleep_id"] = json!(sid);
                    out.push(e);
                    self.slots[k].push(v);
                }
                (200, json!({"user_id": user, "sleep_id": sid, "written": out}))
            }
            "self/notes" => {
                let notes = self.notes.clone().unwrap_or(json!({"proposals": [], "mixed": []}));
                (200, json!({"user_id": user, "version": self.notes_version, "written_at": null,
                             "bytes": notes.to_string().len(), "notes": notes}))
            }
            "self/notes/write" => {
                if b["base_version"].as_i64() != Some(self.notes_version) {
                    return coded(409, "self_notes_conflict", "the notes changed since they were read");
                }
                let n = &b["notes"];
                self.notes = Some(json!({"proposals": n.get("proposals").cloned().unwrap_or(json!([])),
                                         "mixed": n.get("mixed").cloned().unwrap_or(json!([]))}));
                self.notes_version += 1;
                (200, json!({"user_id": user, "version": self.notes_version, "written_at": "2026-10-20T00:00:00Z"}))
            }
            "work/open" => (200, json!({"work_id": "w-1", "steps_left": 5, "deeper_left": 3,
                                        "memories": [{"n": 1, "id": "id-1", "content": "Jamie likes flowers."}]})),
            other => panic!("a route the stand-in does not know: {other}"),
        }
    }
}

type Svc = Arc<Mutex<Service>>;

fn serve(svc: Svc) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
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
            let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
            let body = serde_json::from_slice(&buf[head_end..]).unwrap_or(json!({}));
            let (status, payload) = svc.lock().unwrap().handle(&path, body, headers);
            let payload = payload.to_string();
            let _ = sock.write_all(
                format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                )
                .as_bytes(),
            );
        }
    });
    base
}

fn new_with(svc: Service) -> (Svc, Client) {
    let svc = Arc::new(Mutex::new(svc));
    let base = serve(svc.clone());
    (svc, Client::with_base_url(KEY, &base))
}

fn new() -> (Svc, Client) {
    new_with(Service::default())
}

fn give(svc: &Svc, rows: Vec<Value>) {
    svc.lock().unwrap().rows.extend(rows);
}

fn paths(svc: &Svc) -> Vec<String> {
    svc.lock().unwrap().seen.iter().map(|s| s.path.trim_start_matches("/api/v1/memory/").to_string()).collect()
}

fn night(today: &str) -> SleepOpts {
    SleepOpts { today: Some(today.into()), settings: settings(), ..Default::default() }
}

async fn sleep_with(mem: &Client, model: &Arc<Model>, opts: SleepOpts) -> Result<Value, SleepError> {
    mem.sleep("u", model.clone(), &opts).await
}

fn wos(r: Result<Value, SleepError>) -> WosError {
    match r {
        Err(SleepError::Wos(e)) => e,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn the_prompts_are_the_reference_bytes() {
    for (name, text, want) in [
        ("_SLEEP_DIGEST", SLEEP_DIGEST, "8cff7e06101f3cf1f69256323a5d40563e576d9ecd963016b35a57980ce62541"),
        ("_SLEEP_COMBINE", SLEEP_COMBINE, "dc3132527b20cdef6ce3af6bf7da1bc1d885aa74914230566c6212765ab4942b"),
        ("_SLEEP_MERGE", SLEEP_MERGE, "7ccc820aa16ec2f16f36bf0b4afa57010ece0f36fc03fe6303d28a02a750ac2f"),
        ("_SLEEP_SHORTER", SLEEP_SHORTER, "1697835120c7f56cc71b2dc229987c4cf45d682eb3084480886f49bc23832730"),
        ("_SLEEP_SELF_RULE", SLEEP_SELF_RULE, "47b04387ca56c24adf5d4e31b6072b8a61bdf36d9880704428e7c509441b8ea1"),
        ("_SLEEP_SETTLE", SLEEP_SETTLE, "c20ba6effc265bd5612530f0f657e660a30eff85f28fd846b79ba5bed20163ca"),
        ("_SLEEP_FOLD_SECTION", SLEEP_FOLD_SECTION, "294ec09cd08d75e1778d567c7a3b1eb2dd4dd221f662334f51d8449260fe3881"),
        ("_SLEEP_TIDY", SLEEP_TIDY, "dff0600c9f98c1cc5928f75dd0101e5928e31f06a0d6aadc1a2f553706e3c16c"),
        ("_SLEEP_ROOM", SLEEP_ROOM, "41aff5a9ccb596e41031bd269a78dc4bc71aa5a9a47ec08f67ac8361c4926168"),
        ("_SLEEP_READER_NOTE", SLEEP_READER_NOTE, "0fa78c4e82cdb880ad70871895a61d7dcdc6505d5b1d1a0498c6c3c75669f028"),
    ] {
        assert_eq!(sha256(text.as_bytes()), want, "{name}");
    }
    assert_eq!(sha256(py_dumps(&open_box_tool()).as_bytes()), "4fa83367c475682278e48a00a6b24fdd86a35240c36965001512e4219c1d64d8");
    assert_eq!(SLEEP_NOT_JSON, "\n\nYour last answer was not valid JSON. Answer with the JSON object only.");
    assert_eq!(sha256(SLEEP_NOT_JSON.as_bytes()), "eeaaab2e5fb02a8f626858bd883425b5a5d6b8e84d6b01fa80b924f0530c0703");
}

#[test]
fn the_settings_are_the_reference_ones() {
    assert_eq!(
        (SLEEP_RIPE_SLEEPS, SLEEP_RIPE_DAYS, SLEEP_MIN_EVIDENCE, SLEEP_FOLD_LINES, SLEEP_LINE_TOKENS),
        (3.0, 28.0, 2, 4, 90)
    );
    assert_eq!((SLEEP_DOUBT_TOKENS, SLEEP_OVERTURN, SLEEP_MIN_PART, SLEEP_MAX_READERS), (25, 1, 50_000, 200));
    assert_eq!(SLEEP_KINDS, ["self", "person", "work"]);
    assert_eq!(SLEEP_MIX_KINDS, ["echo", "planted", "speaker"]);
    let s = SleepSettings::default();
    assert_eq!(
        (s.core_tokens, s.box_tokens, s.part_tokens, s.readers, s.group, s.sleepy_tokens),
        (70_000, 20_000, 100_000, 10, 10, 10_000_000)
    );
    assert_eq!((SLEEP_PAGE_TOKENS, SLEEP_LINE_FRAME, SLEEP_MAX_TOKENS), (20_000, 12, 16_000));
    assert_eq!((SLEEP_IN_PER_100, SLEEP_OUT_PER_100, SLEEP_MARGIN, SLEEP_ESTIMATE_PRICES), (172, 7, 0.15, (2.0, 10.0)));
    assert_eq!((SLEEP_ESTIMATE_MODEL, SLEEP_VERSION, crate::SLEEP_VERSION), ("claude-sonnet-5-5", "1.0.2", "1.0.2"));
    let o = SleepOpts::default();
    assert_eq!((o.being.as_str(), o.prompt.as_str(), o.done, o.max_tokens), ("the assistant", "", true, 16_000));
    assert_eq!((o.line_time, o.settings), (SleepLineTime::CreatedAt, s));
}

#[test]
fn settings_are_checked() {
    let with = |f: fn(&mut SleepSettings)| {
        let mut s = SleepSettings::default();
        f(&mut s);
        match s.check() {
            Err(WosError::Api { status: 400, message, .. }) => message,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(with(|s| s.core_tokens = 9_999), "core_tokens must be 10,000-70,000 (got 9,999)");
    assert_eq!(with(|s| s.core_tokens = 70_001), "core_tokens must be 10,000-70,000 (got 70,001)");
    assert_eq!(with(|s| s.box_tokens = 20_001), "box_tokens must be 10,000-20,000 (got 20,001)");
    assert_eq!(with(|s| s.part_tokens = 49_999), "part_tokens must be 50,000 or more (got 49,999)");
    assert_eq!(with(|s| s.readers = 0), "readers must be 1-200 (got 0)");
    assert_eq!(with(|s| s.readers = 201), "readers must be 1-200 (got 201)");
    assert_eq!(with(|s| s.group = 1), "group must be 2-200 (got 1)");
    assert_eq!(with(|s| s.sleepy_tokens = 0), "sleepy_tokens must be 1 or more (got 0)");
    let s = SleepSettings { part_tokens: 50_000, readers: 200, ..Default::default() };
    assert!(s.check().is_ok());
    assert_eq!(s, SleepSettings { part_tokens: 50_000, readers: 200, ..Default::default() });
    assert!(format!("{s:?}").contains("part_tokens: 50000"));
    assert!(!s.sleepy(9_999_999));
    assert!(s.sleepy(10_000_000));
    assert!(SleepSettings { sleepy_tokens: 1, ..Default::default() }.sleepy(1));
}

#[test]
fn the_token_estimate_counts_digits_ascii_and_wide_characters_apart() {
    assert_eq!(sleep_tokens(""), 0);
    assert_eq!(sleep_tokens("abcd"), 1);
    assert_eq!(sleep_tokens("abcde"), 2);
    assert_eq!(sleep_tokens("2024"), 2);
    assert_eq!(sleep_tokens("\u{4eca}\u{65e5}"), 3);
    assert_eq!(sleep_tokens("é"), 1);
}

fn python() -> Value {
    serde_json::from_str(include_str!("../../testdata/sleep_py.json")).unwrap()
}

fn cases<'a>(py: &'a Value, key: &str) -> &'a Vec<Value> {
    py[key].as_array().unwrap()
}

#[test]
fn dates_eids_and_the_no_room_message_are_found_as_the_python_patterns_find_them() {
    let py = python();
    for c in cases(&py, "dated") {
        assert_eq!(sleep_dated(c[0].as_str().unwrap()), c[1].as_bool().unwrap(), "{}", c[0]);
    }
    for c in cases(&py, "eids") {
        let want: Vec<String> = serde_json::from_value(c[1].clone()).unwrap();
        assert_eq!(sleep_eids(Some(&c[0])), want, "{}", c[0]);
    }
    for c in cases(&py, "no_room") {
        let got = sleep_no_room(c[0].as_str().unwrap()).map(|(a, b, d)| json!([a, b, d]));
        assert_eq!(got.unwrap_or(Value::Null), c[1], "{}", c[0]);
    }
}

#[test]
fn an_answer_is_read_as_the_python_sleep_reads_it() {
    let py = python();
    for c in cases(&py, "parse") {
        let got = sleep_parse_json(c[0].as_str().unwrap()).map(Value::Object);
        assert_eq!(got.unwrap_or(Value::Null), c[1], "{}", c[0]);
    }
}

#[test]
fn times_counts_and_numbers_are_the_python_sleeps() {
    let py = python();
    for c in cases(&py, "when") {
        let got = sleep_when(c[0].as_str().unwrap()).map(|(s, t)| json!([s, t]));
        assert_eq!(got.unwrap_or(Value::Null), c[1], "{}", c[0]);
    }
    for c in cases(&py, "tokens") {
        assert_eq!(sleep_tokens(c[0].as_str().unwrap()), c[1].as_u64().unwrap(), "{}", c[0]);
    }
    for c in cases(&py, "str") {
        assert_eq!(text_of(&c[0]), c[1].as_str().unwrap(), "{}", c[0]);
    }
    for c in cases(&py, "floats") {
        assert_eq!(float_repr(c[0].as_f64().unwrap()), c[1].as_str().unwrap());
    }
    for c in cases(&py, "rounds") {
        let got = round_to(c[0].as_f64().unwrap(), c[1].as_u64().unwrap() as usize);
        assert_eq!(got, c[2].as_f64().unwrap(), "round({}, {})", c[0], c[1]);
    }
    assert_eq!((commas(0), commas(999), commas(1_000), commas(-1_234_567)), ("0".into(), "999".into(), "1,000".into(), "-1,234,567".into()));
    let filled = fill("{a} {{b}} {n:,} {n} {{{c}}} {missing}", &[("a", Arg::Text("x")), ("n", Arg::Num(12345)), ("c", Arg::Text("y"))]);
    assert_eq!(filled, "x {b} 12,345 12345 {y} {missing}");
    assert_eq!(digit('٣'), Some(3));
    assert_eq!(digit('²'), None);
    assert_eq!(digits_value("１２"), Some(12));
}

#[tokio::test(flavor = "current_thread")]
async fn four_nights_are_the_reference_sleep_call_for_call() {
    let (svc, mem) = new();
    let model = Arc::new(Model::default());
    let mut got = Vec::new();
    for (n, day) in DAYS.iter().enumerate() {
        give(&svc, night_rows(n, day));
        let opts = SleepOpts { prompt: PROMPT.into(), ..night(&format!("{day} 23:00")) };
        got.push(sleep_with(&mem, &model, opts).await.unwrap());
    }
    assert_eq!(model.calls().len(), REFERENCE_CALLS);
    assert_eq!(model.digest(), REFERENCE_CALLS_DIGEST, "the calls the model got");
    let summaries = Value::Array(got.iter().map(night_summary).collect());
    assert_eq!(digest(&summaries), REFERENCE_NIGHTS_DIGEST, "what each night did");
    let svc = svc.lock().unwrap();
    assert_eq!(digest(&svc.state()), REFERENCE_STATE_DIGEST, "the files and the notes");

    let (n1, n2, n3, n4) = (&got[0], &got[1], &got[2], &got[3]);
    assert_eq!(n1["seeded"], true);
    assert_eq!((&n1["readers"], &n1["combine_rounds"]), (&json!(3), &json!(1)));
    assert!(svc.slots[0][0].body.starts_with("## Me\nI am the assistant in these conversations"));
    assert!(svc.slots[0][0].body.contains("### Style"));
    let calls: Vec<&str> =
        n1["log"].as_array().unwrap().iter().filter(|x| x.get("attempt").is_some()).map(|x| x["call"].as_str().unwrap()).collect();
    assert!(calls.contains(&"read 3 again 2"), "a part with no JSON is read again");
    assert!(n1["log"].as_array().unwrap().iter().any(|x| x["renumbered"] == true), "a part numbered from e1 is put back");
    assert_eq!(n1["items_dropped_by_check"], 3);
    let kinds: Vec<&str> = n1["mixed"].as_array().unwrap().iter().map(|m| m["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["echo", "speaker", "speaker"]);
    assert!(n1["mixed"].as_array().unwrap().iter().all(|m| m["sleep"] == "2026-09-01 23:00"));
    let whys: Vec<&str> = n1["refused"].as_array().unwrap().iter().map(|r| r["why"].as_str().unwrap()).collect();
    for why in [
        "nothing new about this person tonight",
        "\"Me\" changes only from a ripe proposal",
        "a box is a matter, not its state on a date: remake it under the matter's name",
        "no such section",
        "unknown file or op",
        "no evidence, or evidence not in this sleep",
        "a view of the being rests on something it said",
    ] {
        assert!(whys.contains(&why), "{why}");
    }
    let sections = |v: &Value| -> Vec<String> { v.as_array().unwrap().iter().map(|t| t["section"].as_str().unwrap().to_string()).collect() };
    assert_eq!(sections(&n1["tidied"]), ["Garden project"]);
    assert_eq!(sections(&n1["folds"]), ["Garden project"]);
    assert_eq!(svc.latest(1).unwrap().title, "Garden");
    assert_eq!(n2["settled"], json!([{"section": "Jamie", "how": "replace, +1"}, {"section": "Alex", "how": "new"}]));
    assert_eq!(n3["rooms"], json!([{"file": 4, "before": n3["rooms"][0]["before"], "after": 9}]));
    assert_eq!(n3["deduped"], json!([{"file": 2, "section": "Jamie"}]));
    assert!(svc.latest(3).unwrap().body.contains("## Alex"));
    let log4 = n4["log"].as_array().unwrap();
    let failed: Vec<&str> = log4.iter().filter(|x| x["failed"] == true).map(|x| x["call"].as_str().unwrap()).collect();
    assert_eq!(failed, ["merge"]);
    assert!(log4.iter().any(|x| x["call"] == "merge shorter"));
    assert_eq!(n4["ripe"], json!(["p1"]));
    assert!(svc.latest(0).unwrap().body.contains("I keep answers short."));
    assert_eq!(svc.notes.as_ref().unwrap()["proposals"][0]["applied"], "2026-10-05 23:00");
    assert_eq!(svc.last, svc.rows.len());
    assert_eq!(svc.sleeps.len(), 4);
}

#[tokio::test(flavor = "current_thread")]
async fn a_settle_answer_with_a_number_past_what_every_client_reads_is_not_json() {
    let opts = |n: usize| SleepOpts { prompt: PROMPT.into(), ..night(&format!("{} 23:00", DAYS[n])) };
    for (line, how) in [
        ("Infinity", "unshaped"),
        ("-Infinity", "unshaped"),
        ("NaN", "unshaped"),
        ("1e999", "unshaped"),
        ("-1e999", "unshaped"),
        ("1e300", "none, +0"),
        ("2.0", "drop, +0"),
    ] {
        let (svc, mem) = new();
        give(&svc, night_rows(0, DAYS[0]));
        sleep_with(&mem, &Arc::new(Model::default()), opts(0)).await.unwrap();
        give(&svc, night_rows(1, DAYS[1]));
        let model = Arc::new(Model { settle: Some(format!(r#"{{"add": [], "op": "drop", "line": {line}}}"#)), ..Default::default() });
        let res = sleep_with(&mem, &model, opts(1)).await.unwrap();
        assert_eq!(res["settled"][0], json!({"section": "Jamie", "how": how}), "{line}");
        let tries: Vec<Value> =
            res["log"].as_array().unwrap().iter().filter(|x| x["call"] == "settle Jamie").map(|x| x["attempt"].clone()).collect();
        assert_eq!(tries, if how == "unshaped" { vec![json!(1), json!(2)] } else { vec![json!(1)] }, "{line}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_span_run_again_after_its_sleep_done_failed_counts_each_proposal_and_each_mark_once() {
    let (svc, mem) = new();
    let model = Arc::new(Model::default());
    let expire = || {
        let (status, payload) = coded(410, "sleep_expired", "This sleep was read too long ago to finish: read again.");
        svc.lock().unwrap().fail.push(("/api/v1/memory/sleep/done".into(), status, payload));
    };
    let notes = |key: &str| svc.lock().unwrap().notes.as_ref().unwrap()[key].as_array().unwrap().clone();
    let counts = || -> Vec<Value> { notes("proposals").iter().map(|p| json!([p["id"], p["sleeps"], p["span_end"]])).collect() };
    let opts = |n: usize| SleepOpts { prompt: PROMPT.into(), ..night(&format!("{} 23:00", DAYS[n])) };
    let (rows1, rows2) = (night_rows(0, DAYS[0]), night_rows(1, DAYS[1]));
    let (end1, end2) = (rows1.last().unwrap()["id"].clone(), rows2.last().unwrap()["id"].clone());
    give(&svc, rows1);
    expire();
    assert_eq!(wos(sleep_with(&mem, &model, opts(0)).await).status(), Some(410));
    let marks = notes("mixed").len();
    assert!(marks > 0);
    assert_eq!(counts(), [json!(["p1", 1, end1])]);
    sleep_with(&mem, &model, opts(0)).await.unwrap();
    assert_eq!(counts(), [json!(["p1", 1, end1])], "a proposal made by this span is not made again");
    assert_eq!(notes("mixed").len(), marks, "nor a mark");
    give(&svc, rows2);
    expire();
    assert_eq!(wos(sleep_with(&mem, &model, opts(1)).await).status(), Some(410));
    assert_eq!(counts(), [json!(["p1", 2, end2])]);
    let again = sleep_with(&mem, &model, opts(1)).await.unwrap();
    assert_eq!(counts(), [json!(["p1", 2, end2])], "the span counted it once");
    assert_eq!(again["done"]["changed"], true);
    let svc = svc.lock().unwrap();
    assert_eq!(svc.last, svc.rows.len());
}

#[tokio::test(flavor = "current_thread")]
async fn notes_from_before_span_end_still_read_and_their_proposals_count() {
    let (svc, mem) = new();
    svc.lock().unwrap().notes = Some(json!({"proposals": [{"id": "p1", "text": "I prefer short answers.", "sleeps": 1, "first": 0,
        "first_day": "2026-09-01 23:00", "last_day": "2026-09-01 23:00"}], "mixed": []}));
    let rows = night_rows(1, DAYS[1]);
    let end = rows.last().unwrap()["id"].clone();
    give(&svc, rows);
    sleep_with(&mem, &Arc::new(Model::default()), night(&format!("{} 23:00", DAYS[1]))).await.unwrap();
    let svc = svc.lock().unwrap();
    let p = &svc.notes.as_ref().unwrap()["proposals"][0];
    assert_eq!(json!([p["id"], p["sleeps"], p["span_end"]]), json!(["p1", 2, end]));
}

#[tokio::test(flavor = "current_thread")]
async fn one_night_asks_the_service_in_this_order() {
    let (svc, mem) = new();
    give(&svc, night_rows(1, DAYS[1]));
    let res = sleep_with(&mem, &Arc::new(Model::default()), night("2026-09-15 23:00")).await.unwrap();
    assert_eq!(paths(&svc), ["settings?user_id=u", "sleep", "self", "self/notes", "self/write", "self/notes/write", "sleep/done"]);
    let svc = svc.lock().unwrap();
    assert_eq!(svc.seen[1].body, json!({"user_id": "u", "budget_tokens": 20_000, "since_last_sleep": true}));
    let write = &svc.seen[4].body;
    assert_eq!(write["sleep_id"], "s-1");
    let writes = write["writes"].as_array().unwrap();
    assert_eq!(writes.iter().map(|w| w["slot"].as_u64().unwrap()).collect::<Vec<_>>(), [0, 1]);
    assert!(writes.iter().all(|w| w["base_version"] == 0 && w["day"] == "2026-09-15"));
    let ids: HashSet<&str> = svc.rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert!(writes[0]["evidence"].as_array().unwrap().iter().all(|e| ids.contains(e.as_str().unwrap())));
    for x in &svc.seen[4..] {
        assert!(x.headers.contains_key("idempotency-key"), "{}", x.path);
    }
    assert_eq!(res["written"], json!({"0": 1, "1": 1}));
    assert_eq!(res["done"]["changed"], true);
    assert_eq!((&res["sleep_id"], &res["memories"], &res["today"]), (&json!("s-1"), &json!(20), &json!("2026-09-15 23:00")));
    let log_in: i64 = res["log"].as_array().unwrap().iter().filter_map(|x| x.get("in")).map(|x| x.as_i64().unwrap()).sum();
    assert_eq!(res["llm_in"], log_in);
    assert_eq!(res["sleep_version"], SLEEP_VERSION);
    assert!(res["llm_out"].as_i64().unwrap() > 0);
}

#[tokio::test(flavor = "current_thread")]
async fn pages_are_read_to_the_end_and_today_comes_from_the_reading() {
    let (svc, mem) = new();
    give(&svc, night_rows(0, DAYS[0])[..120].to_vec());
    let opts = SleepOpts { done: false, settings: settings(), ..Default::default() };
    let res = sleep_with(&mem, &Arc::new(Model::default()), opts).await.unwrap();
    let svc = svc.lock().unwrap();
    let pages: Vec<&Value> = svc.seen.iter().filter(|x| x.path.ends_with("/sleep")).map(|x| &x.body).collect();
    assert!(pages.len() > 2);
    assert!(pages[0].get("sleep_id").is_none());
    for p in &pages[1..] {
        assert_eq!((&p["sleep_id"], &p["since_last_sleep"]), (&json!("s-1"), &json!(true)));
    }
    assert_eq!(res["memories"], 120);
    assert_eq!(res["today"], "2026-10-20 09:30", "the person's now from the reading's clock");
    assert!(res["done"].is_null());
    assert!(!svc.seen.iter().any(|x| x.path.ends_with("sleep/done")));
    assert_eq!(svc.last, 0, "not done: the next sleep reads this again");
}

#[tokio::test(flavor = "current_thread")]
async fn an_empty_span_seeds_the_self_and_ends_the_sleep_without_the_model() {
    let (svc, mem) = new();
    let model = Arc::new(Model::default());
    let opts = |today: &str| SleepOpts { prompt: "I am Mira.".into(), ..night(today) };
    let res = sleep_with(&mem, &model, opts("2026-10-20 08:00")).await.unwrap();
    assert!(model.calls().is_empty());
    assert_eq!((&res["seeded"], &res["memories"]), (&json!(true), &json!(0)));
    {
        let s = svc.lock().unwrap();
        assert_eq!(s.latest(0).unwrap().body, "## Me\nI am Mira.\n");
        assert_eq!(s.latest(0).unwrap().evidence, json!([]));
    }
    assert_eq!(paths(&svc), ["settings?user_id=u", "sleep", "self", "self/notes", "self/write", "sleep/done"]);
    let res = sleep_with(&mem, &model, opts("2026-10-21 08:00")).await.unwrap();
    assert_eq!(res["seeded"], false, "the self is there: not seeded again");
}

#[tokio::test(flavor = "current_thread")]
async fn a_prompt_over_the_core_is_refused_before_the_model_is_called() {
    let (svc, mem) = new();
    let model = Arc::new(Model::default());
    let s = SleepSettings { core_tokens: 10_000, part_tokens: 50_000, ..Default::default() };
    let opts = SleepOpts { prompt: "word ".repeat(9_000), settings: s, ..Default::default() };
    let e = wos(sleep_with(&mem, &model, opts).await).to_string();
    assert!(e.contains("the core with the prompt in it would be 10,229 tokens, over its 10,000: raise core_tokens or shorten the prompt"), "{e}");
    let at_most = SleepOpts { prompt: "word ".repeat(65_000), settings: SleepSettings { part_tokens: 50_000, ..Default::default() }, ..Default::default() };
    let e = wos(sleep_with(&mem, &model, at_most).await).to_string();
    assert!(e.contains("the core with the prompt in it would be 7"), "{e}");
    assert!(e.contains(" tokens, over its 70,000: shorten the prompt"), "{e}");
    assert!(!e.contains("raise"), "{e}");
    assert!(model.calls().is_empty());
    assert!(!paths(&svc).contains(&"self/write".to_string()));
}

#[tokio::test(flavor = "current_thread")]
async fn a_file_changed_meanwhile_is_a_conflict_and_none_of_the_night_is_kept() {
    let (svc, mem) = new();
    give(&svc, night_rows(1, DAYS[1]));
    svc.lock().unwrap().fail = vec![(
        "/api/v1/memory/self/write".into(),
        409,
        json!({"error": {"code": "self_version_conflict", "message": "the file changed since it was read"}}),
    )];
    let e = wos(sleep_with(&mem, &Arc::new(Model::default()), night("2026-09-15 23:00")).await);
    assert_eq!((e.status(), e.kind()), (Some(409), ErrorKind::Conflict));
    assert!(e.to_string().contains("the file changed since it was read"));
    let p = paths(&svc);
    assert_eq!(p.iter().filter(|x| *x == "self/write").count(), 1, "not tried again");
    assert!(!p.contains(&"self/notes/write".to_string()) && !p.contains(&"sleep/done".to_string()));
    let svc = svc.lock().unwrap();
    assert_eq!(svc.state()["slots"], json!({}));
    assert_eq!(svc.last, 0, "the next sleep reads this span again");
}

#[tokio::test(flavor = "current_thread")]
async fn a_file_the_service_counts_over_its_cap_is_made_room_for_once_then_kept_as_it_was() {
    let (svc, mem) = new();
    give(&svc, night_rows(1, DAYS[1]));
    {
        let mut s = svc.lock().unwrap();
        (s.scale, s.caps[1], s.room_message) = (2, 8, false);
    }
    let model = Arc::new(Model::default());
    let res = sleep_with(&mem, &model, night("2026-09-15 23:00")).await.unwrap();
    let svc = svc.lock().unwrap();
    let writes: Vec<&Value> = svc.seen.iter().filter(|x| x.path.ends_with("/self/write")).map(|x| &x.body["writes"]).collect();
    assert_eq!(writes.len(), 3);
    let slots: Vec<Vec<u64>> =
        writes.iter().map(|ws| ws.as_array().unwrap().iter().map(|w| w["slot"].as_u64().unwrap()).collect()).collect();
    assert_eq!(slots, [vec![0, 1], vec![0, 1], vec![0]]);
    assert_eq!(writes[1][1]["body"], "## Big list\n- Trimmed to what matters.\n");
    let room: Vec<String> = model.texts().into_iter().map(|(s, _)| s).filter(|s| s.contains("there is no room")).collect();
    assert_eq!(room.len(), 1);
    assert!(room[0].contains("left its box \"\" at 13 tokens, over its cap\nof 11: there is no room"), "{}", room[0]);
    assert_eq!(res["rooms"], json!([{"file": 1, "before": 13, "after": 9}]));
    let refused = res["refused"].as_array().unwrap();
    assert_eq!(
        refused[refused.len() - 1],
        json!({"edit": {"file": 1, "op": "room"}, "why": "no room: the service counts it over its cap; the file stays as it was"})
    );
    assert_eq!(res["written"], json!({"0": 1}));
    assert!(svc.slots[1].is_empty());
    assert!(svc.seen.iter().any(|x| x.path.ends_with("sleep/done")));
}

#[tokio::test(flavor = "current_thread")]
async fn when_the_service_names_the_file_and_its_count_the_room_is_made_against_them() {
    let (svc, mem) = new();
    give(&svc, night_rows(1, DAYS[1]));
    {
        let mut s = svc.lock().unwrap();
        (s.scale, s.caps[1]) = (2, 20);
    }
    let model = Arc::new(Model::default());
    let res = sleep_with(&mem, &model, night("2026-09-15 23:00")).await.unwrap();
    let room: Vec<String> = model.texts().into_iter().map(|(s, _)| s).filter(|s| s.contains("there is no room")).collect();
    assert!(room[0].contains("at 13 tokens, over its cap\nof 10: there is no room"), "{}", room[0]);
    assert_eq!(res["rooms"], json!([{"file": 1, "before": 13, "after": 9}]));
    assert_eq!(res["written"], json!({"0": 1, "1": 1}));
    assert_eq!(svc.lock().unwrap().latest(1).unwrap().body, "## Big list\n- Trimmed to what matters.\n");
}

#[tokio::test(flavor = "current_thread")]
async fn a_no_room_answer_that_counts_0_tokens_makes_room_against_the_clients_own_cap() {
    let (svc, mem) = new();
    give(&svc, night_rows(1, DAYS[1]));
    let error = json!({"error": {"code": "self_no_room", "message": "there is no room", "slot": 1, "tokens": 0, "cap": -1}});
    svc.lock().unwrap().fail = vec![("/api/v1/memory/self/write".into(), 400, error)];
    let model = Arc::new(Model::default());
    let res = sleep_with(&mem, &model, night("2026-09-15 23:00")).await.unwrap();
    let room: Vec<String> = model.texts().into_iter().map(|(s, _)| s).filter(|s| s.contains("there is no room")).collect();
    assert!(room[0].contains("at 13 tokens, over its cap\nof 11: there is no room"), "{}", room[0]);
    assert_eq!(res["rooms"], json!([{"file": 1, "before": 13, "after": 9}]));
    assert_eq!(res["written"], json!({"0": 1, "1": 1}));
}

#[tokio::test(flavor = "current_thread")]
async fn token_counts_and_versions_too_big_to_add_stop_at_the_largest_number_instead_of_panicking() {
    let huge = |mut r: Value| {
        r["tokens"] = json!(4_000_000_000_000_000_000u64);
        r
    };
    let (svc, mem) = new();
    give(&svc, night_rows(1, DAYS[1]).into_iter().map(huge).collect());
    let part = SleepSettings { part_tokens: 50_000, ..Default::default() };
    let est = mem.sleep_estimate("u", &SleepEstimateOpts { settings: part, ..Default::default() }).await.unwrap();
    assert_eq!((&est["tokens"], &est["input_tokens"], &est["sleepy"]), (&json!(u64::MAX), &json!(u64::MAX), &json!(true)));
    let max: CountTokens = Arc::new(|_: &str| u64::MAX);
    let counted = SleepEstimateOpts { settings: part, count_tokens: Some(max), ..Default::default() };
    assert_eq!(mem.sleep_estimate("u", &counted).await.unwrap()["tokens"], json!(u64::MAX));
    let written = sleep_written(&json!({}), &[json!({"slot": 0, "base_version": i64::MAX})]);
    assert_eq!(written, BTreeMap::from([(0, i64::MAX)]));
    let usage: Map<String, Value> = serde_json::from_value(json!({"input_tokens": i64::MAX, "output_tokens": i64::MAX})).unwrap();
    assert_eq!(sleep_usage(&usage, &["input_tokens", "output_tokens"]), i64::MAX);
}

#[tokio::test(flavor = "current_thread")]
async fn another_refusal_ends_the_sleep_and_is_not_made_room_for() {
    let (svc, mem) = new();
    give(&svc, night_rows(1, DAYS[1]));
    svc.lock().unwrap().fail =
        vec![("/api/v1/memory/self/write".into(), 400, json!({"error": {"code": "self_evidence_unknown", "message": "no"}}))];
    let e = wos(sleep_with(&mem, &Arc::new(Model::default()), night("2026-09-15 23:00")).await);
    assert_eq!(e.kind(), ErrorKind::BadRequest);
    assert!(!paths(&svc).contains(&"sleep/done".to_string()));
}

#[tokio::test(flavor = "current_thread")]
async fn a_rate_limited_call_is_tried_again_and_another_failure_ends_the_sleep() {
    let now = std::time::Duration::ZERO;
    let (svc, mem) = new();
    give(&svc, night_rows(1, DAYS[1]));
    let model = Model::failing(vec![
        ModelCallError::new(429, "slow down").with_retry_after(now),
        ModelCallError::new(529, "busy").with_retry_after(now),
    ]);
    let res = sleep_with(&mem, &model, night("2026-09-15 23:00")).await.unwrap();
    assert_eq!(res["written"], json!({"0": 1, "1": 1}));
    let calls = model.calls();
    assert!(calls[0] == calls[1] && calls[1] == calls[2]);

    let (svc, mem) = new();
    give(&svc, night_rows(1, DAYS[1]));
    let model = Model::failing(vec![ModelCallError::new(400, "bad request")]);
    match sleep_with(&mem, &model, night("2026-09-15 23:00")).await {
        Err(SleepError::Model(e)) => assert_eq!(e.status, 400),
        other => panic!("{other:?}"),
    }
    assert_eq!(model.calls().len(), 1);
    assert_eq!(paths(&svc), ["settings?user_id=u", "sleep", "self", "self/notes"]);

    let (svc, mem) = new();
    give(&svc, night_rows(1, DAYS[1]));
    let model = Model::failing(vec![ModelCallError::new(429, "slow down").with_retry_after(now); 3]);
    assert!(matches!(sleep_with(&mem, &model, night("2026-09-15 23:00")).await, Err(SleepError::Model(_))));
    assert_eq!(model.calls().len(), 3, "tried twice more, then the sleep ends");
}

#[tokio::test(flavor = "current_thread")]
async fn a_reader_that_fails_ends_the_night_before_another_part_is_sent() {
    let (svc, mem) = new();
    give(&svc, night_rows(0, DAYS[0]));
    let model = Model::failing(vec![ModelCallError::new(400, "bad request")]);
    let opts = SleepOpts { settings: SleepSettings { readers: 1, ..settings() }, ..night("2026-09-01 23:00") };
    match sleep_with(&mem, &model, opts).await {
        Err(SleepError::Model(e)) => assert_eq!(e.status, 400),
        other => panic!("{other:?}"),
    }
    assert_eq!(model.calls().len(), 1, "the parts behind the failed one are not sent");
    assert!(!paths(&svc).contains(&"self/write".to_string()));
}

#[tokio::test(flavor = "current_thread")]
async fn merge_llm_writes() {
    let (svc, mem) = new();
    give(&svc, night_rows(1, DAYS[1]));
    let (reader, writer) = (Arc::new(Model::default()), Arc::new(Model::default()));
    let w = writer.clone();
    let merge = caller_fn(move |c: ModelCall| {
        let w = w.clone();
        async move { w.call(&c).await }
    });
    let opts = SleepOpts { merge_llm: Some(merge), ..night("2026-09-15 23:00") };
    let res = sleep_with(&mem, &reader, opts).await.unwrap();
    assert_eq!(reader.calls().len(), 1);
    assert!(writer.texts().iter().all(|(s, _)| {
        s.contains(", finishing.") || s.contains("Tonight") || s.contains("note of doubt") || s.contains("has grown")
    }));
    assert_eq!(res["written"], json!({"0": 1, "1": 1}));
}

#[tokio::test(flavor = "current_thread")]
async fn the_model_is_the_callers_own_else_sonnet() {
    let (svc, mem) = new();
    give(&svc, night_rows(1, DAYS[1]));
    let named = Arc::new(Model { named: Some("some-model"), ..Default::default() });
    sleep_with(&mem, &named, night("2026-09-15 23:00")).await.unwrap();
    assert!(named.models().iter().all(|m| m == "some-model") && !named.models().is_empty());
    let (svc, mem) = new();
    give(&svc, night_rows(1, DAYS[1]));
    let plain = Arc::new(Model::default());
    sleep_with(&mem, &plain, night("2026-09-15 23:00")).await.unwrap();
    assert!(plain.models().iter().all(|m| m == "claude-sonnet-5-5") && !plain.models().is_empty());
    assert_eq!(SLEEP_MODEL, "claude-sonnet-5-5");
    assert_eq!(crate::ASK_MODEL, "claude-haiku-5-5", "the router and skills keep theirs");
}

#[tokio::test(flavor = "current_thread")]
async fn lines_are_dated_by_when_they_happened_when_asked() {
    let (svc, mem) = new();
    let mut rows = night_rows(1, DAYS[1]);
    for r in &mut rows {
        r["event_date"] = json!("2023-05-21T00:00:00Z");
    }
    give(&svc, rows);
    let model = Arc::new(Model::default());
    let opts = SleepOpts { line_time: SleepLineTime::EventDate, done: false, ..night("2026-09-15 23:00") };
    sleep_with(&mem, &model, opts).await.unwrap();
    let reads = |m: &Model| -> Vec<String> { m.texts().into_iter().filter(|(s, _)| s.contains(" you read part ")).map(|(_, u)| u).collect() };
    assert!(reads(&model)[0].contains("e1 · 2023-05-21 00:00 · user · Night 2 line 0"));
    sleep_with(&mem, &model, night("2026-09-15 23:00")).await.unwrap();
    assert!(reads(&model).last().unwrap().contains("e2 · 2026-09-15 09:00 · the assistant · Night 2 line 1"));
}

#[tokio::test(flavor = "current_thread")]
async fn whose_line_it_is_goes_by_its_speaker_and_the_assistants_name() {
    let rows = || {
        let mut rows = night_rows(1, "2026-11-02")[..4].to_vec();
        rows[2].as_object_mut().unwrap().remove("speaker");
        rows[3]["speaker"] = json!(" Kai ");
        rows[3]["origin"] = json!("x");
        rows
    };
    let reads = |m: &Model| -> String {
        m.texts().into_iter().filter(|(s, _)| s.contains(" you read part ")).map(|(_, u)| u).collect::<Vec<_>>().join("\n")
    };
    let lines = |m: &Model| -> Vec<bool> {
        let text = reads(m);
        (0..4).map(|i| text.contains(&format!("· user · Night 2 line {i}:"))).collect()
    };
    let (svc, mem) = new();
    give(&svc, rows());
    let model = Arc::new(Model::default());
    sleep_with(&mem, &model, SleepOpts { done: false, ..night("2026-11-02 23:00") }).await.unwrap();
    assert_eq!(lines(&model), [true, false, true, false], "Ana's line and the one with no speaker are a person's");
    assert_eq!(paths(&svc)[..2], ["settings?user_id=u", "sleep"]);
    assert_eq!(paths(&svc).iter().filter(|p| p.starts_with("settings")).count(), 1);
    let (svc, mem) = new();
    give(&svc, rows());
    let model = Arc::new(Model::default());
    let opts = SleepOpts { assistant: Some(" Ana ".into()), done: false, ..night("2026-11-02 23:00") };
    sleep_with(&mem, &model, opts).await.unwrap();
    assert_eq!(lines(&model), [false, true, true, true]);
    assert_eq!(paths(&svc)[0], "sleep");
    for none in [Value::Null, json!("  ")] {
        let (svc, mem) = new_with(Service { assistant: none, ..Default::default() });
        give(&svc, rows());
        let model = Arc::new(Model::default());
        let e = wos(sleep_with(&mem, &model, night("2026-11-02 23:00")).await);
        assert_eq!(e.status(), Some(400));
        for want in ["default_speakers.assistant", "SleepOpts::assistant"] {
            assert!(e.to_string().contains(want), "{e}");
        }
        assert_eq!(paths(&svc), ["settings?user_id=u"]);
        assert!(model.calls().is_empty());
    }
    let (svc, mem) = new();
    let e = wos(sleep_with(&mem, &Arc::new(Model::default()), SleepOpts { assistant: Some(" ".into()), ..Default::default() }).await);
    assert!(e.to_string().contains("assistant is required"), "{e}");
    assert!(paths(&svc).is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn the_sleeps_names_are_trimmed_as_the_python_and_typescript_sleeps_trim_them() {
    let reads = |m: &Model| -> String {
        m.texts().into_iter().filter(|(s, _)| s.contains(" you read part ")).map(|(_, u)| u).collect::<Vec<_>>().join("\n")
    };
    let kai = "e2 · 2026-11-02 09:00 · the assistant · Night 2 line 1";
    let (svc, mem) = new_with(Service { assistant: json!("Kai\u{feff}"), ..Default::default() });
    give(&svc, night_rows(1, "2026-11-02")[..2].to_vec());
    let model = Arc::new(Model::default());
    sleep_with(&mem, &model, SleepOpts { done: false, ..night("2026-11-02 23:00") }).await.unwrap();
    assert!(reads(&model).contains(kai), "the store's name is trimmed of JavaScript whitespace: {}", reads(&model));
    let (svc, mem) = new();
    give(&svc, night_rows(1, "2026-11-02")[..2].to_vec());
    let model = Arc::new(Model::default());
    let opts = SleepOpts { assistant: Some("Kai\u{feff}".into()), done: false, ..night("2026-11-02 23:00") };
    sleep_with(&mem, &model, opts).await.unwrap();
    assert!(!reads(&model).contains(kai), "a given name is trimmed of Python whitespace only: {}", reads(&model));
    let (svc, mem) = new();
    for (name, opts) in [
        ("being", SleepOpts { being: "\u{1c}".into(), ..Default::default() }),
        ("since", SleepOpts { since: Some(" \u{1d}".into()), ..Default::default() }),
        ("sleep_id", SleepOpts { sleep_id: Some("\u{1e}".into()), ..Default::default() }),
        ("assistant", SleepOpts { assistant: Some("\u{1f}".into()), ..Default::default() }),
    ] {
        let e = wos(sleep_with(&mem, &Arc::new(Model::default()), opts).await);
        assert!(e.to_string().contains(&format!("{name} is required")), "{name}: {e}");
    }
    let opts = SleepEstimateOpts { since: Some("\u{1c}".into()), ..Default::default() };
    assert!(mem.sleep_estimate("u", &opts).await.unwrap_err().to_string().contains("since is required"));
    assert!(paths(&svc).is_empty());
}

#[test]
fn a_line_number_is_read_as_python_reads_int() {
    for (v, want) in [
        (json!(" 12 "), Some(12)),
        (json!("+5"), Some(5)),
        (json!("-3"), Some(-3)),
        (json!("1_000"), Some(1000)),
        (json!("-1_0"), Some(-10)),
        (json!("\u{661}\u{662}"), Some(12)),
        (json!("\u{ff11}\u{ff12}"), Some(12)),
        (json!(true), Some(1)),
        (json!(false), Some(0)),
        (json!(2.9), Some(2)),
        (json!(-2.9), Some(-2)),
        (json!("2.0"), None),
        (json!(""), None),
        (json!("1__0"), None),
        (json!("_1"), None),
        (json!("1_"), None),
        (json!("\u{b2}"), None),
        (Value::Null, None),
    ] {
        assert_eq!(int_like(Some(&v)), want, "int({v})");
    }
}

#[test]
fn the_many_people_texts_stand_for_the_one_person_passages_once() {
    assert_eq!(
        SLEEP_MANY_PEOPLE.digest,
        "Each line from a person is marked with that person's name, and different names are different\n  people: \
             each name is its own topic, and what one of them said, wants or is never goes under another.\n  A line \
             marked \"user\" names no one: give it to no named person."
    );
    assert_eq!(
        SLEEP_MANY_PEOPLE.merge,
        "and under one name: the memories mark each person by name, so each person has a section under their\n   own \
             name, and what one of them said, wants or is never goes into another's."
    );
    for (text, one, many) in [
        (SLEEP_DIGEST, SLEEP_ONE_PERSON.digest, SLEEP_MANY_PEOPLE.digest),
        (SLEEP_MERGE, SLEEP_ONE_PERSON.merge, SLEEP_MANY_PEOPLE.merge),
    ] {
        assert_eq!(text.matches(one).count(), 1);
        assert_eq!(sleep_people(text, one, many, false), text);
        let got = sleep_people(text, one, many, true);
        assert_eq!(got, text.replace(one, many));
        assert!(!got.contains(one) && got.matches(many).count() == 1);
    }
    let missed = std::panic::catch_unwind(|| sleep_people(SLEEP_COMBINE, SLEEP_ONE_PERSON.digest, "x", true));
    assert!(missed.is_err());
}

#[test]
fn people_are_counted_across_the_whole_night() {
    let row = |speaker: Option<&str>| {
        let mut r = json!({"content": "words", "created_at": "2026-11-02T09:00:00Z"});
        if let Some(s) = speaker {
            r["speaker"] = json!(s);
        }
        r
    };
    let who = |rows: &[Value]| -> (Vec<String>, Vec<bool>, bool) {
        let (lines, many) = sleep_memories(rows, "Kai", "Kai", SleepLineTime::CreatedAt, &Count(None));
        (lines.iter().map(|m| m.who.clone()).collect(), lines.iter().map(|m| m.mine).collect(), many)
    };
    let one = [row(Some("Dana")), row(Some("Kai")), row(None), row(Some(" Dana "))];
    assert_eq!(who(&one), (vec!["user".into(), "Kai".into(), "user".into(), "user".into()], vec![false, true, false, false], false));
    let two = [row(Some("Dana")), row(Some("Kai")), row(None), row(Some(" Dana ")), row(Some("Mina"))];
    let (labels, mine, many) = who(&two);
    assert!(many);
    assert_eq!(labels, ["Dana", "Kai", "user", "Dana", "Mina"]);
    assert_eq!(mine, [false, true, false, false, false]);
}

fn two_people_rows() -> Vec<Value> {
    [
        (Some("Dana"), "I am vegetarian."),
        (Some("Kai"), "Noted, Dana: no meat for you."),
        (Some("Mina"), "I work the night shift."),
        (Some("Kai"), "Noted, Mina: your mornings stay free."),
        (Some("Dana"), "No meat at the dinner, please."),
        (Some("Mina"), "Keep my mornings free."),
        (None, "I like cooking for both of them."),
        (Some("Kai"), "I like planning the dinner."),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (speaker, text))| {
        let mut r = json!({"id": format!("d{i}"), "content": text, "created_at": format!("2026-11-02T09:{:02}:00Z", i),
                           "tokens": sleep_tokens(text)});
        if let Some(s) = speaker {
            r["speaker"] = json!(s);
        }
        r
    })
    .collect()
}

fn two_people_model(seen: Arc<Mutex<Vec<(String, String)>>>) -> Arc<dyn ModelCaller> {
    caller_fn(move |c: ModelCall| {
        let (system, user) = (c.system[0].clone(), c.user.clone());
        seen.lock().unwrap().push((system.clone(), user.clone()));
        async move {
            let v = if system.contains(" you read part ") {
                json!({"items": [
                    {"kind": "person", "topic": "Dana", "text": "Dana is vegetarian.", "evidence": ["e1", "e5"]},
                    {"kind": "person", "topic": "Mina", "text": "Mina works the night shift.", "evidence": ["e3", "e6"]},
                    {"kind": "self", "topic": "me", "text": "I like cooking.", "evidence": ["e7", "e8"]},
                    {"kind": "self", "topic": "me", "text": "Not mine to say.", "evidence": ["e1", "e3"]},
                ], "mixed": [
                    {"line": "e6", "kind": "echo", "source": "e4", "why": "gave back my words as hers"},
                    {"line": "e7", "kind": "planted", "why": "never said by them"},
                    {"line": "e3", "kind": "planted", "why": "a person's line is not planted"},
                    {"line": "e7", "kind": "echo", "why": "the being's line is not an echo"},
                ]})
            } else if system.contains(", finishing.") {
                json!({"edits": [
                    {"file": "core", "op": "rewrite", "section": "Dana", "text": "- Dana is vegetarian.", "evidence": ["e1", "e5"]},
                    {"file": "core", "op": "rewrite", "section": "Mina", "text": "- Mina works the night shift.",
                     "evidence": ["e3", "e6"]},
                ], "proposals": [
                    {"text": "I like cooking.", "same_as": null, "evidence": ["e7"]},
                    {"text": "From the people only.", "same_as": null, "evidence": ["e1", "e3"]},
                ], "notes": []})
            } else if system.contains("lines that are only a note of doubt") {
                json!({"drop": []})
            } else {
                panic!("a call the script does not know: {}", &system[..80]);
            };
            let text = v.to_string();
            Ok(ModelReply { text, stop: "end".into(), usage: Some(json!({"input_tokens": 1, "output_tokens": 1})) })
        }
    })
}

#[tokio::test(flavor = "current_thread")]
async fn two_people_are_named_apart_and_each_rests_on_their_own_lines() {
    let (svc, mem) = new();
    give(&svc, two_people_rows());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let opts = SleepOpts { being: "Kai".into(), done: false, ..night("2026-11-02 23:00") };
    let res = mem.sleep("u", two_people_model(seen.clone()), &opts).await.unwrap();
    let seen = seen.lock().unwrap().clone();
    let read: Vec<&(String, String)> = seen.iter().filter(|(s, _)| s.contains(" you read part ")).collect();
    let merge: Vec<&(String, String)> = seen.iter().filter(|(s, _)| s.contains(", finishing.")).collect();
    assert_eq!((read.len(), merge.len()), (1, 1));
    let labels: Vec<String> = lines_of(&read[0].1).into_iter().map(|l| l.2).collect();
    assert_eq!(labels, ["Dana", "Kai", "Mina", "Kai", "Dana", "Mina", "user", "Kai"]);
    assert!(read[0].1.contains("e7 · 2026-11-02 09:06 · user · I like cooking for both of them."));
    let one = fill(SLEEP_ONE_PERSON.digest, &[("being", Arg::Text("Kai"))]);
    assert!(read[0].0.contains(SLEEP_MANY_PEOPLE.digest) && !read[0].0.contains(&one), "{}", read[0].0);
    let one = fill(SLEEP_ONE_PERSON.merge, &[("being", Arg::Text("Kai"))]);
    assert!(merge[0].0.contains(SLEEP_MANY_PEOPLE.merge) && !merge[0].0.contains(&one), "{}", merge[0].0);
    assert!(read[0].0.contains("A line marked \"user\" names no one") && !merge[0].0.contains("\"user\""));
    let items: Vec<(String, Value)> =
        res["items"].as_array().unwrap().iter().map(|it| (text_of(&it["topic"]), it["evidence"].clone())).collect();
    assert_eq!(
        items,
        [("Dana".into(), json!(["e1", "e5"])), ("Mina".into(), json!(["e3", "e6"])), ("me".into(), json!(["e7", "e8"]))]
    );
    assert_eq!(res["items_dropped_by_check"], json!(1), "the self on the people's lines alone");
    let marks: Vec<(String, String)> =
        res["mixed"].as_array().unwrap().iter().map(|x| (text_of(&x["memory"]), text_of(&x["kind"]))).collect();
    assert_eq!(marks, [("d5".to_string(), "echo".to_string()), ("d6".into(), "echo".into())]);
    let core = svc.lock().unwrap().latest(0).unwrap().body.clone();
    assert!(core.contains("## Dana\n- Dana is vegetarian.") && core.contains("## Mina\n- Mina works the night shift."), "{core}");
    assert_eq!(res["applied"].as_array().unwrap().len(), 2);
    let proposals: Vec<String> = res["proposals"].as_array().unwrap().iter().map(|p| text_of(&p["text"])).collect();
    assert!(proposals.is_empty(), "a view resting on the line with no speaker alone is a person's: {proposals:?}");
    assert_eq!(res["refused"].as_array().unwrap().len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn a_night_with_one_person_reads_the_one_person_texts() {
    let (svc, mem) = new();
    let mut rows = two_people_rows();
    rows.retain(|r| r["speaker"] != "Mina");
    give(&svc, rows);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let opts = SleepOpts { being: "Kai".into(), done: false, ..night("2026-11-02 23:00") };
    mem.sleep("u", two_people_model(seen.clone()), &opts).await.unwrap();
    let seen = seen.lock().unwrap().clone();
    let read = seen.iter().find(|(s, _)| s.contains(" you read part ")).unwrap();
    let labels: Vec<String> = lines_of(&read.1).into_iter().map(|l| l.2).collect();
    assert_eq!(labels, ["user", "Kai", "Kai", "user", "user", "Kai"]);
    let digest = fill(SLEEP_DIGEST, &[("being", Arg::Text("Kai")), ("k", Arg::Num(1)), ("n", Arg::Num(1))]);
    assert_eq!(read.0, digest);
    let merge = seen.iter().find(|(s, _)| s.contains(", finishing.")).unwrap();
    assert!(merge.0.contains(&fill(SLEEP_ONE_PERSON.merge, &[("being", Arg::Text("Kai"))])));
    assert!(!merge.0.contains(SLEEP_MANY_PEOPLE.merge));
}

#[tokio::test(flavor = "current_thread")]
async fn the_estimate_reads_counts_and_prices_and_the_sleep_reads_on_under_its_sleep() {
    let (svc, mem) = new();
    let rows = night_rows(1, DAYS[1]);
    give(&svc, rows.clone());
    let part = SleepSettings { part_tokens: 50_000, ..Default::default() };
    let priced = SleepEstimateOpts { input_price: Some(1.0), output_price: Some(5.0), settings: part, ..Default::default() };
    let est = mem.sleep_estimate("u", &priced).await.unwrap();
    let tokens: u64 = rows.iter().map(|r| r["tokens"].as_u64().unwrap() + 12).sum();
    let frame = sleep_tokens(SLEEP_DIGEST) + sleep_tokens("") + sleep_tokens("1: (empty)\n2: (empty)\n3: (empty)\n4: (empty)");
    let (sent, written) = (((tokens + frame) * 172).div_ceil(100), (tokens * 7).div_ceil(100));
    let basis = |prices: [f64; 2]| {
        json!({"sleep_version": "1.0.2", "model": "claude-sonnet-5-5", "effort": "high", "prompt_caching": false,
               "margin": 0.15, "prices": prices})
    };
    let cost = round_to((sent as f64 * 1.0 + written as f64 * 5.0) * 1.15 / 1_000_000.0, 6);
    assert_eq!(
        est,
        json!({"sleep_id": "s-1", "memories": 20, "tokens": tokens, "parts": 1, "readers": 1, "input_tokens": sent,
               "output_tokens": written, "cost": cost, "basis": basis([1.0, 5.0]), "sleepy": false})
    );
    assert_eq!(paths(&svc), ["sleep", "self"]);
    let opts = SleepOpts { sleep_id: est["sleep_id"].as_str().map(str::to_string), ..night("2026-09-15 23:00") };
    let nightly = sleep_with(&mem, &Arc::new(Model::default()), opts).await.unwrap();
    assert_eq!(nightly["sleep_id"], "s-1");
    assert_eq!(paths(&svc)[2..4], ["settings?user_id=u", "sleep"], "the estimate reads no settings; the night does");
    assert_eq!(svc.lock().unwrap().seen[3].body["sleep_id"], "s-1");
    assert_eq!(svc.lock().unwrap().sleeps.len(), 1, "no second sleep opened");
    let sleepy = SleepSettings { part_tokens: 50_000, sleepy_tokens: 1, ..Default::default() };
    let none = mem.sleep_estimate("u", &SleepEstimateOpts { settings: sleepy, ..Default::default() }).await.unwrap();
    assert_eq!((&none["cost"], &none["memories"]), (&json!(0.0), &json!(0)));
    give(&svc, night_rows(2, DAYS[2]));
    let chars: CountTokens = Arc::new(|t: &str| t.chars().count() as u64);
    let by_chars = SleepEstimateOpts { settings: sleepy, count_tokens: Some(chars), ..Default::default() };
    let est = mem.sleep_estimate("u", &by_chars).await.unwrap();
    assert_eq!(est["sleepy"], true);
    let want: u64 = night_rows(2, DAYS[2]).iter().map(|r| r["content"].as_str().unwrap().chars().count() as u64 + 12).sum();
    assert_eq!(est["tokens"], want);
    let cost = round_to(
        (est["input_tokens"].as_f64().unwrap() * 2.0 + est["output_tokens"].as_f64().unwrap() * 10.0) * 1.15 / 1_000_000.0,
        6,
    );
    assert_eq!((&est["cost"], &est["basis"]), (&json!(cost), &basis([2.0, 10.0])), "no prices: Claude Sonnet 5.5's, plus 15%");
    assert!(est["cost"].as_f64().unwrap() > 0.0);
    let n = svc.lock().unwrap().seen.len();
    for bad in [-1.0, f64::NAN, f64::INFINITY] {
        let o = SleepEstimateOpts { input_price: Some(bad), output_price: Some(1.0), ..Default::default() };
        assert!(mem.sleep_estimate("u", &o).await.unwrap_err().to_string().contains("input_price must be a price"));
    }
    for (i, o) in [(Some(1.0), None), (None, Some(5.0))] {
        let o = SleepEstimateOpts { input_price: i, output_price: o, ..Default::default() };
        assert!(mem.sleep_estimate("u", &o).await.unwrap_err().to_string().contains("together, or neither"));
    }
    assert_eq!(svc.lock().unwrap().seen.len(), n);
}

struct Wontopos;

impl ModelCaller for Wontopos {
    fn call<'a>(&'a self, _: &'a ModelCall) -> BoxFuture<'a, Result<ModelReply, ModelCallError>> {
        unreachable!("never called")
    }

    fn base_url(&self) -> Option<&str> {
        Some("https://api.wontopos.com")
    }
}

#[tokio::test(flavor = "current_thread")]
async fn arguments_are_refused_before_anything_is_sent() {
    let (svc, mem) = new();
    let model = Arc::new(Model::default());
    let local = openai_caller(OpenAiCallerOpts {
        base_url: "http://localhost:11434/v1".into(),
        model: "m".into(),
        ..Default::default()
    })
    .unwrap();
    let m: Arc<dyn ModelCaller> = model.clone();
    let host: Arc<dyn ModelCaller> = Arc::new(Wontopos);
    let local: Arc<dyn ModelCaller> = Arc::new(local);
    let cases: Vec<(Arc<dyn ModelCaller>, SleepOpts, &str)> = vec![
        (m.clone(), SleepOpts { settings: SleepSettings { readers: 0, ..Default::default() }, ..Default::default() }, "readers must be 1-200"),
        (m.clone(), SleepOpts { today: Some("yesterday".into()), ..Default::default() }, "today must be"),
        (m.clone(), SleepOpts { today: Some("2026-10-20T08:00".into()), ..Default::default() }, "today must be"),
        (m.clone(), SleepOpts { today: Some("2026-02-30".into()), ..Default::default() }, "today must be"),
        (m.clone(), SleepOpts { max_tokens: 0, ..Default::default() }, "max_tokens must be a whole number from 1 to 128000"),
        (m.clone(), SleepOpts { max_tokens: 128_001, ..Default::default() }, "max_tokens must be"),
        (m.clone(), SleepOpts { being: " ".into(), ..Default::default() }, "being is required"),
        (m.clone(), SleepOpts { since: Some(String::new()), ..Default::default() }, "since is required"),
        (m.clone(), SleepOpts { sleep_id: Some(" ".into()), ..Default::default() }, "sleep_id is required"),
        (host.clone(), SleepOpts::default(), "is a Wontopos host"),
        (local, SleepOpts { merge_llm: Some(host), ..Default::default() }, "is a Wontopos host"),
    ];
    for (llm, opts, want) in cases {
        let e = wos(mem.sleep("u", llm, &opts).await);
        assert_eq!(e.status(), Some(400));
        assert!(e.to_string().contains(want), "want {want:?}, got {e}");
    }
    for (opts, want) in [
        (
            SleepOpts { today: Some("yesterday".into()), ..Default::default() },
            "[400] today must be \"YYYY-MM-DD HH:MM\" or \"YYYY-MM-DD\", the sleep's day, got \"yesterday\".",
        ),
        (SleepOpts { being: " ".into(), ..Default::default() }, "[400] being is required."),
    ] {
        assert_eq!(wos(mem.sleep("u", m.clone(), &opts).await).to_string(), want);
    }
    assert!(wos(mem.sleep(" ", model.clone(), &SleepOpts::default()).await).to_string().contains("user_id must be a non-blank string"));
    assert!(svc.lock().unwrap().seen.is_empty());
    assert!(model.calls().is_empty());
    for today in ["2026-10-20", "2026-10-20 08:00"] {
        let (svc, mem) = new();
        let res = sleep_with(&mem, &model, SleepOpts { today: Some(today.into()), ..Default::default() }).await.unwrap();
        assert_eq!(res["today"], today);
        assert_eq!(paths(&svc).len(), 5);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn progress_hears_each_stage() {
    let (svc, mem) = new();
    give(&svc, night_rows(1, DAYS[1]));
    let heard = Arc::new(Mutex::new(Vec::<String>::new()));
    let h = heard.clone();
    let opts = SleepOpts { progress: Some(Arc::new(move |m: &str| h.lock().unwrap().push(m.to_string()))), ..night("2026-09-15 23:00") };
    sleep_with(&mem, &Arc::new(Model::default()), opts).await.unwrap();
    let heard = heard.lock().unwrap();
    assert_eq!(heard.len(), 3);
    assert!(heard[0].starts_with("20 memories, "), "{}", heard[0]);
    assert!(heard[1].starts_with("merging 4 items (0 combine rounds)"), "{}", heard[1]);
    assert!(heard[2].starts_with("done in ") && heard[2].ends_with("written [0, 1]"), "{}", heard[2]);
}

#[tokio::test(flavor = "current_thread")]
async fn sleep_read_since_the_last_sleep_or_an_instant_and_done() {
    let (svc, mem) = new();
    give(&svc, night_rows(1, DAYS[1]));
    let last = |svc: &Svc| svc.lock().unwrap().seen.last().map(|s| (s.body.clone(), s.headers.clone())).unwrap();
    let page = mem.sleep_read("u", &SleepReadOpts { since_last_sleep: Some(true), budget_tokens: Some(500), ..Default::default() }).await.unwrap();
    assert_eq!(last(&svc).0, json!({"user_id": "u", "budget_tokens": 500, "since_last_sleep": true}));
    let next = SleepReadOpts {
        since_last_sleep: Some(true),
        sleep_id: page["sleep_id"].as_str().map(str::to_string),
        cursor: Some(page["next_cursor"].clone()),
        ..Default::default()
    };
    mem.sleep_read("u", &next).await.unwrap();
    assert_eq!(last(&svc).0["sleep_id"], "s-1");
    mem.sleep_read("u", &SleepReadOpts { since: Some(" 2026-09-01T00:00:00Z ".into()), sleep_id: Some("s-1".into()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(last(&svc).0["since"], "2026-09-01T00:00:00Z");
    let r = mem.sleep_done(" s-1 ", "u").await.unwrap();
    assert_eq!(last(&svc).0, json!({"user_id": "u", "sleep_id": "s-1"}));
    assert!(last(&svc).1.contains_key("idempotency-key"));
    assert_eq!(r["changed"], true);
    let n = svc.lock().unwrap().seen.len();
    let timed = mem.with_tz(9.0).unwrap();
    let since = || Some("2026-09-01T00:00:00Z".to_string());
    for (opts, want) in [
        (SleepReadOpts { since_last_sleep: Some(true), since: since(), ..Default::default() }, "pass one of them"),
        (SleepReadOpts { since_last_sleep: Some(true), days_ago: Some(1), ..Default::default() }, "not with days_ago, room or today_start"),
        (SleepReadOpts { since: since(), room: Some("n1".into()), ..Default::default() }, "not with days_ago, room or today_start"),
        (SleepReadOpts { since_last_sleep: Some(true), today_start: since(), ..Default::default() }, "not with days_ago"),
        (SleepReadOpts { since: Some(" ".into()), ..Default::default() }, "since is required"),
    ] {
        let e = timed.sleep_read("u", &opts).await.unwrap_err().to_string();
        assert!(e.contains(want), "want {want:?}, got {e}");
    }
    assert!(mem.sleep_done("", "u").await.unwrap_err().to_string().contains("sleep_id is required"));
    assert_eq!(svc.lock().unwrap().seen.len(), n);
    timed.sleep_read("u", &SleepReadOpts { days_ago: Some(1), since_last_sleep: Some(false), ..Default::default() }).await.unwrap();
    assert!(last(&svc).0.get("since_last_sleep").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn the_file_and_note_routes() {
    let (svc, mem) = new();
    give(&svc, night_rows(1, DAYS[1]));
    let page = mem.sleep_read("u", &SleepReadOpts { since_last_sleep: Some(true), ..Default::default() }).await.unwrap();
    let sid = page["sleep_id"].as_str().unwrap().to_string();
    let ids: Vec<String> = svc.lock().unwrap().rows[..2].iter().map(|r| r["id"].as_str().unwrap().to_string()).collect();
    let last = |svc: &Svc| svc.lock().unwrap().seen.last().map(|s| (s.body.clone(), s.headers.clone())).unwrap();
    let w = mem
        .write_sleep_files(
            &sid,
            &[
                SleepFileWrite { slot: 0, body: "## Me\nI am Mira.\n".into(), day: Some("2026-10-20".into()), ..Default::default() },
                SleepFileWrite { slot: 2, title: "Garden".into(), body: "## Beds\n- Two.\n".into(), evidence: ids.clone(), ..Default::default() },
            ],
            "u",
        )
        .await
        .unwrap();
    let written: Vec<(u64, u64, u64)> = w["written"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x["slot"].as_u64().unwrap(), x["version"].as_u64().unwrap(), x["evidence_count"].as_u64().unwrap()))
        .collect();
    assert_eq!(written, [(0, 1, 0), (2, 1, 2)]);
    assert!(last(&svc).1.contains_key("idempotency-key"));
    assert_eq!(
        last(&svc).0["writes"][1],
        json!({"slot": 2, "base_version": 0, "title": "Garden", "body": "## Beds\n- Two.\n", "evidence": ids})
    );
    let files = mem.sleep_files("u").await.unwrap();
    assert_eq!(files["core"]["body"], "## Me\nI am Mira.\n");
    let listed: Vec<(u64, u64)> =
        files["files"].as_array().unwrap().iter().map(|f| (f["slot"].as_u64().unwrap(), f["version"].as_u64().unwrap())).collect();
    assert_eq!(listed, [(1, 0), (2, 1), (3, 0), (4, 0)]);
    assert!(files["files"][1].get("body").is_none());
    let again = SleepFileWrite { slot: 2, base_version: 1, title: "Garden".into(), body: "## Beds\n- Three.\n".into(), ..Default::default() };
    mem.write_sleep_files(&sid, &[again], "u").await.unwrap();
    assert_eq!(mem.open_sleep_file(2, "u", None).await.unwrap()["body"], "## Beds\n- Three.\n");
    assert_eq!(mem.open_sleep_file(2, "u", Some(1)).await.unwrap()["body"], "## Beds\n- Two.\n");
    assert_eq!(last(&svc).0, json!({"user_id": "u", "slot": 2, "version": 1}));
    let hist = mem.sleep_file_history(2, "u").await.unwrap();
    let versions: Vec<u64> = hist["versions"].as_array().unwrap().iter().map(|v| v["version"].as_u64().unwrap()).collect();
    assert_eq!(versions, [1, 2]);
    let notes = mem.sleep_notes("u").await.unwrap();
    assert_eq!((&notes["version"], &notes["notes"]), (&json!(0), &json!({"proposals": [], "mixed": []})));
    mem.write_sleep_notes(0, &json!({"proposals": [], "mixed": [{"memory": ids[0]}]}), "u").await.unwrap();
    assert_eq!(mem.sleep_notes("u").await.unwrap()["notes"]["mixed"], json!([{"memory": ids[0]}]));
    assert_eq!(mem.write_sleep_notes(0, &json!({}), "u").await.unwrap_err().kind(), ErrorKind::Conflict);
    let n = svc.lock().unwrap().seen.len();
    let one = |slot: u32| SleepFileWrite { slot, ..Default::default() };
    for (writes, want) in [
        (vec![], "writes must be a list of 1 to 5 writes, one a file."),
        (vec![one(0), one(1), one(2), one(3), one(4), one(1)], "writes must be a list of 1 to 5"),
        (vec![one(5)], "writes[0].slot must be 0 (the core) or 1 to 4 (a box), got 5."),
        (vec![SleepFileWrite { evidence: vec!["".into()], ..one(1) }], "writes[0].evidence must be a list of memory ids."),
        (vec![SleepFileWrite { day: Some("today".into()), ..one(1) }], "writes[0].day must be \"YYYY-MM-DD\", got \"today\"."),
        (vec![one(1), one(1)], "writes[1].slot 1 is written twice: one write a file."),
    ] {
        let e = mem.write_sleep_files(&sid, &writes, "u").await.unwrap_err().to_string();
        assert!(e.contains(want), "want {want:?}, got {e}");
    }
    for slot in [5, 99] {
        assert!(mem.open_sleep_file(slot, "u", None).await.unwrap_err().to_string().contains("slot must be 0"));
        assert!(mem.sleep_file_history(slot, "u").await.unwrap_err().to_string().contains("slot must be 0"));
    }
    assert!(mem.open_sleep_file(1, "u", Some(0)).await.unwrap_err().to_string().contains("version must be a whole number"));
    assert!(mem.write_sleep_notes(0, &json!([]), "u").await.unwrap_err().to_string().contains("notes must be an object"));
    assert!(mem.write_sleep_files(" ", &[one(1)], "u").await.unwrap_err().to_string().contains("sleep_id is required"));
    assert_eq!(svc.lock().unwrap().seen.len(), n);
}

fn version(n: i64, title: &str, body: &str, tokens: i64, day: &str) -> Version {
    Version {
        version: n,
        title: title.into(),
        body: body.into(),
        tokens,
        evidence: json!([]),
        day: json!(day),
        sleep_id: Value::Null,
    }
}

fn files_service() -> Service {
    let mut svc = Service::default();
    svc.slots[0] = vec![version(2, "", "## Jamie\n- Close friend.\n\n## Me\nI am the assistant.\n", 10, "2026-10-01")];
    svc.slots[1] = vec![version(1, "Garden", "## Beds\n- Two beds of tomatoes.\n", 9, "2026-09-28")];
    svc.slots[3] = vec![version(1, "Reading list", "  ", 1, "2026-10-05")];
    svc
}

fn scroll2(mem: &Client) -> Client {
    mem.with_model("scroll-2")
}

const TOOLS_TODAY: [((bool, bool, bool), &str); 8] = [
    ((false, false, false), "81b100c86e145fc85bde3044ad55b441397c80b8af53d4bf8fb5b7c2a2180bb7"),
    ((false, false, true), "bb8fafea3d722148d1fed9bdc480b02140594aeeb9e5ade744d29e6b09c32123"),
    ((false, true, false), "248413498d4644742be51182540d14641ee87aed84409f227948c18dec630caa"),
    ((false, true, true), "5da071c6a1de7190a4ea2fe7137f264f4b68ebe656b0ffa169b7fdbce13bf2e1"),
    ((true, false, false), "bfb41a8d660bb77f4a6b033b0d319ff6ffcf4f4ebb183d66a69436ea55272c96"),
    ((true, false, true), "c5fcbda4b02200b423c45d67c768f7e79b13278f2cc31a16d07ad2685e2a9f6e"),
    ((true, true, false), "4380b38d91910bdb804a133ce18cc3056e759731e4abccc71d00fe0c961775bd"),
    ((true, true, true), "e149cc1a01bd4883fe5ec1b10ffda10f0dbb6325a94bf405d28fa067ae9e8679"),
];

fn tools_digest(t: &crate::MemoryTools) -> String {
    let blob = json!([t.note(), t.definitions(ToolFormat::Anthropic), t.definitions(ToolFormat::OpenAi)]);
    sha256(serde_json::to_string(&blob).unwrap().as_bytes())
}

#[tokio::test(flavor = "current_thread")]
async fn memory_tools_with_sleep_off_are_the_tools_without_it_to_the_byte() {
    let (svc, mem) = new_with(files_service());
    let mem = scroll2(&mem);
    for ((speakers, dates, images), want) in TOOLS_TODAY {
        let on = |t: crate::MemoryTools| t.with_speakers(speakers).with_dates(dates).with_images(images);
        assert_eq!(tools_digest(&on(mem.memory_tools_preset("u", None, "S-2").unwrap())), want, "{speakers} {dates} {images}");
        assert_eq!(tools_digest(&on(mem.memory_tools_preset("u", None, "S-2").unwrap().with_sleep(false))), want);
        assert_eq!(tools_digest(&on(mem.memory_tools_preset("u", None, "S-2").unwrap().with_sleep(true).with_sleep(false))), want);
    }
    let tools = mem.memory_tools_preset("u", None, "S-2").unwrap();
    let names: Vec<String> = tools.definitions(ToolFormat::Anthropic).iter().map(|d| d["name"].as_str().unwrap().to_string()).collect();
    assert!(!names.contains(&"open_box".to_string()));
    let e = tools.run("open_box", &json!({"box": "1"})).await.unwrap_err().to_string();
    assert!(e.contains("not a memory tool"), "{e}");
    assert!(tools.load().await.unwrap_err().to_string().contains("make the tools with with_sleep(true)"));
    tools.begin("q").await.unwrap();
    assert_eq!(paths(&svc), ["work/open"]);
}

const USAGE_ON: &str = "3369ca49bce3f9085260d9e0b88cf61d115d6a3e6986f10a932ecb98856877cc";

#[test]
fn memory_tools_with_usage_are_pinned_and_without_it_are_the_tools_to_the_byte() {
    let mem = Client::with_base_url(KEY, "http://127.0.0.1:9").with_model("scroll-2");
    for ((speakers, dates, images), want) in TOOLS_TODAY {
        let t = mem.memory_tools_preset("u", None, "S-2").unwrap().with_speakers(speakers).with_dates(dates).with_images(images);
        assert_eq!(tools_digest(&t.with_usage(false)), want, "{speakers} {dates} {images}");
    }
    assert_eq!(tools_digest(&mem.memory_tools_preset("u", None, "S-2").unwrap().with_usage(true)), USAGE_ON);
}

#[tokio::test(flavor = "current_thread")]
async fn memory_tools_with_sleep_carry_the_core_the_boxes_and_the_reader_note() {
    let (svc, mem) = new_with(files_service());
    let mem = scroll2(&mem);
    let tools = mem.memory_tools_preset("u", None, "S-2").unwrap().with_sleep(true).with_today("2026-10-05").unwrap();
    let base = mem.memory_tools_preset("u", None, "S-2").unwrap().note();
    assert_eq!(tools.note(), base, "before the files are read, the note without them");
    assert!(paths(&svc).is_empty(), "the note reads nothing");
    tools.load().await.unwrap();
    let files = [
        "<core last edited 2026-10-01, 4 days ago>",
        "## Me",
        "I am the assistant.",
        "",
        "## Jamie",
        "- Close friend.",
        "</core>",
        "<boxes>",
        "1. Garden (last edited 2026-09-28, 7 days ago)",
        "2. (empty)",
        "3. Reading list (last edited 2026-10-05, today)",
        "4. (empty)",
        "</boxes>",
    ]
    .join("\n");
    assert_eq!(tools.note(), format!("{base}\n\n{SLEEP_READER_NOTE}\n{files}"));
    assert_eq!(paths(&svc), ["self"]);
    let defs = tools.definitions(ToolFormat::Anthropic);
    assert_eq!(defs[..defs.len() - 1], mem.memory_tools_preset("u", None, "S-2").unwrap().definitions(ToolFormat::Anthropic)[..]);
    assert_eq!(defs[defs.len() - 1], open_box_tool());
    let b = open_box_tool();
    assert_eq!(
        tools.definitions(ToolFormat::OpenAi).last().unwrap(),
        &json!({"type": "function", "function": {"name": "open_box", "description": b["description"], "parameters": b["input_schema"]}})
    );

    let opened = "<box 1 name=\"Garden\" last edited 2026-09-28, 7 days ago>\n## Beds\n- Two beds of tomatoes.\n</box>";
    assert_eq!(tools.run("open_box", &json!({"box": "1"})).await.unwrap(), opened);
    assert_eq!(tools.run_arguments("open_box", r#"{"box": "garden"}"#).await.unwrap(), opened);
    assert_eq!(tools.run_content("open_box", &json!({"box": 1})).await.unwrap(), crate::ToolContent::Text(opened.into()));
    assert_eq!(svc.lock().unwrap().seen.last().unwrap().body, json!({"user_id": "u", "slot": 1}));
    let n = svc.lock().unwrap().seen.len();
    assert_eq!(tools.run("open_box", &json!({"box": "2"})).await.unwrap(), "Box 2 is empty.");
    assert_eq!(svc.lock().unwrap().seen.len(), n, "an empty box asks nothing");
    assert_eq!(tools.run("open_box", &json!({"box": "Reading list"})).await.unwrap(), "Box 3 is empty.");
    let index = between(&files, "<boxes>\n", "\n</boxes>").to_string();
    assert_eq!(
        tools.run("open_box", &json!({"box": "Recipes"})).await.unwrap(),
        format!("No box by that name. The boxes are:\n{index}")
    );
    assert_eq!(tools.run_arguments("open_box", "{not json").await.unwrap(), "Refused: the arguments are not JSON.");
    svc.lock().unwrap().fail =
        vec![("/api/v1/memory/self/open".into(), 404, json!({"error": {"code": "self_version_not_found", "message": "gone"}}))];
    assert!(tools.run("open_box", &json!({"box": "1"})).await.unwrap().starts_with("Refused: [404]"));

    tools.begin("q").await.unwrap();
    let p = paths(&svc);
    assert_eq!(p.iter().filter(|x| *x == "self").count(), 1);
    assert_eq!(p.last().unwrap(), "work/open");
    svc.lock().unwrap().slots[0].push(version(3, "", "## Me\nI am the assistant, rested.\n", 9, "2026-10-05"));
    let note = tools.load().await.unwrap();
    assert!(note.contains("<core last edited 2026-10-05, today>\n## Me\nI am the assistant, rested.\n</core>"), "{note}");
    assert_eq!(note, tools.note());
}

#[tokio::test(flavor = "current_thread")]
async fn memory_tools_with_sleep_read_the_files_at_the_first_turn_and_date_them_on_the_clock() {
    let (svc, mem) = new_with(files_service());
    let mem = scroll2(&mem).with_tz(-12.0).unwrap();
    let tools = mem.memory_tools_preset("u", None, "S-2").unwrap().with_sleep(true);
    tools.begin("q").await.unwrap();
    assert_eq!(paths(&svc), ["self", "work/open"]);
    assert!(tools.note().contains("<core last edited 2026-10-01, "));
    let day = today_on(Some(-12.0));
    let want = format!("{} days ago", day - sleep_date("2026-10-01").unwrap());
    assert!(tools.note().contains(&want) || (day == sleep_date("2026-10-01").unwrap() && tools.note().contains("today")));
    let e = mem.memory_tools_preset("u", None, "S-2").unwrap().with_sleep(true).with_today("2026-13-01").unwrap_err().to_string();
    assert!(e.contains("today must be a day"), "{e}");
    let e = mem.memory_tools_preset("u", None, "S-2").unwrap().with_today("2026-10-01").unwrap_err().to_string();
    assert!(e.contains("it goes with with_sleep(true)"), "{e}");
    let plain = mem.memory_tools_preset("u", None, "S-2").unwrap();
    assert!(plain.load().await.unwrap_err().to_string().contains("make the tools with with_sleep(true)"));
}

mod attacks;
mod held;
mod hunt;
mod v102;
