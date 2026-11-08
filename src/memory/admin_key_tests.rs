    use super::rooms_work_tests::{scripted, KEY};
    use super::tests::{http, mock_server_recording};
    use super::{Client, ErrorKind, WosError};
    use serde_json::json;

    const ADMIN_ONLY: &str = "Only an admin key lists stores; this is a standard API key.";

    #[tokio::test(flavor = "current_thread")]
    async fn ping_and_key_scope_ask_the_ping_endpoint() {
        let ping = r#"{"ok":true,"scope":"admin"}"#;
        let (base, seen) = scripted(vec![(200, ping), (200, ping), (200, r#"{"ok":true}"#)]);
        let mem = Client::with_base_url(KEY, &base);
        assert!(mem.ping().await.unwrap());
        assert_eq!(mem.key_scope().await.unwrap(), "admin");
        let e = mem.key_scope().await.unwrap_err();
        assert!(matches!(&e, WosError::Api { status: 200, message } if message == "the answer carried no key scope"), "{e}");
        let sent: Vec<String> = seen.lock().unwrap().iter().map(|s| format!("{} {}", s.method, s.path)).collect();
        assert_eq!(sent, vec!["GET /api/v1/memory/ping"; 3]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_standard_key_cannot_list_stores_where_only_an_admin_key_does() {
        let (base, seen) = scripted(vec![
            (200, r#"{"collections":[],"count":0,"admin_only":true}"#),
            (200, r#"{"collections":[{"user_id":"a"}],"admin_only":false}"#),
            (200, r#"{"collections":[{"user_id":"a"},null]}"#),
        ]);
        let mem = Client::with_base_url(KEY, &base);
        let e = mem.list_stores().await.unwrap_err();
        assert_eq!((e.status(), e.kind(), e.code()), (Some(403), ErrorKind::PermissionDenied, None));
        assert!(matches!(&e, WosError::Api { status: 403, message } if message == ADMIN_ONLY), "{e}");
        assert_eq!(format!("{e}"), format!("[403] {ADMIN_ONLY}"));
        assert_eq!(mem.list_stores().await.unwrap(), vec![json!({"user_id": "a"})]);
        assert_eq!(mem.list_stores().await.unwrap(), vec![json!({"user_id": "a"})]);
        assert_eq!(seen.lock().unwrap().len(), 3);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn usage_carries_stores_admin_only_and_fails_nothing() {
        let (base, _) = scripted(vec![(200, r#"{"stores":[],"stores_admin_only":true,"balance_cents":5}"#)]);
        let u = Client::with_base_url(KEY, &base).usage(7).await.unwrap();
        assert_eq!(u, json!({"stores": [], "stores_admin_only": true, "balance_cents": 5}));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_admin_key_is_sent_as_given_and_its_403_reads_as_the_service_said() {
        let key = format!("wos-admin-{}", "k".repeat(147));
        assert_eq!(key.len(), 157);
        let said = "This is an admin key: it lists stores and reads usage and revision counts, and opens no memory. \
                    Use a standard API key for this call.";
        let body = json!({"type": "error", "error": {"type": "permission_error", "message": said, "request_id": "req_adm"}});
        let (base, reqs) = mock_server_recording(vec![http("403 Forbidden", "", &body.to_string())]);
        let e = Client::with_base_url(&key, &base).search("q", "alice", 10).await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::PermissionDenied);
        assert_eq!(format!("{e}"), format!("[403] {said} (request_id: req_adm)"));
        assert!(reqs.lock().unwrap()[0].to_lowercase().contains(&format!("x-api-key: {key}")), "the key goes as given");
    }
