    use super::tests::{http, mock_server};
    use super::{Client, WosError};
    use std::io::{Read, Write};
    use std::sync::atomic::Ordering;

    fn cut(status: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{{\"e"
        )
    }

    const STORES: &str = "{\"collections\":[],\"count\":0}";

    #[tokio::test(flavor = "current_thread")]
    async fn a_read_whose_error_body_drops_reports_its_status() {
        for (status, code) in [("500 Internal Server Error", 500), ("404 Not Found", 404), ("409 Conflict", 409), ("400 Bad Request", 400)] {
            let (base, hits) = mock_server(vec![cut(status), http("200 OK", "", STORES)]);
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
            match mem.list_stores().await {
                Err(WosError::Api { status: s, message, .. }) if s == code => {
                    assert!(message.contains("could not be read"), "{status}: got {message}")
                }
                other => panic!("{status}: got {other:?}"),
            }
            assert_eq!(hits.load(Ordering::SeqCst), 1, "{status}");
        }
        let (base, hits) = mock_server(vec![cut("500 Internal Server Error"), http("200 OK", "", "{\"deleted\":true}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert_eq!(mem.delete_store("tenant_a").await.unwrap_err().status(), Some(500));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_write_whose_error_body_drops_is_not_sent_again() {
        let (base, hits) = mock_server(vec![cut("500 Internal Server Error"), http("200 OK", "", "{\"id\":\"m1\"}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.add("hello", "alice", serde_json::json!({})).await {
            Err(WosError::Api { status: 500, message, .. }) => {
                assert!(message.contains("could not be read"), "got {message}")
            }
            other => panic!("expected the 500, got {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1, "a write is attempted exactly once");
    }

    fn stalling_server(first: String, then: String) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h = hits.clone();
        std::thread::spawn(move || {
            while let Ok((mut sock, _)) = listener.accept() {
                let n = h.fetch_add(1, Ordering::SeqCst);
                let mut chunk = [0u8; 8192];
                let _ = sock.read(&mut chunk);
                if n > 0 {
                    let _ = sock.write_all(then.as_bytes());
                    continue;
                }
                let _ = sock.write_all(first.as_bytes());
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(6));
                    drop(sock);
                });
            }
        });
        (base, hits)
    }

    fn cut_with(status: &str, headers: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n{headers}\r\n{{\"e"
        )
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_error_body_that_stalls_is_not_sent_again() {
        let (base, hits) = stalling_server(cut("500 Internal Server Error"), http("200 OK", "", STORES));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_timeout(1);
        match mem.list_stores().await {
            Err(WosError::Api { status: 500, .. }) => {}
            other => panic!("expected the 500, got {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1, "a timeout is not a drop");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_429_whose_body_stalls_is_sent_again_without_waiting_for_the_body() {
        let (base, hits) = stalling_server(
            cut_with("429 Too Many Requests", "Retry-After: 0\r\n"),
            http("200 OK", "", "{\"id\":\"m1\"}"),
        );
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_timeout(4);
        let t0 = std::time::Instant::now();
        let got = mem.add("hello", "alice", serde_json::json!({})).await;
        assert!(got.is_ok(), "got {got:?}");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert!(t0.elapsed() < std::time::Duration::from_secs(3), "waited {:?}", t0.elapsed());
    }

    fn late_body_server(head: &str, body: &str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (head, body) = (head.to_string(), body.to_string());
        std::thread::spawn(move || {
            while let Ok((mut sock, _)) = listener.accept() {
                let (head, body) = (head.clone(), body.clone());
                std::thread::spawn(move || {
                    let mut chunk = [0u8; 8192];
                    let _ = sock.read(&mut chunk);
                    let _ = sock.write_all(
                        format!("{head}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes(),
                    );
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    let _ = sock.write_all(body.as_bytes());
                });
            }
        });
        base
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_429_that_is_not_retried_waits_for_its_body() {
        let body = r#"{"error":{"type":"rate_limit_error","message":"slow down"}}"#;
        for (retry_after, retries) in [("60", 2), ("0", 0)] {
            let head = format!("HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nRetry-After: {retry_after}\r\n");
            let base = late_body_server(&head, body);
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_timeout(4).with_retries(retries);
            let e = mem.stats("alice").await.unwrap_err();
            assert_eq!(e.status(), Some(429), "{e:?}");
            assert!(e.to_string().contains("slow down"), "Retry-After {retry_after}: {e}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_image_does_not_wait_for_a_429_body_that_stalls() {
        let (base, hits) = stalling_server(
            cut_with("429 Too Many Requests", "Retry-After: 0\r\n"),
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPNG!".to_string(),
        );
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_timeout(4);
        let t0 = std::time::Instant::now();
        let (bytes, _) = mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await.unwrap();
        assert_eq!(bytes, b"PNG!");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert!(t0.elapsed() < std::time::Duration::from_secs(3), "waited {:?}", t0.elapsed());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_read_503_whose_body_stalls_is_sent_again_inside_the_deadline() {
        let (base, hits) = stalling_server(
            cut_with("503 Service Unavailable", "Retry-After: 0\r\n"),
            http("200 OK", "", STORES),
        );
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_timeout(4)
            .with_deadline(std::time::Duration::from_secs(3));
        let got = mem.list_stores().await;
        assert!(got.is_ok(), "got {got:?}");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_retryable_answer_whose_body_does_not_arrive_says_so() {
        let (base, hits) = stalling_server(
            cut_with("503 Service Unavailable", "Retry-After: 2\r\n"),
            http("200 OK", "", STORES),
        );
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_timeout(4)
            .with_deadline(std::time::Duration::from_millis(2500));
        match mem.list_stores().await {
            Err(WosError::Api { status: 503, message, .. }) => {
                assert!(message.contains("could not be read: not received within 1s"), "got {message}")
            }
            other => panic!("expected the 503, got {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_429_whose_body_stalls_on_the_last_attempt_reports_the_429() {
        let (base, hits) = stalling_server(
            cut_with("429 Too Many Requests", "Retry-After: 0\r\n"),
            http("200 OK", "", STORES),
        );
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_timeout(2).with_retries(0);
        match mem.list_stores().await {
            Err(WosError::Api { status: 429, .. }) => {}
            other => panic!("expected the 429, got {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
