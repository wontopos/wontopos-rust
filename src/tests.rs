    #[test]
    fn a_deadline_past_the_clocks_range_does_not_panic() {
        for d in [
            std::time::Duration::MAX,
            std::time::Duration::from_secs(u64::MAX / 2),
            std::time::Duration::from_secs(u64::MAX),
        ] {
            let c = Client::new("wos-live-kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk").with_deadline(d);
            let at = c.deadline_at().expect("a non-zero deadline is Some");
            assert!(at > std::time::Instant::now(), "{d:?} must resolve to a future instant");
            assert!(c.attempt_budget(Some(at), c.attempt_timeout("/api/v1/memory/store")).is_ok(), "{d:?} must leave a usable budget");
        }
        assert!(Client::new("wos-live-kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk")
            .with_deadline(std::time::Duration::ZERO)
            .deadline_at()
            .is_none());
    }

    #[test]
    fn the_largest_retry_count_still_retries() {
        let c = Client::new("wos-live-kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk").with_retries(u32::MAX);
        assert_eq!(c.retries.saturating_add(1), u32::MAX, "attempts must not wrap to 0");
    }

    #[test]
    fn with_user_blank_cannot_silently_become_the_default_store() {
        let bound = Client::new("wos-live-kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk").with_user("tenant-a");
        assert_eq!(bound.uid(None).unwrap(), "tenant-a");
        assert_eq!(bound.uid(Some("tenant-b")).unwrap(), "tenant-b");

        for blank in ["", " ", "\t", "\n"] {
            let c = Client::new("wos-live-kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk").with_user(blank);
            let err = c.uid(None).unwrap_err();
            assert!(
                format!("{err}").contains("default store is blank"),
                "with_user({blank:?}) must not resolve to a store: {err}"
            );
            assert!(c.uid(Some(blank)).is_err(), "uid(Some({blank:?}))");
        }

        assert_eq!(
            Client::new("wos-live-kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk").uid(None).unwrap(),
            "default"
        );
    }

    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    pub(super) fn mock_server_recording(responses: Vec<String>) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let s = seen.clone();
        std::thread::spawn(move || {
            for resp in responses {
                let Ok((mut sock, _)) = listener.accept() else { return };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 2048];
                loop {
                    match sock.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                s.lock().unwrap().push(String::from_utf8_lossy(&buf).to_string());
                let _ = sock.write_all(resp.as_bytes());
            }
        });
        (format!("http://{}", addr), seen)
    }

    pub(super) fn mock_server(responses: Vec<String>) -> (String, Arc<AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        std::thread::spawn(move || {
            for resp in responses {
                let Ok((mut sock, _)) = listener.accept() else { return };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 2048];
                loop {
                    match sock.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                h.fetch_add(1, Ordering::SeqCst);
                let _ = sock.write_all(resp.as_bytes());
            }
        });
        (format!("http://{}", addr), hits)
    }

    pub(super) fn http(status: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        )
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retries_429_then_succeeds() {
        let (base, hits) = mock_server(vec![
            http("429 Too Many Requests", "Retry-After: 0\r\n", "{\"error\":\"rate limited\"}"),
            http("200 OK", "", "{\"memories\":[]}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let out = mem.search("q", "alice", 10).await.unwrap();
        assert!(out.is_empty());
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_image_refuses_a_bad_key_before_the_network() {
        let (base, hits) = mock_server(vec![]);
        for bad in ["", "wos-live-\u{0}abc", "wos-live-\u{1}abc", "wos live"] {
            let mem = Client::with_base_url(bad, &base);
            match mem.get_image(None, "11111111-1111-1111-1111-111111111111").await {
                Err(WosError::Api { status, .. }) => assert_eq!(status, 400, "key {bad:?}"),
                other => panic!("key {bad:?} should be refused, got {other:?}"),
            }
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing may reach the network");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_count_out_of_range_is_refused_not_adjusted() {
        let (base, hits) = mock_server(vec![]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for bad in [0usize, 1, 4, 21, 50, 100] {
            for r in [
                mem.search("q", "alice", bad).await.map(|_| ()),
                mem.search_full("q", "alice", bad, &SearchOpts::default()).await.map(|_| ()),
                mem.search_self("q", "alice", bad).await.map(|_| ()),
            ] {
                match r {
                    Err(WosError::Api { status, message, .. }) => {
                        assert_eq!(status, 400, "a caller's mistake reads as a bad request");
                        assert!(message.contains("between 5 and 20"), "got {message}");
                    }
                    other => panic!("limit {bad} should be refused, got {other:?}"),
                }
            }
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing may reach the network");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn no_retry_on_400_and_parses_envelope() {
        let (base, hits) = mock_server(vec![http(
            "400 Bad Request",
            "",
            "{\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"boom\",\"request_id\":\"req_123\"}}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.search("q", "alice", 10).await {
            Err(WosError::Api { status, message, .. }) => {
                assert_eq!(status, 400);
                assert!(message.contains("boom"));
                assert!(message.contains("req_123"));
            }
            other => panic!("expected Api error, got {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn refuses_redirects() {
        let (base, _) = mock_server(vec![http("302 Found", "Location: http://evil.example/\r\n", "")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.stats("alice").await {
            Err(WosError::Api { status, message, .. }) => {
                assert_eq!(status, 302);
                assert!(message.contains("redirect"));
            }
            other => panic!("expected redirect refusal, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn delete_requires_memory_id() {
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:9");
        match mem.delete("alice", "").await {
            Err(WosError::Api { status, message, .. }) => {
                assert_eq!(status, 400);
                assert!(message.contains("delete_all"));
            }
            other => panic!("expected guard error, got {other:?}"),
        }
        match mem.delete_all("").await {
            Err(WosError::Api { status, .. }) => assert_eq!(status, 400),
            other => panic!("expected guard error, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn image_calls_require_a_memory_id() {
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:9");
        for r in [
            mem.get_image("alice", "").await.err(),
            mem.get_image("alice", "   ").await.err(),
            mem.forget_image("alice", "", false).await.err(),
            mem.forget_image("alice", "  ", true).await.err(),
            mem.lineage("alice", "").await.err(),
            mem.lineage("alice", "   ").await.err(),
        ] {
            match r {
                Some(WosError::Api { status, message, .. }) => {
                    assert_eq!(status, 400);
                    assert!(message.contains("memory_id is required"), "got {message}");
                }
                other => panic!("expected guard error, got {other:?}"),
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_call_that_names_no_model_carries_the_default() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.stats("alice").await.unwrap();
        let reqs = seen.lock().unwrap();
        assert!(
            reqs[0].to_lowercase().contains(&format!("x-wos-model: {DEFAULT_MODEL}").to_lowercase()),
            "got {}",
            reqs[0]
        );
    }

    pub(super) fn dropping_server(drops: usize, tail: String) -> (String, Arc<AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        std::thread::spawn(move || {
            let mut served = 0usize;
            while let Ok((mut sock, _)) = listener.accept() {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 2048];
                loop {
                    match sock.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                h.fetch_add(1, Ordering::SeqCst);
                if served < drops {
                    let _ = sock.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 512\r\n\r\n{\"co",
                    );
                } else {
                    let _ = sock.write_all(tail.as_bytes());
                }
                served += 1;
                let _ = sock.shutdown(std::net::Shutdown::Both);
            }
        });
        (format!("http://{}", addr), hits)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_is_retried_when_the_body_drops_mid_stream() {
        let (base, hits) = dropping_server(1, http("200 OK", "", r#"{"collections":[],"count":0}"#));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_retries(2);
        let got = mem.list_stores().await;
        assert!(got.is_ok(), "a dropped GET body should be retried, got {got:?}");
        assert_eq!(hits.load(Ordering::SeqCst), 2, "one drop + one good response");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_write_is_never_retried_when_the_body_drops_mid_stream() {
        let (base, hits) = dropping_server(1, http("200 OK", "", r#"{"status":"created"}"#));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_retries(2);
        let got = mem.create_store("alice").await;
        assert!(got.is_err(), "a dropped POST body must NOT be retried without a key");
        assert_eq!(hits.load(Ordering::SeqCst), 1, "the write must be attempted exactly once");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_memory_write_is_retried_when_the_body_drops_mid_stream() {
        let (base, hits) = dropping_server(1, http("200 OK", "", r#"{"id":"m1","status":"stored"}"#));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_retries(2);
        let got = mem.add("hello", "alice", serde_json::json!({})).await.unwrap();
        assert_eq!(got["id"], "m1");
        assert_eq!(hits.load(Ordering::SeqCst), 2, "one drop + one good response");
    }

    #[test]
    fn debug_never_shows_the_key() {
        let mem = Client::new("wos-live-supersecretkeyvalue1234");
        let dbg = format!("{mem:?}");
        assert!(!dbg.contains("supersecretkeyvalue"));
        assert!(dbg.contains("\"***1234\""));
    }

    #[test]
    fn backoff_honors_retry_after_and_caps() {
        assert_eq!(retry_wait(0, Some("2")).unwrap().as_millis(), 2000);
        assert_eq!(retry_wait(0, Some("30")).unwrap().as_millis(), 30_000);
        assert_eq!(retry_wait(0, Some("999")), None);
        let b = retry_wait(3, None).unwrap().as_millis();
        assert!((4000..=4250).contains(&b), "got {b}");
        let cap = retry_wait(10, None).unwrap().as_millis();
        assert!((8000..=8250).contains(&cap), "got {cap}");
    }

    #[test]
    fn only_delta_seconds_or_an_http_date_count_as_retry_after() {
        for junk in ["-5", "5, 10", "0x2", "garbage", "", " ", "1.5", "+5", "inf", "NaN", "1e3", "٣"] {
            let d = retry_wait(0, Some(junk)).expect("junk falls back to the backoff").as_millis();
            assert!((500..750).contains(&d), "{junk:?} waited {d}ms instead of the backoff");
        }
        assert_eq!(retry_wait(0, Some("0")).unwrap().as_millis(), 0);
        assert_eq!(retry_wait(0, Some("99999999999999999999999")), None, "huge is still above the cap");
    }

    #[test]
    fn key_is_trimmed() {
        let mem = Client::new(" wos-test-xxxxxxxxxx\n");
        let dbg = format!("{mem:?}");
        assert_eq!(mem.api_key, "wos-test-xxxxxxxxxx");
        assert!(dbg.contains("\"***\""));
        assert!(!dbg.contains('\n'));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn invalid_model_name_errors_before_sending() {
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:9").with_model("bad model");
        match mem.stats("alice").await {
            Err(WosError::Api { status, message, .. }) => {
                assert_eq!(status, 400);
                assert!(message.contains("invalid model name"));
            }
            other => panic!("expected model validation error, got {other:?}"),
        }
    }

    #[test]
    fn from_env_reads_the_key() {
        std::env::set_var("WONTOPOS_API_KEY", "wos-test-envkey12345");
        let mem = Client::from_env().unwrap();
        assert_eq!(mem.api_key, "wos-test-envkey12345");
        std::env::remove_var("WONTOPOS_API_KEY");
        std::env::remove_var("WOS_API_KEY");
        assert!(Client::from_env().is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn refuses_oversized_responses() {
        let (base, _) = mock_server(vec![format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{{}}",
            65 * 1024 * 1024
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.stats("alice").await {
            Err(WosError::Api { message, .. }) => assert!(message.contains("too large"), "got {message}"),
            other => panic!("expected size-cap error, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn delete_all_rejects_blank_and_whitespace() {
        let mem = Client::new("wos-test-xxxxxxxxxx");
        for uid in ["", " ", "\t", "\n", "   "] {
            assert!(mem.delete_all(uid).await.is_err(), "uid {uid:?} must be rejected");
        }
    }

    #[test]
    fn backoff_honors_retry_after_seconds_and_http_date() {
        assert_eq!(retry_wait(0, Some("2")).unwrap().as_millis(), 2000);
        assert_eq!(retry_wait(3, Some("Wed, 21 Oct 2015 07:28:00 GMT")).unwrap().as_millis(), 0);
        let future = httpdate::fmt_http_date(std::time::SystemTime::now() + std::time::Duration::from_secs(5));
        let d = retry_wait(0, Some(&future)).unwrap().as_millis();
        assert!(d > 0 && d <= 5_000, "got {d}");
        let later = httpdate::fmt_http_date(std::time::SystemTime::now() + std::time::Duration::from_secs(3600));
        assert_eq!(retry_wait(0, Some(&later)), None);
    }


    #[tokio::test(flavor = "current_thread")]
    async fn non_object_2xx_body_is_error() {
        for body in ["null", "12345", "\"hi\"", "[1,2,3]"] {
            let (base, _) = mock_server(vec![http("200 OK", "", body)]);
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
            match mem.stats("u").await {
                Err(WosError::Api { message, .. }) => assert!(message.contains("object"), "got {message}"),
                other => panic!("expected object error for {body}, got {other:?}"),
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn null_scalar_fields_default_instead_of_failing_the_batch() {
        let body = "{\"memories\":[\
            {\"id\":\"1\",\"content\":\"good\",\"similarity\":0.9},\
            {\"id\":\"2\",\"content\":null,\"category\":null,\"similarity\":null,\"importance\":null,\"is_superseded\":null}\
        ]}";
        let (base, _) = mock_server(vec![http("200 OK", "", body)]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let out = mem.search("q", "u", 5).await.expect("null scalars must not fail the batch");
        assert_eq!(out.len(), 2, "both memories survive the null element");
        assert_eq!(out[0].content, "good");
        assert_eq!(out[1].content, "");
        assert_eq!(out[1].similarity, 0.0);
        assert!(!out[1].is_superseded);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_self_returns_both_fields_null_self_is_empty() {
        let (base, _) = mock_server(vec![
            http("200 OK", "", "{\"memories\":[{\"id\":\"m1\"}],\"self_memories\":[{\"id\":\"s1\"}]}"),
            http("200 OK", "", "{\"memories\":[{\"id\":\"m2\"}],\"self_memories\":null}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let r = mem.search_self("q", "alice", 5).await.unwrap();
        assert_eq!(r.memories.len(), 1);
        assert_eq!(r.self_memories.len(), 1);
        assert_eq!(r.self_memories[0].id.as_deref(), Some("s1"));
        let r2 = mem.search_self("q", "u", 5).await.unwrap();
        assert_eq!(r2.self_memories.len(), 0);
        assert_eq!(r2.memories[0].id.as_deref(), Some("m2"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_returns_both_fields() {
        let (base, _) = mock_server(vec![http(
            "200 OK",
            "",
            "{\"memories\":[{\"id\":\"m1\",\"content\":\"partner said this\"}],\
              \"self_memories\":[{\"id\":\"s1\",\"content\":\"I said this\",\"speaker\":\"me\"},\
                                 {\"id\":\"m1\",\"content\":\"dup\"}]}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let out = mem.search("q", "u", 5).await.unwrap();
        let ids: Vec<&str> = out.iter().filter_map(|m| m.id.as_deref()).collect();
        assert_eq!(ids, vec!["m1", "s1"], "both fields, id-deduplicated");
        assert_eq!(out[1].speaker.as_deref(), Some("me"), "self_memories keeps its speaker");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bad_elements_are_skipped_not_fatal() {
        let (base, _) = mock_server(vec![
            http("200 OK", "", "{\"memories\":[null,1,\"x\",[],{\"id\":\"ok\",\"content\":\"c\"}]}"),
            http(
                "200 OK",
                "",
                "{\"memories\":[null,{\"id\":\"m1\"}],\"self_memories\":[2,{\"id\":\"s1\"}]}",
            ),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let out = mem.search("q", "u", 5).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id.as_deref(), Some("ok"));
        let r = mem.search_self("q", "u", 5).await.unwrap();
        assert_eq!(r.memories.len(), 1);
        assert_eq!(r.memories[0].id.as_deref(), Some("m1"));
        assert_eq!(r.self_memories.len(), 1);
        assert_eq!(r.self_memories[0].id.as_deref(), Some("s1"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_coerces_null_or_nonarray_memories_to_empty() {
        for body in ["{\"memories\":null}", "{\"memories\":\"oops\"}", "{\"memories\":42}", "{}"] {
            let (base, _) = mock_server(vec![http("200 OK", "", body), http("200 OK", "", body)]);
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
            let out = mem.search("q", "u", 5).await.expect("null/non-array memories must coerce to []");
            assert!(out.is_empty(), "search: expected [] for {body}");
            let out2 = mem.search_with("q", "u", 5, serde_json::json!({"speaker": "me"})).await.expect("search_with too");
            assert!(out2.is_empty(), "search_with: expected [] for {body}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn recall_with_and_engram_with_send_form() {
        let (base, _) = mock_server(vec![http("200 OK", "", "{\"short_term\":{},\"long_term\":{},\"context\":\"\"}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let r = mem.recall_with("q", "u", serde_json::json!({"form": "archive", "tz": 9})).await;
        assert!(r.is_ok(), "recall_with should send form and parse the response");
    }

    #[test]
    fn error_message_is_capped() {
        let long = "Z".repeat(100_000);
        let (msg, _) = parse_error(&format!("{{\"error\":\"{long}\"}}"));
        assert!(msg.chars().count() <= MAX_ERR_MSG + 20);
        assert!(msg.contains("truncated"));
        let multi = "\u{4E00}".repeat(100_000);
        let (m2, _) = parse_error(&format!("{{\"error\":\"{multi}\"}}"));
        assert!(m2.chars().count() <= MAX_ERR_MSG + 20);
    }

    #[test]
    fn user_agent_carries_platform_info() {
        let ua = user_agent();
        assert!(ua.starts_with("wontopos-rust/"), "got {ua}");
        assert!(ua.contains(std::env::consts::OS) && ua.contains(std::env::consts::ARCH), "got {ua}");
    }

    #[test]
    fn mask_key_handles_unicode_without_panic() {
        let masked = mask_key("wos-caf\u{e9}-\u{4E00}\u{4E8C}-se\u{f1}or-key-\u{4E09}\u{56DB}");
        assert_eq!(masked, "***y-\u{4E09}\u{56DB}");
        assert_eq!(mask_key("sh\u{f6}rt"), "***");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn post_write_not_retried_on_502() {
        let (base, hits) = mock_server(vec![http("502 Bad Gateway", "", "{\"error\":\"bad gateway\"}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let r = mem.add_speaker("Bob", "u").await;
        assert!(matches!(r, Err(WosError::Api { status: 502, .. })));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_rate_limit_header_is_read_as_a_whole_number_as_every_client_reads_it() {
        let cases: [(&str, Option<u64>); 12] = [
            ("60", Some(60)),
            ("007", Some(7)),
            ("9007199254740991", Some(9_007_199_254_740_991)),
            ("9007199254740992", None),
            ("-3", None),
            ("1.5", None),
            ("1e3", None),
            ("0x10", None),
            ("+5", None),
            ("1_000", None),
            ("12abc", None),
            ("", None),
        ];
        let script = cases
            .iter()
            .map(|(v, _)| http("200 OK", &format!("X-RateLimit-Limit: {v}\r\nX-RateLimit-Remaining: 1\r\nX-RateLimit-Reset: 2\r\n"), "{}"))
            .collect();
        let (base, _) = mock_server(script);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for (v, want) in cases {
            mem.stats("u").await.unwrap();
            assert_eq!(mem.rate_limit(), Some(RateLimit { limit: want, remaining: Some(1), reset: Some(2) }), "{v:?}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn billed_to_says_when_api_credit_paid_and_clears_after() {
        let (base, _) = mock_server(vec![
            http("200 OK", "X-WOS-Billed-To: balance\r\n", "{}"),
            http("200 OK", "", "{}"),
            http(
                "402 Payment Required",
                "",
                "{\"type\":\"error\",\"error\":{\"type\":\"billing_error\",\"message\":\"This week's subscription usage is used up.\",\"reason\":\"weekly_limit_reached\",\"resets_at\":\"2026-11-01T01:00:00.000Z\"}}",
            ),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert_eq!(mem.billed_to(), None);
        mem.usage(7).await.unwrap();
        assert_eq!(mem.billed_to(), Some("balance"));
        mem.usage(7).await.unwrap();
        assert_eq!(mem.billed_to(), None);
        let e = mem.usage(7).await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::PaymentRequired);
        assert!(e.to_string().contains("used up"), "{e}");
        assert!(!e.to_string().contains("resets_at"), "{e}");
        assert_eq!((e.reason(), e.resets_at()), (Some("weekly_limit_reached"), Some("2026-11-01T01:00:00.000Z")));
        assert_eq!(e.code(), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn week_used_is_the_whole_percent_the_last_answer_carried() {
        let cases: [(Option<&str>, Option<u8>); 15] = [
            (Some("37"), Some(37)),
            (Some("0"), Some(0)),
            (Some("100"), Some(100)),
            (Some("007"), Some(7)),
            (Some("000000000000000000000050"), Some(50)),
            (None, None),
            (Some("101"), None),
            (Some("256"), None),
            (Some("-1"), None),
            (Some("37.5"), None),
            (Some("1e2"), None),
            (Some("+5"), None),
            (Some("0x10"), None),
            (Some("12abc"), None),
            (Some(""), None),
        ];
        let mut script: Vec<String> = cases
            .iter()
            .map(|(v, _)| http("200 OK", &v.map(|v| format!("X-WOS-Week-Used: {v}\r\n")).unwrap_or_default(), "{}"))
            .collect();
        let used_up = "{\"type\":\"error\",\"error\":{\"type\":\"billing_error\",\"message\":\"This week's subscription usage is used up.\",\"reason\":\"weekly_limit_reached\",\"resets_at\":\"2026-11-01T01:00:00.000Z\"}}";
        script.push(http("200 OK", "X-WOS-Week-Used: 91\r\n", "{}"));
        script.push(http("402 Payment Required", "X-WOS-Week-Used: 100\r\n", used_up));
        script.push(http("502 Bad Gateway", "", "{\"error\":\"bad gateway\"}"));
        script.push(http("402 Payment Required", "X-WOS-Week-Used: 101\r\n", used_up));
        script.push(http("200 OK", "", "{}"));
        let (base, _) = mock_server(script);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_retries(0);
        assert_eq!(mem.week_used(), None);
        for (v, want) in cases {
            mem.stats("u").await.unwrap();
            assert_eq!(mem.week_used(), want, "{v:?}");
        }
        mem.stats("u").await.unwrap();
        assert_eq!(mem.week_used(), Some(91));
        assert_eq!(mem.stats("u").await.unwrap_err().kind(), ErrorKind::PaymentRequired);
        assert_eq!(mem.week_used(), Some(100), "a refusal that carries it sets it");
        assert_eq!(mem.stats("u").await.unwrap_err().kind(), ErrorKind::Server);
        assert_eq!(mem.week_used(), Some(100), "a refusal without it leaves it");
        assert_eq!(mem.stats("u").await.unwrap_err().kind(), ErrorKind::PaymentRequired);
        assert_eq!(mem.week_used(), Some(100), "and so does one whose value is not a percent");
        assert_eq!(mem.with_model("scroll-2").week_used(), None, "a copy starts with none");
        mem.stats("u").await.unwrap();
        assert_eq!(mem.week_used(), None, "a success without it says no plan week");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_memory_write_is_retried_on_502_under_one_key() {
        let (base, seen) = mock_server_recording(vec![
            http("502 Bad Gateway", "Retry-After: 0\r\n", "{\"error\":\"bad gateway\"}"),
            http("200 OK", "Idempotent-Replayed: true\r\n", "{\"id\":\"m1\",\"status\":\"stored\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let r = mem.add("hi", "u", serde_json::json!({})).await.unwrap();
        assert_eq!(r["id"], "m1");
        assert!(r.get("replayed").is_none(), "the client's own retry is not the caller's replay");
        let reqs = seen.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        let key = |r: &str| {
            r.lines()
                .find_map(|l| l.to_lowercase().strip_prefix("idempotency-key:").map(|v| v.trim().to_string()))
                .expect("a key on every attempt")
        };
        let k = key(&reqs[0]);
        assert!(k.starts_with("sdk-") && k.len() == 36 && k[4..].bytes().all(|b| b.is_ascii_hexdigit()), "{k}");
        assert_eq!(key(&reqs[1]), k, "a retry must repeat the key, or it is a second write");
    }

    #[test]
    fn made_keys_differ_and_have_the_right_shape() {
        let a = new_idempotency_key();
        let b = new_idempotency_key();
        assert_ne!(a, b);
        for k in [&a, &b] {
            assert!(valid_idempotency_key(k));
            assert!(k.starts_with("sdk-") && k.len() == 36, "{k}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_retried_on_503() {
        let (base, hits) = mock_server(vec![
            http("503 Service Unavailable", "Retry-After: 0\r\n", "{}"),
            http("200 OK", "", "{\"models\":[]}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.list_models().await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_image_is_retried_on_503_like_the_other_reads() {
        let (base, hits) = mock_server(vec![
            http("503 Service Unavailable", "Retry-After: 0\r\n", "{}"),
            http("200 OK", "", "PNGBYTES"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let (bytes, _) = mem.get_image("u", "11111111-2222-3333-4444-555555555555").await.unwrap();
        assert_eq!(bytes, b"PNGBYTES");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_read_sent_as_post_is_retried_on_503_and_a_write_is_not() {
        let (base, hits) = mock_server(vec![
            http("503 Service Unavailable", "Retry-After: 0\r\n", "{}"),
            http("200 OK", "", "{\"memories\":[]}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.search("q", "u", 5).await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 2);

        let (base, hits) = mock_server(vec![
            http("503 Service Unavailable", "Retry-After: 0\r\n", "{}"),
            http("200 OK", "", "{\"status\":\"created\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert!(mem.create_store("u").await.is_err(), "a write without a key is not sent again");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bad_json_on_200_is_an_error_not_empty() {
        let (base, _) = mock_server(vec![http("200 OK", "", "not valid json{")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.search("q", "u", 5).await {
            Err(WosError::Api { message, .. }) => assert!(message.contains("invalid JSON"), "got {message}"),
            other => panic!("expected invalid-JSON error, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn whitespace_in_key_errors_before_sending() {
        let mem = Client::with_base_url("wos-test xxxxxxxxxx", "http://127.0.0.1:9");
        match mem.stats("alice").await {
            Err(WosError::Api { status, message, .. }) => {
                assert_eq!(status, 400);
                assert!(message.contains("whitespace"), "got {message}");
            }
            other => panic!("expected whitespace-key error, got {other:?}"),
        }
    }

    #[test]
    fn store_writes_wait_longer_unless_a_timeout_was_set() {
        let mem = Client::new("wos-test-xxxxxxxxxx");
        let secs = |c: &Client, p: &str| c.attempt_timeout(p).as_secs();
        for p in ["/api/v1/memory/store", "/api/v1/memory/store-turn", "/api/v1/memory/bulk-store"] {
            assert_eq!(secs(&mem, p), 200, "{p}");
        }
        for p in ["/api/v1/memory/supersede", "/api/v1/memory/search", "/api/v1/memory/recall", "/api/v1/memory/image"] {
            assert_eq!(secs(&mem, p), 30, "{p}");
        }
        let set = mem.with_timeout(10);
        assert_eq!((secs(&set, "/api/v1/memory/store"), secs(&set, "/api/v1/memory/search")), (10, 10));
        assert_eq!(secs(&set.with_model("tablet-1"), "/api/v1/memory/bulk-store"), 10);
        assert_eq!(secs(&set.with_timeout(0), "/api/v1/memory/store"), 200);
        assert_eq!(secs(&mem.with_user("bob"), "/api/v1/memory/store-turn"), 200);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn every_form_of_add_add_turn_and_add_bulk_gets_the_store_timeout() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}"); 11]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let meta = serde_json::json!({});
        mem.add("x", "alice", meta.clone()).await.unwrap();
        mem.store("x", "alice", meta.clone()).await.unwrap();
        mem.add_with("x", "alice", meta.clone(), meta.clone()).await.unwrap();
        mem.add_with_idempotent("x", "alice", meta.clone(), meta.clone(), "k1").await.unwrap();
        mem.add_idempotent("x", "alice", meta.clone(), "k2").await.unwrap();
        mem.add_turn("hi", "yo", "alice").await.unwrap();
        mem.add_turn_idempotent("hi", "yo", "alice", "k3").await.unwrap();
        mem.add_bulk("blob", "alice", "general").await.unwrap();
        mem.add_bulk_with("blob", "alice", "general", meta.clone()).await.unwrap();
        mem.add_bulk_with_idempotent("blob", "alice", "general", meta.clone(), "k4").await.unwrap();
        mem.add_bulk_idempotent("blob", "alice", "general", "k5").await.unwrap();
        let reqs = seen.lock().unwrap();
        assert_eq!(reqs.len(), 11);
        for r in reqs.iter() {
            let path = r.split_whitespace().nth(1).unwrap_or_default();
            assert!(STORE_PATHS.contains(&path), "{path}");
            assert_eq!(mem.attempt_timeout(path).as_secs(), STORE_TIMEOUT_SECS, "{path}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn with_timeout_zero_falls_back_to_default() {
        let (base, _) = mock_server(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_timeout(0);
        mem.stats("alice").await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_unwraps_memory_and_guards_empty_id() {
        let (base, _) = mock_server(vec![http(
            "200 OK",
            "",
            "{\"user_id\":\"u\",\"memory\":{\"id\":\"9b2d\",\"content\":\"tea\",\"is_superseded\":false}}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let m = mem.get("u", "9b2d").await.unwrap();
        assert_eq!(m["content"], "tea");
        let off = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:9");
        match off.get("u", " ").await {
            Err(WosError::Api { status: 400, message, .. }) => assert!(message.contains("memory_id")),
            other => panic!("expected guard error, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_takes_a_flat_row() {
        let (base, _) = mock_server(vec![
            http("200 OK", "", "{\"id\":\"9b2d\",\"content\":\"tea\"}"),
            http("200 OK", "", "{\"user_id\":\"u\",\"memory\":null}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let m = mem.get("u", "9b2d").await.unwrap();
        assert_eq!(m["content"], "tea");
        assert_eq!(mem.get("u", "9b2d").await.unwrap(), serde_json::json!({}));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_returns_image_rows_after_the_text_each_id_once() {
        let payload = "{\"memories\":[{\"id\":\"m1\"},{\"id\":\"dup\"}],\"self_memories\":[{\"id\":\"s1\"}],\
                       \"images\":[{\"id\":\"dup\"},{\"id\":\"i1\",\"content\":\"\"}]}";
        let (base, _) = mock_server(vec![http("200 OK", "", payload), http("200 OK", "", payload)]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let ids: Vec<_> = mem.search("q", "alice", 10).await.unwrap().into_iter().filter_map(|m| m.id).collect();
        assert_eq!(ids, ["m1", "dup", "s1", "i1"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_all_memories_refuses_a_repeated_cursor() {
        let (base, hits) = mock_server(vec![
            http("200 OK", "", "{\"memories\":[{\"id\":\"1\"}],\"next_cursor\":\"C\"}"),
            http("200 OK", "", "{\"memories\":[{\"id\":\"2\"}],\"next_cursor\":\"C\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.list_all_memories("u").await {
            Err(WosError::Api { status: 200, message, .. }) => {
                assert!(message.contains("truncated"), "got {message}");
                assert!(!message.contains('\u{2014}'), "got {message}");
            }
            other => panic!("a repeated cursor must not read as a complete store, got {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_all_memories_refuses_a_cursor_cycle() {
        let (base, hits) = mock_server(vec![
            http("200 OK", "", "{\"memories\":[{\"id\":\"1\"}],\"next_cursor\":\"A\"}"),
            http("200 OK", "", "{\"memories\":[{\"id\":\"2\"}],\"next_cursor\":\"B\"}"),
            http("200 OK", "", "{\"memories\":[{\"id\":\"3\"}],\"next_cursor\":\"A\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem.list_all_memories("u").await.unwrap_err();
        assert!(format!("{e}").contains("truncated"), "got {e}");
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_all_memories_ends_on_an_empty_page_that_repeats_its_cursor() {
        let (base, hits) = mock_server(vec![
            http("200 OK", "", "{\"memories\":[{\"id\":\"1\"}],\"next_cursor\":\"C\"}"),
            http("200 OK", "", "{\"memories\":[],\"next_cursor\":\"C\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let all = mem.list_all_memories("u").await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn all_images_keeps_walking_when_images_share_a_timestamp() {
        let ts = "2026-08-19T00:00:00Z";
        let (base, hits) = mock_server(vec![
            http("200 OK", "", &format!("{{\"images\":[{{\"id\":\"a\"}}],\"has_more\":true,\"next_before\":\"{ts}\",\"next_skip_ids\":[\"a\"]}}")),
            http("200 OK", "", &format!("{{\"images\":[{{\"id\":\"b\"}}],\"has_more\":true,\"next_before\":\"{ts}\",\"next_skip_ids\":[\"a\",\"b\"]}}")),
            http("200 OK", "", &format!("{{\"images\":[{{\"id\":\"c\"}}],\"has_more\":true,\"next_before\":\"{ts}\",\"next_skip_ids\":[\"a\",\"b\",\"c\"]}}")),
            http("200 OK", "", "{\"images\":[{\"id\":\"d\"}],\"has_more\":false}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let all = mem.list_all_images("u", None).await.unwrap();
        let ids: Vec<&str> = all.iter().filter_map(|m| m["id"].as_str()).collect();
        assert_eq!(ids, ["a", "b", "c", "d"], "stopped partway through one timestamp's images");
        assert_eq!(hits.load(Ordering::SeqCst), 4);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn all_images_refuses_a_repeated_cursor_pair() {
        let page = "{\"images\":[{\"id\":\"x\"}],\"has_more\":true,\"next_before\":\"T\",\"next_skip_ids\":[\"x\"]}";
        let (base, hits) = mock_server(vec![http("200 OK", "", page), http("200 OK", "", page)]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.list_all_images("u", None).await {
            Err(WosError::Api { status: 200, message, .. }) => {
                assert!(message.contains("truncated"), "got {message}");
                assert!(!message.contains('\u{2014}'), "got {message}");
            }
            other => panic!("a repeated cursor pair must not read as every image, got {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn no_content_is_a_success_not_a_parse_error() {
        let (base, _) = mock_server(vec![http("204 No Content", "", "")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert_eq!(mem.delete("alice", "m1").await.unwrap(), serde_json::json!({}));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn idempotency_key_rides_on_the_write() {
        let (base, seen) = mock_server_recording(vec![
            http("200 OK", "", "{\"id\":\"m1\",\"status\":\"stored\"}"),
            http("200 OK", "", "{}"),
            http("200 OK", "", "{}"),
            http("200 OK", "", "{}"),
            http("200 OK", "", "{}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = serde_json::json!({});
        mem.add_idempotent("x", "alice", e.clone(), "import:row-42").await.unwrap();
        mem.add_turn_idempotent("hi", "yo", "alice", "k2").await.unwrap();
        mem.add_bulk_idempotent("blob", "alice", "general", "k3").await.unwrap();
        mem.update_idempotent("m1", "new", "alice", "k4").await.unwrap();
        mem.add("x", "alice", e).await.unwrap();
        let reqs = seen.lock().unwrap();
        let keys: Vec<bool> = reqs.iter().map(|r| r.to_lowercase().contains("idempotency-key:")).collect();
        assert_eq!(keys, vec![true, true, true, true, true], "reqs: {reqs:?}");
        assert!(reqs[0].contains("import:row-42"));
        assert!(reqs[4].contains("sdk-"), "reqs: {reqs:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn malformed_idempotency_key_fails_before_the_request() {
        let off = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:9");
        for bad in ["caf\u{e9} key", &"a".repeat(129), "", "has space"] {
            match off.add_idempotent("x", "alice", serde_json::json!({}), bad).await {
                Err(WosError::Api { status: 400, message, .. }) => {
                    assert!(message.contains("invalid idempotency_key"), "got {message}")
                }
                other => panic!("expected a local reject for {bad:?}, got {other:?}"),
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn revisions_goes_to_the_won_surface() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"revised\":3,\"total\":40}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.revisions("alice").await.unwrap();
        let reqs = seen.lock().unwrap();
        assert!(reqs[0].contains("POST /api/v1/won/revisions"), "got {}", reqs[0]);
        assert!(!reqs[0].contains("/api/v1/memory/revisions"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_full_keeps_what_search_merges_away() {
        let payload = "{\"memories\":[{\"id\":\"m1\"}],\"self_memories\":[{\"id\":\"s1\"}],\
                       \"images\":[{\"id\":\"i1\"}],\"verify_used\":2}";
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", payload)]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let r = mem
            .search_full("q", "alice", 10, &SearchOpts { verify: Some(2), max_images: Some(3) })
            .await
            .unwrap();
        assert_eq!(r.memories.len(), 1);
        assert_eq!(r.self_memories.len(), 1);
        assert_eq!(r.images.len(), 1);
        assert_eq!(r.verify_used, Some(2));
        let reqs = seen.lock().unwrap();
        assert!(reqs[0].contains("\"max_images\":3"), "got {}", reqs[0]);
        assert!(reqs[0].contains("\"verify\":2"), "got {}", reqs[0]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_full_normalizes_a_nulled_field() {
        let (base, _seen) = mock_server_recording(vec![http(
            "200 OK",
            "",
            "{\"memories\":null,\"images\":null}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let r = mem.search_full("q", "alice", 10, &SearchOpts::default()).await.unwrap();
        assert!(r.memories.is_empty());
        assert!(r.images.is_empty());
        assert!(r.self_memories.is_empty());
        assert_eq!(r.verify_used, None, "absent means it was never asked for");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_opts_names_verify_and_max_images() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"memories\":[]}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.search_opts("q", "alice", 10, &SearchOpts { verify: Some(2), max_images: Some(5) })
            .await
            .unwrap();
        let reqs = seen.lock().unwrap();
        assert!(reqs[0].contains("\"verify\":2"), "got {}", reqs[0]);
        assert!(reqs[0].contains("\"max_images\":5"), "got {}", reqs[0]);
        assert!(reqs[0].contains("\"user_id\":\"alice\""), "got {}", reqs[0]);
        assert!(reqs[0].contains("\"max_results\":10"), "got {}", reqs[0]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_opts_default_sends_neither_field() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"memories\":[]}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.search_opts("q", "alice", 10, &SearchOpts::default()).await.unwrap();
        let reqs = seen.lock().unwrap();
        assert!(!reqs[0].contains("verify"), "got {}", reqs[0]);
        assert!(!reqs[0].contains("max_images"), "got {}", reqs[0]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn revisions_asks_for_counts_only() {
        let (base, seen) = mock_server_recording(vec![http(
            "200 OK",
            "",
            "{\"revised\":3,\"unrevised\":37,\"total\":40}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.revisions("alice").await.unwrap();
        let reqs = seen.lock().unwrap();
        assert!(!reqs[0].contains("include"), "a counts call carried include: {}", reqs[0]);
        assert!(!reqs[0].contains("limit"), "a counts call carried limit: {}", reqs[0]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn revisions_page_sends_the_cursor_under_its_wire_name() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"memories\":[]}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let skip = vec!["11111111-1111-1111-1111-111111111111".to_string()];
        mem.revisions_page("alice", "unrevised", 20usize, "2026-08-20T01:00:00Z", Some(&skip))
            .await
            .unwrap();
        let reqs = seen.lock().unwrap();
        assert!(reqs[0].contains("POST /api/v1/won/revisions"), "got {}", reqs[0]);
        assert!(reqs[0].contains("\"include\":\"unrevised\""), "got {}", reqs[0]);
        assert!(reqs[0].contains("\"limit\":20"), "got {}", reqs[0]);
        assert!(reqs[0].contains("\"before\":\"2026-08-20T01:00:00Z\""), "got {}", reqs[0]);
        assert!(reqs[0].contains("\"skip_ids\""), "cursor did not go out as snake_case: {}", reqs[0]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_filters_reach_the_api() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"memories\":[]}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.search_with("q", "alice", 10, serde_json::json!({
            "filters": {"categories": ["work"], "event_from": "2026-01-01"}
        })).await.unwrap();
        let reqs = seen.lock().unwrap();
        assert!(reqs[0].contains("\"categories\":[\"work\"]"), "got {}", reqs[0]);
        assert!(reqs[0].contains("\"event_from\":\"2026-01-01\""));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_with_carries_an_image_and_strips_what_breaks_it() {
        let flat = "AAAABBBBCCCCDDDD".repeat(12);
        let wrapped: String = flat
            .as_bytes()
            .chunks(76)
            .map(|c| String::from_utf8_lossy(c).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(wrapped.contains('\n'), "the fixture must actually be wrapped");

        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"id\":\"m1\"}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.add_with(
            "a striped test card",
            "alice",
            serde_json::json!({}),
            serde_json::json!({"image": {
                "data": format!("data:image/png;base64,{wrapped}"),
                "taken_at": "2026-08-15T09:00:00Z",
            }}),
        )
        .await
        .unwrap();

        let reqs = seen.lock().unwrap();
        assert!(reqs[0].contains(&format!("\"data\":\"{flat}\"")), "got {}", reqs[0]);
        assert!(!reqs[0].contains("data:image/png"), "the data: prefix must not be sent");
        assert!(!reqs[0].contains("\\n"), "no line break may survive into the payload");
        assert!(reqs[0].contains("\"taken_at\":\"2026-08-15T09:00:00Z\""), "siblings must pass through");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_with_lets_reserved_fields_win() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"id\":\"m1\"}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.add_with(
            "the real one",
            "alice",
            serde_json::json!({"speaker": "me"}),
            serde_json::json!({"content": "smuggled", "user_id": "bob", "categories": ["work"]}),
        )
        .await
        .unwrap();

        let reqs = seen.lock().unwrap();
        assert!(reqs[0].contains("\"content\":\"the real one\""), "got {}", reqs[0]);
        assert!(!reqs[0].contains("smuggled"));
        assert!(reqs[0].contains("\"user_id\":\"alice\""));
        assert!(!reqs[0].contains("bob"));
        assert!(reqs[0].contains("\"speaker\":\"me\""));
        assert!(reqs[0].contains("\"categories\":[\"work\"]"), "extra fields still reach the API");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_with_refuses_an_image_that_is_only_a_prefix() {
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:1");
        match mem
            .add_with("a caption", "alice", serde_json::json!({}), serde_json::json!({"image": {"data": "data:image/png;base64,"}}))
            .await
        {
            Err(WosError::Api { status: 400, message, .. }) => {
                assert!(message.contains("empty"), "got {message}")
            }
            other => panic!("expected a 400 about an empty image, got {other:?}"),
        }
        match mem
            .add_with("a caption", "alice", serde_json::json!({}), serde_json::json!({"image": {"reference": "s3://x"}}))
            .await
        {
            Err(WosError::Api { status: 400, message, .. }) => {
                assert!(message.contains("image.data is required"), "got {message}")
            }
            other => panic!("expected a 400 about a missing data field, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_deadline_bounds_the_whole_call_not_one_attempt() {
        let (base, hits) = mock_server(vec![
            http(
                "429 Too Many Requests",
                "Retry-After: 5\r\n",
                "{\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"rate limited\",\"request_id\":\"req_rl\"}}",
            ),
            http("200 OK", "", "{\"memories\":[]}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(200));
        let t0 = std::time::Instant::now();
        let e = mem.search("q", "alice", 10).await.unwrap_err();
        assert_eq!(e.status(), Some(429), "got {e}");
        assert!(e.is_rate_limited(), "got {e}");
        assert!(format!("{e}").contains("req_rl"), "the request id must survive: {e}");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert!(t0.elapsed().as_secs_f64() < 0.15, "spent {:?} on a 200ms budget", t0.elapsed());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_clone_keeps_the_deadline() {
        let (base, _) = mock_server(vec![http(
            "429 Too Many Requests", "Retry-After: 5\r\n", "{}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(200))
            .with_model("tablet-2");
        let t0 = std::time::Instant::now();
        assert!(mem.search("q", "alice", 10).await.is_err());
        assert!(t0.elapsed().as_secs_f64() < 1.0, "the clone slept for {:?}", t0.elapsed());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn recall_refuses_a_count_out_of_range_without_sending() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for bad in [0usize, 1, 4, 21, 500] {
            let opts = RecallOpts { limit: Some(bad), context_limit: None, ..Default::default() };
            match mem.recall_opts("q", "alice", &opts).await {
                Err(WosError::Api { status: 400, message, .. }) => {
                    assert!(message.contains("between 5 and 20"), "got {message}")
                }
                other => panic!("expected a refusal for limit={bad}, got {other:?}"),
            }
        }
        assert_eq!(seen.lock().unwrap().len(), 0, "nothing may reach the wire");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn recall_refuses_a_context_limit_out_of_range_but_allows_zero() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for bad in [21usize, 100] {
            let opts = RecallOpts { limit: None, context_limit: Some(bad), ..Default::default() };
            assert!(mem.recall_opts("q", "alice", &opts).await.is_err(), "context_limit={bad}");
        }
        let opts = RecallOpts { limit: Some(20), context_limit: Some(0), ..Default::default() };
        mem.recall_opts("q", "alice", &opts).await.expect("0 and 20 are in range");
        let body = &seen.lock().unwrap()[0];
        assert!(body.contains("\"context_limit\":0"), "got {body}");
        assert!(body.contains("\"limit\":20"), "got {body}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn replayed_never_overwrites_what_the_service_sent() {
        let (base, _) = mock_server(vec![http(
            "200 OK",
            "Idempotent-Replayed: true\r\n",
            "{\"id\":\"m1\",\"replayed\":false}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let v = mem
            .add_with("x", "alice", serde_json::json!({}), serde_json::json!({}))
            .await
            .expect("stored");
        assert_eq!(v["replayed"], serde_json::json!(false), "the service said false");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_200_is_an_error_not_a_silent_ok() {
        let (base, _) = mock_server(vec![http("200 OK", "", "")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.add("x", "alice", serde_json::json!({})).await {
            Err(WosError::Api { status: 200, message, .. }) => {
                assert!(message.contains("empty response body"), "got {message}")
            }
            other => panic!("expected an empty-body error, got {other:?}"),
        }
    }
    #[tokio::test(flavor = "current_thread")]
    async fn add_refuses_a_store_name_as_metadata_before_the_network() {
        let off = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:9");
        for md in [
            serde_json::json!({"userId": "bob"}),
            serde_json::json!({"store_id": "t"}),
            serde_json::json!({"Idempotency-Key": "k"}),
        ] {
            match off.add("x", "alice", md.clone()).await {
                Err(WosError::Api { status: 400, message, .. }) => assert!(message.contains("not a metadata field")),
                other => panic!("{md}: expected a refusal, got {other:?}"),
            }
            match off.add_with("x", "alice", md, serde_json::json!({})).await {
                Err(WosError::Api { status: 400, .. }) => {}
                other => panic!("add_with: expected a refusal, got {other:?}"),
            }
        }
        assert!(check_metadata_keys(&serde_json::json!({"store": "Costco", "user_id_2": "b", "model": "m"})).is_ok());
    }

    #[test]
    fn the_accepted_store_id_format() {
        for ok in ["a", "Alice.Smith", "team-a_1", &"x".repeat(64)] {
            assert!(valid_store_id_format(ok), "{ok:?}");
        }
        for bad in ["", "-a", ".a", "bob.lee@example.com", "na\u{ef}ve", "a b", &"x".repeat(65)] {
            assert!(!valid_store_id_format(bad), "{bad:?}");
        }
    }

    #[test]
    fn store_id_normalization_is_what_the_api_does() {
        assert_eq!(normalize_store_id("Alice.Smith"), "alice_smith");
        assert_eq!(normalize_store_id("alice-smith"), "alice_smith");
        assert_eq!(normalize_store_id("bob.lee.x"), "bob_lee_x");
        assert_eq!(normalize_store_id("bob-lee-x"), "bob_lee_x");
        assert_eq!(normalize_store_id("alice_smith"), "alice_smith");
        assert_eq!(normalize_store_id("na\u{ef}ve"), "na_ve");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn idempotent_replay_is_surfaced() {
        let (base, _) = mock_server(vec![
            http("200 OK", "Idempotent-Replayed: true\r\n", "{\"id\":\"m1\",\"status\":\"stored\"}"),
            http("200 OK", "", "{\"id\":\"m2\",\"status\":\"stored\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let a = mem
            .add_idempotent("x", "alice", serde_json::json!({}), "k1")
            .await
            .unwrap();
        assert_eq!(a["replayed"], serde_json::json!(true));
        let b = mem.add("y", "alice", serde_json::json!({})).await.unwrap();
        assert!(b.get("replayed").is_none(), "a fresh write must carry no replay marker");
    }
