    use super::tests::{http, mock_server_recording};
    use super::{Client, SearchOpts, WosError, WARNED_FILTER_KEYS};

    fn body(raw: &str) -> serde_json::Value {
        serde_json::from_str(raw.split("\r\n\r\n").nth(1).expect("no body")).expect("not JSON")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn filters_and_images_travel_together() {
        let payload = "{\"memories\":[{\"id\":\"m1\"}],\"self_memories\":[{\"id\":\"s1\"}],\
                       \"images\":[{\"id\":\"i1\"}],\"verify_used\":1}";
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", payload)]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let r = mem
            .search_full_with(
                "photos from june",
                "alice",
                10,
                &SearchOpts { max_images: Some(3), ..Default::default() },
                serde_json::json!({
                    "filters": {"event_from": "2026-06-01", "event_to": "2026-06-30"},
                    "speaker": "Bob",
                    "max_images": 1,
                    "user_id": "bob",
                    "query": "smuggled",
                    "max_results": 50,
                }),
            )
            .await
            .unwrap();
        assert_eq!(r.memories.len(), 1);
        assert_eq!(r.self_memories.len(), 1);
        assert_eq!(r.images.len(), 1);
        assert_eq!(r.verify_used, Some(1));
        let b = body(&seen.lock().unwrap()[0]);
        assert_eq!(b["filters"]["event_from"], "2026-06-01");
        assert_eq!(b["speaker"], "Bob");
        assert_eq!(b["max_images"], 3, "opts win over extra");
        assert_eq!(b["user_id"], "alice", "reserved fields win over extra");
        assert_eq!(b["query"], "photos from june");
        assert_eq!(b["max_results"], 10);
        assert!(b.get("verify").is_none(), "an unset option is not sent");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unknown_filters_are_warned_about_and_limits_checked() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"memories\":[]}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem
            .search_full_with("q", "alice", 50, &SearchOpts::default(), serde_json::json!({}))
            .await
        {
            Err(WosError::Api { status: 400, message, .. }) => assert!(message.contains("between 5 and 20")),
            other => panic!("expected a local refusal, got {other:?}"),
        }
        mem.search_full_with(
            "q",
            "alice",
            10,
            &SearchOpts::default(),
            serde_json::json!({"filters": {"evnt_from_sfw": "2026-01-01"}}),
        )
        .await
        .unwrap();
        let hit = WARNED_FILTER_KEYS
            .get()
            .map(|m| m.lock().unwrap().contains("evnt_from_sfw"))
            .unwrap_or(false);
        assert!(hit, "an unknown filter key must be warned about");
        assert_eq!(seen.lock().unwrap().len(), 1, "only the valid call is sent");
    }
