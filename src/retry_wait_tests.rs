    use super::tests::{http, mock_server};
    use super::{Client, ErrorKind};
    use std::sync::atomic::Ordering;

    #[tokio::test(flavor = "current_thread")]
    async fn a_read_whose_backoff_cannot_fit_reports_the_503() {
        let (base, hits) = mock_server(vec![
            http(
                "503 Service Unavailable",
                "Retry-After: 5\r\n",
                "{\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"busy\",\"request_id\":\"req_503\"}}",
            ),
            http("200 OK", "", "{\"collections\":[]}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(200));
        let t0 = std::time::Instant::now();
        let e = mem.list_stores().await.unwrap_err();
        assert_eq!(e.status(), Some(503), "got {e}");
        assert_eq!(e.kind(), ErrorKind::Server);
        assert!(format!("{e}").contains("req_503"), "got {e}");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert!(t0.elapsed().as_secs_f64() < 0.15, "spent {:?}", t0.elapsed());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_retry_after_beyond_the_cap_is_not_slept_through() {
        let an_hour = httpdate::fmt_http_date(std::time::SystemTime::now() + std::time::Duration::from_secs(3600));
        for ra in ["3600".to_string(), an_hour] {
            let (base, hits) = mock_server(vec![
                http(
                    "429 Too Many Requests",
                    &format!("Retry-After: {ra}\r\n"),
                    "{\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"free endpoint limit\",\"request_id\":\"req_cap\"}}",
                ),
                http("200 OK", "", "{\"days\":7}"),
            ]);
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
            let t0 = std::time::Instant::now();
            let e = mem.usage(7).await.unwrap_err();
            assert!(e.is_rate_limited(), "Retry-After {ra}: got {e}");
            assert!(format!("{e}").contains("req_cap"), "got {e}");
            assert_eq!(hits.load(Ordering::SeqCst), 1, "Retry-After {ra} was retried");
            assert!(t0.elapsed().as_secs_f64() < 1.0, "slept {:?} for Retry-After {ra}", t0.elapsed());
        }
    }

    const LIMITED: &str = "{\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\
        \"message\":\"slow down\",\"request_id\":\"req_429\"}}";

    fn limited_then_stalls(hold: std::time::Duration) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h = hits.clone();
        std::thread::spawn(move || {
            for i in 0..2 {
                let Ok((mut sock, _)) = listener.accept() else { return };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 2048];
                while let Ok(n) = sock.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                h.fetch_add(1, Ordering::SeqCst);
                if i == 0 {
                    let _ = sock.write_all(http("429 Too Many Requests", "Retry-After: 1\r\n", LIMITED).as_bytes());
                } else {
                    std::thread::sleep(hold);
                    let _ = sock.write_all(http("200 OK", "", "{\"collections\":[]}").as_bytes());
                }
            }
        });
        (format!("http://{addr}"), hits)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_read_whose_last_attempt_runs_out_of_budget_reports_the_429() {
        let (base, hits) = limited_then_stalls(std::time::Duration::from_secs(2));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(1200));
        let t0 = std::time::Instant::now();
        let e = mem.list_stores().await.unwrap_err();
        assert_eq!(e.status(), Some(429), "got {e:?}");
        assert!(e.is_rate_limited());
        assert!(format!("{e}").contains("req_429"), "got {e}");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert!(t0.elapsed().as_secs_f64() < 1.6, "spent {:?}", t0.elapsed());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_retry_that_drops_with_no_time_to_back_off_reports_the_answer_before_it() {
        let (base, hits) = mock_server(vec![
            http("429 Too Many Requests", "Retry-After: 0\r\n", r#"{"error":{"type":"rate_limit_error","message":"slow down"}}"#),
            String::new(),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(600));
        let e = mem.list_stores().await.unwrap_err();
        assert_eq!(e.status(), Some(429), "got {e:?}");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_search_whose_last_attempt_runs_out_of_budget_reports_the_429() {
        let (base, hits) = limited_then_stalls(std::time::Duration::from_secs(2));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(1200));
        let e = mem.search("q", "alice", 10).await.unwrap_err();
        assert_eq!(e.status(), Some(429), "got {e:?}");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_image_whose_last_attempt_runs_out_of_budget_reports_the_429() {
        let (base, hits) = limited_then_stalls(std::time::Duration::from_secs(2));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(1200));
        let e = mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await.unwrap_err();
        assert_eq!(e.status(), Some(429), "got {e:?}");
        assert!(format!("{e}").contains("req_429"), "got {e}");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_write_that_times_out_at_the_deadline_reports_the_deadline() {
        let (base, hits) = limited_then_stalls(std::time::Duration::from_secs(2));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(1200));
        let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Connection, "got {e:?}");
        assert_eq!(e.status(), Some(0), "got {e:?}");
        assert!(format!("{e}").contains("deadline of 1.2s exhausted"), "got {e}");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    fn stalls(hold: std::time::Duration, head_first: bool, body: &'static [u8]) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let Ok((mut sock, _)) = listener.accept() else { return };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 2048];
            while let Ok(n) = sock.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            if head_first {
                let _ = sock.write_all(head.as_bytes());
                let _ = sock.flush();
            }
            std::thread::sleep(hold);
            if !head_first {
                let _ = sock.write_all(head.as_bytes());
            }
            let _ = sock.write_all(body);
        });
        format!("http://{addr}")
    }

    fn assert_deadline(e: &super::WosError, what: &str) {
        assert_eq!(e.status(), Some(0), "{what}: got {e:?}");
        assert_eq!(e.kind(), ErrorKind::Connection, "{what}: got {e:?}");
        assert!(format!("{e}").contains("deadline of 300ms exhausted"), "{what}: got {e}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_first_attempt_cut_short_by_the_deadline_reports_the_deadline() {
        const OK: &[u8] = b"{\"collections\":[],\"id\":\"m1\"}";
        let hold = std::time::Duration::from_secs(2);
        let deadline = std::time::Duration::from_millis(300);
        for head_first in [false, true] {
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &stalls(hold, head_first, OK)).with_deadline(deadline);
            let e = mem.list_stores().await.unwrap_err();
            assert_deadline(&e, &format!("read, head_first={head_first}"));

            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &stalls(hold, head_first, OK)).with_deadline(deadline);
            let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
            assert_deadline(&e, &format!("write, head_first={head_first}"));

            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &stalls(hold, head_first, b"PNG!")).with_deadline(deadline);
            let e = mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await.unwrap_err();
            assert_deadline(&e, &format!("get_image, head_first={head_first}"));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_attempt_timeout_without_a_deadline_still_says_timed_out() {
        let mem = Client::with_base_url(
            "wos-test-xxxxxxxxxx",
            &stalls(std::time::Duration::from_secs(3), false, b"{}"),
        )
        .with_timeout(1)
        .with_retries(0);
        let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(e.status(), None, "got {e:?}");
        assert!(format!("{e}").contains("timed out"), "got {e}");
    }

    #[test]
    fn backoff_jitter_spreads_retries() {
        let jitters: std::collections::HashSet<u128> =
            (0..64).map(|_| super::backoff(0).as_millis() - 500).collect();
        assert!(jitters.iter().all(|j| *j < 250), "jitter out of range: {jitters:?}");
        assert!(jitters.len() > 8, "jitter barely varies: {jitters:?}");
        for attempt in 1..8 {
            let d = super::backoff(attempt).as_millis();
            let base = (500u128 << attempt.min(4)).min(8_000);
            assert!((base..base + 250).contains(&d), "attempt {attempt}: {d}ms");
        }
    }
