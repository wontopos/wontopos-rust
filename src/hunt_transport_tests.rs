    use super::rooms_work_tests::{count, refusal, scripted, KEY};
    use super::Client;

    #[tokio::test(flavor = "current_thread")]
    async fn a_long_request_id_is_cut_as_python_and_typescript_cut_it() {
        let body = serde_json::json!({"error": {"type": "t".repeat(1000), "message": "bad key", "request_id": "r".repeat(1000)}})
            .to_string();
        let type_only = serde_json::json!({"error": {"type": "t".repeat(1000)}}).to_string();
        let (base, seen) = scripted(vec![(401, &body), (401, &type_only)]);
        let mem = Client::with_base_url(KEY, &base);
        let cut_id = format!("{}\u{2026}(truncated)", "r".repeat(256));
        assert_eq!(mem.stats("alice").await.unwrap_err().to_string(), format!("[401] bad key (request_id: {cut_id})"));
        assert_eq!(mem.stats("alice").await.unwrap_err().to_string(), format!("[401] {}", "t".repeat(1000)));
        assert_eq!(count(&seen), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_bad_key_names_the_same_fault_as_python_and_typescript() {
        let (base, seen) = scripted(vec![]);
        for (key, want) in [
            ("wos-abc\ndef", "api_key contains whitespace"),
            ("wos-abc\u{a0}def", "api_key contains whitespace"),
            ("wos-abc\u{85}def", "api_key contains a non-ASCII character"),
            ("wos-abc\u{1}def", "api_key contains a control character"),
        ] {
            let m = refusal(Client::with_base_url(key, &base).stats("u").await);
            assert!(m.starts_with(want), "{key:?}: want {want:?}, got {m}");
        }
        assert_eq!(count(&seen), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_blank_answer_is_an_empty_body_as_in_python_and_typescript() {
        let (base, _) = scripted(vec![(200, "  \r\n")]);
        let e = Client::with_base_url(KEY, &base).with_retries(0).stats("u").await.unwrap_err().to_string();
        assert!(e.starts_with("[200] empty response body "), "{e}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_409_asking_for_an_endless_wait_is_a_conflict_not_retried() {
        let body = format!(
            "{{\"error\":{{\"type\":\"conflict_error\",\"message\":\"busy\",\"retry_after_ms\":{}}}}}",
            "9".repeat(400)
        );
        let (base, seen) = scripted(vec![(409, &body)]);
        let e = Client::with_base_url(KEY, &base).add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(e.kind(), super::ErrorKind::Conflict, "{e}");
        assert_eq!(count(&seen), 1);
    }

    #[test]
    fn an_out_of_range_tz_is_shown_as_python_and_typescript_show_it() {
        for (tz, shown) in [(f64::INFINITY, "Infinity"), (f64::NEG_INFINITY, "-Infinity"), (1e300, "1e+300")] {
            let m = refusal(Client::new(KEY).with_tz(tz));
            assert!(m.ends_with(&format!("got {shown}.")), "{m}");
        }
    }

    #[test]
    fn a_long_retry_budget_keeps_waiting_about_8s() {
        for attempt in [4, 1023, 1024, u32::MAX] {
            let ms = super::backoff(attempt).as_millis();
            assert!((8000..=8250).contains(&ms), "{attempt}: {ms}");
        }
    }
