use super::*;

#[tokio::test(flavor = "current_thread")]
async fn today_takes_a_clock_of_hours_0_to_23_and_minutes_0_to_59() {
    let (svc, mem) = new();
    let model = Arc::new(Model::default());
    let m: Arc<dyn ModelCaller> = model.clone();
    for (today, want) in [
        ("2026-10-20 24:00", "today must be"),
        ("2026-10-20 23:60", "today must be"),
        ("2026-10-20 99:99", "today must be"),
        ("2026-10-20 23:59", "max_tokens must be"),
        ("2026-10-20 00:00", "max_tokens must be"),
        ("2026-10-20 19:05", "max_tokens must be"),
    ] {
        let opts = SleepOpts { today: Some(today.into()), max_tokens: 0, ..Default::default() };
        let e = wos(mem.sleep("u", m.clone(), &opts).await);
        assert!(e.to_string().contains(want), "{today}: want {want:?}, got {e}");
    }
    assert!(svc.lock().unwrap().seen.is_empty());
    assert!(model.calls().is_empty());
}

#[test]
fn a_long_run_of_digits_is_checked_for_a_date_in_one_pass() {
    let digits = "9".repeat(5_000);
    let started = std::time::Instant::now();
    assert!(!sleep_dated(&digits));
    assert!(!sleep_dated(&format!("{digits}-{digits}-{digits}")));
    assert!(sleep_dated(&format!("{digits}-10-20")));
    assert!(sleep_dated(&format!("20-10-{digits}")));
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
}

#[test]
fn a_heading_of_hash_marks_alone_keeps_them_as_its_name() {
    let got = sleep_sections("## #\n- a\n\n## ## Me\n- b\n\n## ##\n- c\n\n## \n- d");
    let want = [("#", "- a"), ("Me", "- b"), ("##", "- c"), ("", "- d")];
    assert_eq!(got, want.map(|(h, t)| (h.to_string(), t.to_string())).to_vec());
}

/// Reads each part into one item, except where it is told to answer with a text cut at its length (`Cut`) or with
/// a text that is not JSON (`Garbage`). `again` cuts only the ask that follows an answer that was not JSON;
/// `relative` numbers the evidence from e1, as a model that counts the lines it was shown does.
#[derive(Clone, Copy, PartialEq)]
enum Cut {
    /// Part 2 whole.
    Whole,
    /// Every piece of part 2.
    Every,
    /// Every ask of every part.
    All,
    /// Each part whole, and none of its pieces.
    EachWhole,
}

#[derive(Clone, Copy, PartialEq)]
enum Garbage {
    None,
    /// The first half of part 2.
    FirstHalf,
    /// Any piece of part 2 that does not start where the part starts.
    SecondHalf,
}

struct CutReader {
    cut: Cut,
    garbage: Garbage,
    again: bool,
    relative: bool,
    whole: Mutex<std::collections::HashMap<usize, (usize, String)>>,
    calls: Mutex<Vec<(usize, usize)>>,
}

impl CutReader {
    fn new(cut: Cut, garbage: Garbage, again: bool, relative: bool) -> Arc<Self> {
        Arc::new(Self { cut, garbage, again, relative, whole: Default::default(), calls: Default::default() })
    }

    fn of_two(&self) -> Vec<usize> {
        self.calls.lock().unwrap().iter().filter(|(k, _)| *k == 2).map(|(_, lines)| *lines).collect()
    }
}

impl ModelCaller for CutReader {
    fn call<'a>(&'a self, c: &'a ModelCall) -> BoxFuture<'a, Result<ModelReply, ModelCallError>> {
        let said = |text: String, stop: &str| ModelReply { text, stop: stop.into(), usage: None };
        let reply = match c.system[0].split_once(" you read part ") {
            None => said(r#"{"edits": [], "proposals": [], "notes": []}"#.into(), "end"),
            Some((_, rest)) => {
                let k: usize = rest.split(' ').next().unwrap().parse().unwrap();
                let held = c.user.split_once("<memories>\n").unwrap().1.split_once("\n</memories>").unwrap().0;
                let lines: Vec<&str> = held.lines().filter(|ln| !ln.is_empty()).collect();
                let id = |ln: &str| ln.split(' ').next().unwrap().to_string();
                self.calls.lock().unwrap().push((k, lines.len()));
                let (whole, starts) = self.whole.lock().unwrap().entry(k).or_insert((lines.len(), id(lines[0]))).clone();
                let piece = lines.len() < whole;
                let cut_here = match self.cut {
                    Cut::Whole => k == 2 && !piece,
                    Cut::Every => k == 2,
                    Cut::All => true,
                    Cut::EachWhole => !piece,
                };
                let garbage_here = k == 2
                    && piece
                    && match self.garbage {
                        Garbage::None => false,
                        Garbage::FirstHalf => id(lines[0]) == starts,
                        Garbage::SecondHalf => id(lines[0]) != starts,
                    };
                let asked_again = c.user.ends_with("Answer with the JSON object only.");
                if cut_here && asked_again == self.again {
                    said(r#"{"items": [{"kind": "work", "topic": "Nigh"#.into(), "max_tokens")
                } else if cut_here || garbage_here {
                    said("I cannot answer in JSON tonight.".into(), "end")
                } else {
                    let evidence =
                        if self.relative { ["e1".to_string(), "e2".to_string()] } else { [id(lines[0]), id(lines[1.min(lines.len() - 1)])] };
                    let item = json!({"kind": "work", "topic": format!("Night {k}"), "text": "Lines were said.", "evidence": evidence});
                    said(json!({"items": [item], "mixed": []}).to_string(), "end")
                }
            }
        };
        Box::pin(async move { Ok(reply) })
    }
}

fn halving_night(day: &str) -> SleepOpts {
    SleepOpts {
        today: Some(format!("{day} 23:00")),
        settings: SleepSettings { part_tokens: 50_000, ..Default::default() },
        ..Default::default()
    }
}

fn logged(res: &Value, entry: Value) -> bool {
    res["log"].as_array().unwrap().contains(&entry)
}

async fn slept(reader: &Arc<CutReader>, rows: Vec<Value>, day: &str) -> (Svc, Value) {
    let (svc, mem) = new();
    give(&svc, rows);
    let m: Arc<dyn ModelCaller> = reader.clone();
    let res = mem.sleep("u", m, &halving_night(day)).await.unwrap();
    (svc, res)
}

#[tokio::test(flavor = "current_thread")]
async fn an_answer_cut_at_its_length_is_not_asked_again_and_its_part_is_read_in_halves() {
    let reader = CutReader::new(Cut::Whole, Garbage::None, false, false);
    let (_, res) = slept(&reader, night_rows(0, DAYS[0]), DAYS[0]).await;
    let of_two = reader.of_two();
    assert_eq!(of_two.len(), 3, "the cut answer is not asked again; each half is asked once");
    assert_eq!(of_two[1] + of_two[2], of_two[0]);
    assert!(logged(&res, json!({"call": "unread parts", "parts": []})));
    assert!(logged(&res, json!({"call": "read 2", "cut": true, "halves": [of_two[1], of_two[2]]})));
    let asked: Vec<&str> = res["log"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x.get("attempt").is_some())
        .filter_map(|x| x["call"].as_str())
        .filter(|call| call.starts_with("read 2"))
        .collect();
    assert_eq!(asked, ["read 2", "read 2a", "read 2b"]);
    assert!(!res["done"].is_null());
    let items = res["items"].as_array().unwrap();
    assert_eq!(items.len() as u64, res["readers"].as_u64().unwrap() + 1, "both halves gave their item");
    let first: Vec<u64> = items.iter().map(|it| it["evidence"][0].as_str().unwrap()[1..].parse().unwrap()).collect();
    let mut sorted = first.clone();
    sorted.sort_unstable();
    assert_eq!(first, sorted, "the first half's item comes before the second half's");
}

#[tokio::test(flavor = "current_thread")]
async fn a_part_cut_at_every_size_is_halved_twice_and_asked_seven_times_in_all() {
    let reader = CutReader::new(Cut::Every, Garbage::None, false, false);
    let (svc, res) = slept(&reader, night_rows(0, DAYS[0]), DAYS[0]).await;
    let mut of_two = reader.of_two();
    assert_eq!(of_two.len(), 1 + 2 + 4);
    let whole = of_two[0];
    of_two.sort_unstable();
    assert_eq!(of_two[..4].iter().sum::<usize>(), whole, "the four smallest pieces are the part");
    assert!(of_two[0] >= 2, "no piece of one memory: an item needs two lines");
    assert!(logged(&res, json!({"call": "unread parts", "parts": [2]})));
    assert!(res["done"].is_null());
    assert!(svc.lock().unwrap().notes.is_none(), "nothing but proposals and marks goes into the notes");
}

#[tokio::test(flavor = "current_thread")]
async fn an_answer_cut_only_when_asked_again_is_not_halved() {
    let reader = CutReader::new(Cut::Every, Garbage::None, true, false);
    let (_, res) = slept(&reader, night_rows(0, DAYS[0]), DAYS[0]).await;
    let of_two = reader.of_two();
    assert_eq!(of_two, vec![of_two[0]; 6], "three tries of two asks each, the whole part every time");
    assert!(!res["log"].as_array().unwrap().iter().any(|x| x.get("cut").is_some()));
    assert!(logged(&res, json!({"call": "unread parts", "parts": [2]})));
}

#[tokio::test(flavor = "current_thread")]
async fn a_part_too_small_to_halve_is_asked_as_before_when_its_answer_is_cut() {
    let reader = CutReader::new(Cut::All, Garbage::None, false, false);
    let (_, res) = slept(&reader, night_rows(1, DAYS[1]).into_iter().take(3).collect(), DAYS[1]).await;
    assert_eq!(*reader.calls.lock().unwrap(), vec![(1, 3); 6]);
    assert!(!res["log"].as_array().unwrap().iter().any(|x| x.get("cut").is_some()));
    assert!(logged(&res, json!({"call": "unread parts", "parts": [1]})));
}

#[tokio::test(flavor = "current_thread")]
async fn a_renumbered_answer_of_a_half_is_logged_under_the_halfs_name() {
    let reader = CutReader::new(Cut::Whole, Garbage::None, false, true);
    let (_, res) = slept(&reader, night_rows(0, DAYS[0]), DAYS[0]).await;
    let named: Vec<&str> =
        res["log"].as_array().unwrap().iter().filter(|x| x.get("renumbered").is_some()).filter_map(|x| x["call"].as_str()).collect();
    assert!(named.contains(&"read 2a") && named.contains(&"read 2b"), "{named:?}");
    assert!(!named.contains(&"read 2"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_half_that_cannot_be_read_leaves_its_part_unread_after_one_try() {
    for garbage in [Garbage::SecondHalf, Garbage::FirstHalf] {
        let reader = CutReader::new(Cut::Whole, garbage, false, false);
        let (_, res) = slept(&reader, night_rows(0, DAYS[0]), DAYS[0]).await;
        let halves = res["log"].as_array().unwrap().iter().find(|x| x.get("cut").is_some()).unwrap()["halves"].clone();
        let (a, b) = (halves[0].as_u64().unwrap() as usize, halves[1].as_u64().unwrap() as usize);
        let want = if garbage == Garbage::SecondHalf { vec![a + b, a, b, b] } else { vec![a + b, a, a, b] };
        assert_eq!(reader.of_two(), want, "a piece is tried once: one ask, and one more when the answer is not JSON");
        assert!(logged(&res, json!({"call": "unread parts", "parts": [2]})));
        assert!(res["done"].is_null());
        assert_eq!(res["items"].as_array().unwrap().len() as u64, res["readers"].as_u64().unwrap(), "the half that was read still gives its item");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_part_is_halved_by_its_tokens_not_by_its_count() {
    let mut rows: Vec<Value> = night_rows(1, DAYS[1]).into_iter().take(8).collect();
    rows[0]["tokens"] = json!(5_000);
    let reader = CutReader::new(Cut::EachWhole, Garbage::None, false, false);
    let (_, res) = slept(&reader, rows, DAYS[1]).await;
    assert!(logged(&res, json!({"call": "read 1", "cut": true, "halves": [2, 6]})), "{}", res["log"]);
    assert!(logged(&res, json!({"call": "unread parts", "parts": []})));
}

#[test]
fn halving_does_not_overflow_on_token_counts_past_any_real_one() {
    let line = |tokens: u64| Line { eid: String::new(), mid: String::new(), at: String::new(), who: String::new(), mine: false, text: String::new(), tokens, ts: 0.0 };
    for sizes in [vec![10, u64::MAX - 5, 1, 1], vec![(1 << 63) - 5, (1 << 63) + 11, 1, 1], vec![u64::MAX; 9]] {
        let part: Vec<Line> = sizes.iter().map(|t| line(*t)).collect();
        let at = sleep_half(&part);
        assert!(at >= 2 && at <= part.len() - 2, "{sizes:?} halved at {at}");
    }
    let even: Vec<Line> = [10u64, 10, 10, 10, 10, 10].iter().map(|t| line(*t)).collect();
    assert_eq!(sleep_half(&even), 3);
}
