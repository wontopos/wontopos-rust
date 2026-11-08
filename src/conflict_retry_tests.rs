    use super::tests::{http, mock_server};
    use super::{Client, ErrorKind};
    use std::sync::atomic::Ordering;

    const WRITE_LOCK: &str = "{\"type\":\"error\",\"error\":{\"type\":\"conflict_error\",\
        \"message\":\"Another write to store 'alice' is already in flight. Nothing was stored or changed.\",\
        \"request_id\":\"req_lock\",\"retry_after_ms\":100}}";
    const COLLISION: &str = "{\"type\":\"error\",\"error\":{\"type\":\"conflict_error\",\
        \"message\":\"Store id 'team.a' collides with existing store 'team-a'.\",\
        \"request_id\":\"req_col\",\"conflicts_with\":\"team-a\"}}";

    #[tokio::test(flavor = "current_thread")]
    async fn a_write_lock_409_is_retried_even_on_a_write() {
        let (base, hits) = mock_server(vec![
            http("409 Conflict", "", WRITE_LOCK),
            http("200 OK", "", "{\"id\":\"m1\",\"status\":\"stored\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let v = mem.add("x", "alice", serde_json::json!({})).await.expect("retried after the lock");
        assert_eq!(v["id"], "m1");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_write_lock_409_waits_at_least_what_it_asked_for() {
        let lock = WRITE_LOCK.replace("\"retry_after_ms\":100", "\"retry_after_ms\":900");
        let (base, hits) = mock_server(vec![
            http("409 Conflict", "", &lock),
            http("200 OK", "", "{\"id\":\"m1\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let t0 = std::time::Instant::now();
        mem.add("x", "alice", serde_json::json!({})).await.unwrap();
        assert!(t0.elapsed() >= std::time::Duration::from_millis(900), "waited only {:?}", t0.elapsed());
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_write_lock_409_stays_inside_the_retry_count() {
        let (base, hits) = mock_server(vec![
            http("409 Conflict", "", WRITE_LOCK),
            http("409 Conflict", "", WRITE_LOCK),
            http("409 Conflict", "", WRITE_LOCK),
            http("200 OK", "", "{\"id\":\"m1\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(e.status(), Some(409), "got {e}");
        assert_eq!(e.kind(), ErrorKind::Conflict);
        assert!(format!("{e}").contains("req_lock"), "got {e}");
        assert_eq!(hits.load(Ordering::SeqCst), 3);

        let (base, hits) = mock_server(vec![
            http("409 Conflict", "", WRITE_LOCK),
            http("200 OK", "", "{\"id\":\"m1\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_retries(0);
        let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Conflict);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_store_id_collision_is_never_retried() {
        let (base, hits) = mock_server(vec![
            http("409 Conflict", "", COLLISION),
            http("200 OK", "", "{\"user_id\":\"team.a\",\"status\":\"created\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem.create_store("team.a").await.unwrap_err();
        assert_eq!(e.status(), Some(409));
        assert_eq!(e.kind(), ErrorKind::Conflict);
        assert!(format!("{e}").contains("collides"), "got {e}");
        assert_eq!(e.conflicts_with(), Some("team-a"));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn conflicts_with_is_only_the_collision_409s_own_string() {
        let (base, _) = mock_server(vec![http("409 Conflict", "", WRITE_LOCK)]);
        let e = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_retries(0)
            .add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(e.status(), Some(409));
        assert_eq!(e.conflicts_with(), None);
        let number = COLLISION.replace("\"team-a\"", "7");
        let not_409 = COLLISION.replace("conflict_error", "invalid_request_error");
        let ctl = COLLISION.replace("\"team-a\"}", "\"team\\u0007-a\"}");
        for (status, body, want) in [
            ("409 Conflict", number.as_str(), None),
            ("400 Bad Request", not_409.as_str(), None),
            ("409 Conflict", ctl.as_str(), Some("team-a")),
        ] {
            let (base, _) = mock_server(vec![http(status, "", body)]);
            let e = Client::with_base_url("wos-test-xxxxxxxxxx", &base).create_store("team.a").await.unwrap_err();
            assert_eq!(e.conflicts_with(), want, "{status} {body}");
        }
        assert_eq!(super::WosError::Api { status: 409, message: "x".into() }.conflicts_with(), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_409_with_both_fields_or_neither_is_not_retried() {
        let both = COLLISION.replace("\"conflicts_with\"", "\"retry_after_ms\":100,\"conflicts_with\"");
        let neither = "{\"error\":{\"type\":\"conflict_error\",\"message\":\"conflict\"}}";
        let plain = "{\"error\":\"conflict\"}";
        for body in [both.as_str(), neither, plain] {
            let (base, hits) = mock_server(vec![
                http("409 Conflict", "", body),
                http("200 OK", "", "{\"id\":\"m1\"}"),
            ]);
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
            let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
            assert_eq!(e.status(), Some(409), "{body}");
            assert_eq!(hits.load(Ordering::SeqCst), 1, "{body} was retried");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_image_retries_a_write_lock_409_too() {
        let (base, hits) = mock_server(vec![
            http("409 Conflict", "", WRITE_LOCK),
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPNG!".to_string(),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let (bytes, ctype) = mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await.unwrap();
        assert_eq!(bytes, b"PNG!");
        assert_eq!(ctype, "image/png");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }
