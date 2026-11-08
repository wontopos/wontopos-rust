    use super::rooms_work_tests::KEY;
    use super::tests::{http, mock_server_recording};
    use super::{base64, Client, MemoryTools, ToolContent, ToolFormat};
    use serde_json::json;

    const USAGE_TOOL: &str = "This week's plan usage in percent: used, left and today's (UTC), and when the week resets. \
                              Free: not a step.";
    const BILLED_NOTE: &str = "This call was paid from API credit: this week's plan usage is used up, so calls now cost \
                               money. Tell the user.";
    const NO_PLAN_WEEK: &str = "There is no plan week to show for this key.";
    const PLAN: &str = r#"{"window_days":7,"balance_cents":120,"subscription":{
        "plan":"pro","status":"active","overflow_to_balance":true,"week":{
            "used_percent":37.5,"resets_at":"2026-11-09T00:00:00Z","days":[
                {"day":"2026-11-02","used_percent":20},{"day":"2026-11-03","used_percent":0},
                {"day":"2026-11-04","used_percent":12.9},{"day":"2026-11-05","used_percent":0.1},
                {"day":"2026-11-06","used_percent":4.5}]}}}"#;
    const PLAN_WEEK: &str = "This week's plan usage: 37.5% used, 62.5% left.\n\
                             Today (2026-11-06, UTC) used 4.5% of the week.\n\
                             The week resets at 2026-11-09T00:00:00Z.";
    const HUGE_USED: &str = r#"{"subscription":{"week":{"used_percent":1e308,"days":[{"day":"2026-11-06","used_percent":-1e308}]}}}"#;
    const HUGE_LEFT: &str = r#"{"subscription":{"week":{"used_percent":-1e308,"days":[{"day":"2026-11-06","used_percent":1e308}]}}}"#;
    const HUGE_USED_WEEK: &str = "This week's plan usage: 100% used, 0% left.\nToday (2026-11-06, UTC) used 0% of the week.";
    const HUGE_LEFT_WEEK: &str = "This week's plan usage: 0% used, 100% left.\nToday (2026-11-06, UTC) used 100% of the week.";
    const OPEN_TEA: &str =
        r#"{"work_id":"w-1","memories":[{"n":1,"id":"m-1","content":"Tea at nine."}],"steps_left":5,"deeper_left":3}"#;
    const DEEPER_TEA: &str = r#"{"work_id":"w-1","memories":[{"n":2,"id":"m-2","content":"Green tea."}],"deeper_left":2}"#;
    const STEPS: [&str; 9] = [
        "begin",
        "memory_deeper",
        "memory_deeper",
        "begin",
        "memory_timeline",
        "begin",
        "memory_deeper",
        "begin",
        "memory_deeper",
    ];
    const BEGUN: &str = "[01] Tea at nine.\n(steps left 5 · deeper left 3)";
    const DEEPER: &str = "[02] Green tea.\n(deeper left 2)";
    const TIMELINE: &str = "In the order they happened, oldest first:\n(no date)\n[01] Tea at nine.";

    fn turns_script() -> Vec<String> {
        [
            ("X-WOS-Week-Used: 79\r\n", OPEN_TEA),
            ("X-WOS-Week-Used: 80\r\n", DEEPER_TEA),
            ("X-WOS-Week-Used: 81\r\n", DEEPER_TEA),
            ("X-WOS-Week-Used: 81\r\n", OPEN_TEA),
            ("X-WOS-Billed-To: balance\r\nX-WOS-Week-Used: 100\r\n", OPEN_TEA),
            ("X-WOS-Billed-To: balance\r\nX-WOS-Week-Used: 100\r\n", DEEPER_TEA),
            ("X-WOS-Week-Used: 12\r\n", OPEN_TEA),
            ("X-WOS-Week-Used: 79\r\n", DEEPER_TEA),
        ]
        .iter()
        .map(|(h, b)| http("200 OK", h, b))
        .collect()
    }

    async fn turns(tools: &MemoryTools) -> Vec<String> {
        let mut out = Vec::new();
        for step in STEPS {
            out.push(if step == "begin" {
                tools.begin("tea").await.unwrap()
            } else {
                tools.run(step, &json!({})).await.unwrap()
            });
        }
        out
    }

    #[tokio::test(flavor = "current_thread")]
    async fn usage_off_is_the_tools_and_the_results_without_it_to_the_byte() {
        let (base, _) = mock_server_recording(turns_script());
        let scroll2 = Client::with_base_url(KEY, &base).with_model("scroll-2");
        for (speakers, dates, seen) in [(false, false, false), (false, true, false), (true, true, false), (false, false, true)] {
            let plain = scroll2.memory_tools_preset("u", None, "S-2").unwrap().with_speakers(speakers).with_dates(dates).with_seen(seen);
            let off = scroll2.memory_tools_preset("u", None, "S-2").unwrap().with_speakers(speakers).with_dates(dates).with_seen(seen);
            let off = off.with_usage(false);
            for format in [ToolFormat::Anthropic, ToolFormat::OpenAi] {
                assert_eq!(off.definitions(format), plain.definitions(format));
            }
            assert_eq!(off.note(), plain.note());
        }
        let e = scroll2.memory_tools_preset("u", None, "S-2").unwrap().run("memory_usage", &json!({})).await.unwrap_err();
        assert!(e.to_string().contains(r#"not a memory tool: "memory_usage""#), "{e}");
        let tools = scroll2.memory_tools_preset("u", None, "S-2").unwrap().with_dates(true);
        assert_eq!(turns(&tools).await, [BEGUN, DEEPER, DEEPER, BEGUN, TIMELINE, BEGUN, DEEPER, BEGUN, DEEPER]);
        assert_eq!(scroll2.week_used(), Some(79));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn usage_on_adds_memory_usage_which_says_the_week_in_percent_with_no_turn_open() {
        let refused = r#"{"type":"error","error":{"type":"permission_error","message":"this key cannot read usage","request_id":"req_403"}}"#;
        let (base, seen) = mock_server_recording(vec![
            http("200 OK", "", PLAN),
            http("200 OK", "", r#"{"subscription":{"plan":"pro","status":"active","week":{"used_percent":100,"days":[]}}}"#),
            http("200 OK", "", r#"{"window_days":7,"balance_cents":4913}"#),
            http("200 OK", "", r#"{"subscription":{"plan":"pro","status":"inactive","week":null}}"#),
            http("200 OK", "", HUGE_USED),
            http("200 OK", "", HUGE_LEFT),
            http("403 Forbidden", "", refused),
        ]);
        let scroll2 = Client::with_base_url(KEY, &base).with_model("scroll-2");
        let tools = scroll2.memory_tools_preset("u", None, "S-2").unwrap().with_usage(true);
        let defs = tools.definitions(ToolFormat::Anthropic);
        let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["memory_deeper", "memory_around", "memory_search", "memory_usage"]);
        let schema = json!({"type": "object", "properties": {}});
        assert_eq!(defs[3], json!({"name": "memory_usage", "description": USAGE_TOOL, "input_schema": schema}));
        assert_eq!(
            tools.definitions(ToolFormat::OpenAi)[3],
            json!({"type": "function", "function": {"name": "memory_usage", "description": USAGE_TOOL, "parameters": schema}})
        );
        assert_eq!(tools.note(), scroll2.memory_tools_preset("u", None, "S-2").unwrap().note());
        for want in [
            PLAN_WEEK,
            "This week's plan usage: 100% used, 0% left.",
            NO_PLAN_WEEK,
            NO_PLAN_WEEK,
            HUGE_USED_WEEK,
            HUGE_LEFT_WEEK,
            "Refused: [403] this key cannot read usage (request_id: req_403)",
        ] {
            assert_eq!(tools.run("memory_usage", &json!({})).await.unwrap(), want);
        }
        let asked = seen.lock().unwrap();
        assert_eq!(asked.len(), 7);
        assert!(asked.iter().all(|r| r.starts_with("GET /api/v1/won/usage?days=7 ")), "{asked:?}");
        let all = scroll2.memory_tools_preset("u", None, "S-2").unwrap().with_seen(true).with_usage(true).with_sleep(true);
        let names: Vec<String> =
            all.definitions(ToolFormat::Anthropic).iter().map(|d| d["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names[names.len() - 4..], ["memory_recent", "memory_seen", "memory_usage", "open_box"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn usage_on_adds_one_line_a_turn_when_the_plan_week_is_at_80_or_api_credit_paid() {
        let (base, _) = mock_server_recording(turns_script());
        let scroll2 = Client::with_base_url(KEY, &base).with_model("scroll-2");
        let tools = scroll2.memory_tools_preset("u", None, "S-2").unwrap().with_dates(true).with_usage(true);
        let deeper_80 = format!("{DEEPER}\nThis week's plan usage is at 80%: tell the user.");
        let timeline_81 = format!("{TIMELINE}\nThis week's plan usage is at 81%: tell the user.");
        let billed = format!("{DEEPER}\n{BILLED_NOTE}");
        assert_eq!(
            turns(&tools).await,
            [BEGUN, deeper_80.as_str(), DEEPER, BEGUN, timeline_81.as_str(), BEGUN, billed.as_str(), BEGUN, DEEPER]
        );
        assert_eq!((scroll2.week_used(), scroll2.billed_to()), (Some(79), None), "the tools on the client's own model are it");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn usage_on_says_api_credit_only_on_a_result_whose_own_call_it_paid() {
        let paid = "X-WOS-Billed-To: balance\r\n";
        let refused = r#"{"type":"error","error":{"type":"invalid_request_error","message":"work_id is not open","request_id":"req_400"}}"#;
        let (base, seen) = mock_server_recording(vec![
            http("200 OK", paid, OPEN_TEA),
            http("400 Bad Request", "", refused),
            http("200 OK", paid, DEEPER_TEA),
            http("200 OK", paid, DEEPER_TEA),
        ]);
        let scroll2 = Client::with_base_url(KEY, &base).with_model("scroll-2");
        let tools = scroll2.memory_tools_preset("u", None, "S-2").unwrap().with_dates(true).with_usage(true);
        assert_eq!(tools.begin("tea").await.unwrap(), BEGUN);
        assert_eq!(tools.run("memory_timeline", &json!({})).await.unwrap(), TIMELINE, "it sends nothing");
        assert_eq!(tools.run_arguments("memory_deeper", "{").await.unwrap(), "Refused: the arguments are not JSON.");
        assert_eq!(
            tools.run("memory_deeper", &json!({})).await.unwrap(),
            "Refused: [400] work_id is not open (request_id: req_400)"
        );
        assert_eq!(tools.run("memory_deeper", &json!({})).await.unwrap(), format!("{DEEPER}\n{BILLED_NOTE}"));
        assert_eq!(tools.run("memory_deeper", &json!({})).await.unwrap(), DEEPER, "once a turn");
        assert_eq!(scroll2.billed_to(), Some("balance"));
        assert_eq!(
            scroll2.memory_tools_preset("u", None, "S-2").unwrap().with_usage(true).run("memory_deeper", &json!({})).await.unwrap(),
            "Nothing is open: memory tools answer within a turn that began with begin()."
        );
        assert_eq!(seen.lock().unwrap().len(), 4);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_week_and_who_paid_are_read_when_the_headers_arrive_though_the_body_then_drops() {
        let paid = "X-WOS-Billed-To: balance\r\nX-WOS-Week-Used: 91\r\n";
        let dropped = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 200\r\nConnection: close\r\n{paid}\r\n{{\"stat"
        );
        let (base, seen) = mock_server_recording(vec![
            dropped.clone(),
            http("503 Service Unavailable", "", r#"{"error": "busy"}"#),
            dropped,
            http("200 OK", "", PLAN),
        ]);
        let scroll2 = Client::with_base_url(KEY, &base).with_model("scroll-2").with_retries(1);
        let e = scroll2.stats("u").await.unwrap_err();
        assert!(matches!(e, super::WosError::Api { status: 503, .. }), "{e:?}");
        assert_eq!((scroll2.week_used(), scroll2.billed_to()), (Some(91), Some("balance")));
        let tools = scroll2.memory_tools_preset("u", None, "S-2").unwrap().with_usage(true);
        assert_eq!(tools.run("memory_usage", &json!({})).await.unwrap(), format!("{PLAN_WEEK}\n{BILLED_NOTE}"));
        assert_eq!((scroll2.week_used(), scroll2.billed_to()), (None, None));
        assert_eq!(seen.lock().unwrap().len(), 4);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn usage_on_leaves_a_picture_as_it_is_and_says_it_on_the_next_text() {
        let opened = r#"{"work_id":"w-1","memories":[{"n":1,"id":"id-1","content":"The beach at dusk.","image_ref":"img-1"}]}"#;
        let png = "not really a picture";
        let picture = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nX-WOS-Week-Used: 85\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{png}",
            png.len()
        );
        let (base, _) = mock_server_recording(vec![
            http("200 OK", "X-WOS-Week-Used: 85\r\n", opened),
            picture,
            http("200 OK", "X-WOS-Week-Used: 85\r\n", DEEPER_TEA),
        ]);
        let scroll2 = Client::with_base_url(KEY, &base).with_model("scroll-2");
        let tools = scroll2.memory_tools_preset("u", None, "S-2").unwrap().with_images(true).with_usage(true);
        assert_eq!(tools.begin("beach").await.unwrap(), "[01] (photo) The beach at dusk.");
        let b64 = base64(png.as_bytes());
        assert_eq!(
            tools.run_content("memory_photo", &json!({"n": 1})).await.unwrap(),
            ToolContent::Blocks(vec![json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": b64}})])
        );
        assert_eq!(
            tools.run("memory_deeper", &json!({})).await.unwrap(),
            format!("{DEEPER}\nThis week's plan usage is at 85%: tell the user.")
        );
    }
