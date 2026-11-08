    use super::rooms_work_tests::{count, refusal, scripted, KEY};
    use super::tests::{http, mock_server_recording};
    use super::{Client, SettingsOpts};
    use serde_json::json;

    #[tokio::test(flavor = "current_thread")]
    async fn what_a_call_is_given_is_trimmed_as_python_and_typescript_trim_it() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        mem.get("u", "\u{feff}m1").await.unwrap();
        mem.delete("u", "m1\u{85}").await.unwrap();
        mem.lineage("u", "\u{feff}m1\u{3000}").await.unwrap();
        mem.seen("\u{feff}2026-10-19T09:00:00Z", "u").await.unwrap();
        let zone = SettingsOpts { time_zone: Some("Asia/Seoul\u{feff}".into()), ..Default::default() };
        mem.set_settings("u", &zone).await.unwrap();
        mem.work_deeper("\u{feff}w-1\u{1c}", "u").await.unwrap();
        mem.add_with("x", "u", json!({}), json!({"image": {"data": "\u{feff}aG\u{feff}k=\u{85}"}})).await.unwrap();
        mem.delete_all("\u{1c}").await.unwrap();
        {
            let seen = seen.lock().unwrap();
            let ids: Vec<_> = seen[..3].iter().map(|s| s.body["memory_id"].clone()).collect();
            assert_eq!(ids, [json!("m1"), json!("m1\u{85}"), json!("m1")]);
            assert_eq!(
                [&seen[3].body["at"], &seen[4].body["time_zone"], &seen[5].body["work_id"]],
                [&json!("2026-10-19T09:00:00Z"), &json!("Asia/Seoul"), &json!("w-1\u{1c}")]
            );
            assert_eq!([&seen[6].body["image"]["data"], &seen[7].body["user_id"]], [&json!("aGk=\u{85}"), &json!("\u{1c}")]);
        }
        for (m, want) in [
            (refusal(mem.get("u", "\u{feff}").await), "memory_id is required"),
            (refusal(mem.seen("\u{feff}", "u").await), "at is required"),
            (refusal(mem.stats("\u{feff}").await), "user_id was passed but is blank"),
            (refusal(mem.delete_store(" \u{feff}").await), "user_id is required"),
            (refusal(mem.forget_speaker(["Dana"], "\u{feff}", false).await), "user_id is required"),
        ] {
            assert!(m.contains(want), "want {want:?}, got {m}");
        }
        assert_eq!(count(&seen), 8);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_key_and_a_base_url_are_trimmed_as_python_and_typescript_trim_them() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        Client::with_base_url(&format!("\u{feff}{KEY} "), &format!("\u{feff}{base}\u{3000}")).stats("u").await.unwrap();
        assert!(seen.lock().unwrap()[0].to_ascii_lowercase().contains(&format!("x-api-key: {KEY}\r\n")));
        for key in [format!("{KEY}\u{85}"), format!("\u{1c}{KEY}")] {
            let m = refusal(Client::with_base_url(&key, &base).stats("u").await);
            assert!(m.contains("api_key contains"), "{m}");
        }
        let m = refusal(Client::with_base_url(KEY, &format!("{base}\u{85}")).stats("u").await);
        assert!(m.contains("base_url"), "{m}");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_answer_that_starts_with_a_byte_order_mark_is_read_as_python_and_typescript_read_it() {
        let lock = "\u{feff}{\"type\":\"error\",\"error\":{\"type\":\"conflict_error\",\"message\":\"in flight\",\
                    \"request_id\":\"req_lock\",\"retry_after_ms\":100}}";
        let expired = "\u{feff}{\"type\":\"error\",\"error\":{\"type\":\"gone_error\",\"message\":\"This sleep ran out: \
                       read it again.\",\"request_id\":\"req_exp\",\"code\":\"sleep_expired\"}}";
        let (base, seen) = scripted(vec![
            (200, "\u{feff}{\"total_memories\": 3}"),
            (409, lock),
            (200, "\u{feff}{\"id\": \"m1\"}"),
            (410, expired),
        ]);
        let mem = Client::with_base_url(KEY, &base);
        assert_eq!(mem.stats("u").await.unwrap()["total_memories"], 3);
        assert_eq!(mem.add("x", "u", json!({})).await.unwrap()["id"], "m1");
        let e = mem.sleep_done("s-1", "u").await.unwrap_err();
        assert_eq!((e.status(), e.code(), e.is_gone()), (Some(410), Some("sleep_expired"), false));
        assert!(e.to_string().contains("(request_id: req_exp)"), "{e}");
        assert_eq!(count(&seen), 4);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_empty_cursor_is_left_out_as_python_and_typescript_leave_it_out() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        mem.list_memories("u", None, "").await.unwrap();
        mem.list_memories("u", None, "c-2").await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!((&seen[0].body, &seen[1].body["cursor"]), (&json!({"user_id": "u", "limit": 100}), &json!("c-2")));
    }
