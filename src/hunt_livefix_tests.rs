    use super::*;
    use std::sync::atomic::Ordering;

    const KEY: &str = "wos-test-xxxxxxxxxx";

    fn search_answer() -> String {
        serde_json::json!({
            "memories": [{"id": "w1", "content": "The office moves to the fourth floor.", "category": "work"}],
            "self_memories": [
                {"id": "s1", "content": "You like green tea.", "speaker": "me", "category": "general"},
                {"id": "s2", "content": "hello", "speaker": "me", "category": "general"}
            ]
        })
        .to_string()
    }

    fn refusal<T: std::fmt::Debug>(r: Result<T, WosError>) -> String {
        match r {
            Err(WosError::Api { status: 400, message }) => message,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_filtered_search_leaves_out_the_assistants_words_the_filter_did_not_narrow() {
        let ok = || super::tests::http("200 OK", "", &search_answer());
        let (base, _) = super::tests::mock_server(vec![ok(), ok()]);
        let mem = Client::with_base_url(KEY, &base).with_retries(0);
        let filtered = mem.search_with("office", "u", 5, serde_json::json!({"filters": {"categories": ["work"]}})).await.unwrap();
        assert_eq!(filtered.iter().map(|m| m.id.as_deref()).collect::<Vec<_>>(), [Some("w1")]);
        let plain = mem.search("office", "u", 5).await.unwrap();
        assert_eq!(plain.iter().map(|m| m.id.as_deref()).collect::<Vec<_>>(), [Some("w1"), Some("s1"), Some("s2")]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_turn_with_both_sides_blank_is_refused_before_anything_is_sent() {
        let (base, hits) = super::tests::mock_server(vec![super::tests::http("200 OK", "", "{\"status\":\"ok\"}")]);
        let mem = Client::with_base_url(KEY, &base).with_retries(0);
        for (a, b) in [("", ""), ("  ", "\n")] {
            assert_eq!(
                refusal(mem.add_turn(a, b, "u").await),
                "user_msg and assistant_msg are both blank: there is no turn to store."
            );
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        mem.add_turn("hello", "", "u").await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn update_refuses_a_blank_old_memory_id_as_the_other_id_calls_do() {
        let (base, hits) = super::tests::mock_server(vec![]);
        let mem = Client::with_base_url(KEY, &base).with_retries(0);
        for bad in ["", "   "] {
            assert_eq!(refusal(mem.update(bad, "new text", "u").await), "old_memory_id is required.");
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    fn sleep_answers() -> Vec<String> {
        let night = serde_json::json!([
            {"id": "m-1", "content": "Dana planted roses today.", "created_at": "2026-10-19T09:00:00Z", "speaker": "Dana", "tokens": 8},
            {"id": "m-2", "content": "Kai said the roses need water.", "created_at": "2026-10-19T09:01:00Z", "speaker": "Kai", "tokens": 9}
        ]);
        let files: Vec<serde_json::Value> = (1..=4)
            .map(|slot| serde_json::json!({"slot": slot, "title": "", "version": 0, "tokens": 0, "written_at": null, "day": null}))
            .collect();
        let answers = [
            serde_json::json!({"user_id": "u", "sleep_id": "s-1", "memories": night, "count": 2, "tokens": 17, "exhausted": true,
                               "next_cursor": null, "clock": {"local_now": "2026-10-20T09:30:00+09:00"}}),
            serde_json::json!({"user_id": "u", "core": {"title": "", "version": 0, "tokens": 0, "written_at": null, "day": null, "body": ""},
                               "files": files, "notes_version": 0}),
            serde_json::json!({"user_id": "u", "version": 0, "written_at": null, "bytes": 30, "notes": {"proposals": [], "mixed": []}}),
            serde_json::json!({"user_id": "u", "sleep_id": "s-1", "changed": true, "last_sleep": {"until": 2}}),
        ];
        answers.iter().map(|a| super::tests::http("200 OK", "", &a.to_string())).collect()
    }

    fn sleep_opts() -> SleepOpts {
        SleepOpts { assistant: Some("Kai".into()), today: Some("2026-10-20 09:30".into()), ..Default::default() }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_sleep_whose_reading_failed_does_not_end_the_last_sleep() {
        let (base, seen) = super::tests::mock_server_recording(sleep_answers());
        let mem = Client::with_base_url(KEY, &base).with_retries(0);
        let unreadable = caller_fn(|_| async { Ok(ModelReply::new("")) });
        let res = mem.sleep("u", unreadable, &sleep_opts()).await.unwrap();
        assert_eq!(res["memories"], 2);
        assert!(res["log"].as_array().unwrap().contains(&serde_json::json!({"call": "unread parts", "parts": [1]})), "{}", res["log"]);
        assert!(res["done"].is_null(), "{}", res["done"]);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3, "{seen:?}");
        assert!(!seen.iter().any(|r| r.contains("/sleep/done")));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_sleep_whose_merge_came_back_unreadable_twice_does_not_end_the_last_sleep() {
        let (base, seen) = super::tests::mock_server_recording(sleep_answers());
        let mem = Client::with_base_url(KEY, &base).with_retries(0);
        let read = serde_json::json!({"items": [{"kind": "person", "topic": "Dana", "text": "Dana planted roses.", "evidence": ["e1", "e2"]}], "mixed": []});
        let llm = caller_fn(move |c: ModelCall| {
            let text = if c.system[0].contains("you read part") { read.to_string() } else { String::new() };
            async move { Ok(ModelReply::new(text)) }
        });
        let res = mem.sleep("u", llm, &sleep_opts()).await.unwrap();
        assert_eq!(res["items"].as_array().map(Vec::len), Some(1), "{}", res["items"]);
        let failed: Vec<&str> = res["log"].as_array().unwrap().iter().filter(|x| x["failed"] == true).filter_map(|x| x["call"].as_str()).collect();
        assert_eq!(failed, ["merge", "merge shorter"]);
        assert!(res["done"].is_null(), "{}", res["done"]);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3, "{seen:?}");
        assert!(!seen.iter().any(|r| r.contains("/sleep/done")));
    }
