    use super::tests::{http, mock_server_recording};
    use super::Client;

    fn sent(seen: &std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> serde_json::Value {
        let raw = seen.lock().unwrap()[0].clone();
        let body = raw.split("\r\n\r\n").nth(1).expect("request had no body");
        serde_json::from_str(body).expect("body was not JSON")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_empty_category_is_sent_as_it_is() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.add_bulk("blob", "alice", "").await.unwrap();
        assert_eq!(sent(&seen)["category"], "");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_timestamp_reaches_the_body() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.add_bulk_with("blob", "alice", "notes", serde_json::json!({"timestamp": "2024-03-01T10:00:00Z"}))
            .await
            .unwrap();
        let b = sent(&seen);
        assert_eq!(b["timestamp"], "2024-03-01T10:00:00Z");
        assert_eq!(b["category"], "notes");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_backfill_can_carry_a_date_and_a_key_at_once() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.add_bulk_with_idempotent(
            "blob", "alice", "notes",
            serde_json::json!({"timestamp": "2024-03-01T10:00:00Z"}),
            "import:run-7",
        ).await.unwrap();
        let raw = seen.lock().unwrap()[0].clone();
        assert!(
            raw.to_ascii_lowercase().contains("idempotency-key: import:run-7"),
            "the key did not ride on the write: {raw}"
        );
        assert_eq!(sent(&seen)["timestamp"], "2024-03-01T10:00:00Z");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn extra_cannot_overwrite_the_reserved_fields() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.add_bulk_with("real", "alice", "notes", serde_json::json!({
            "user_id": "somebody_else", "content": "spoofed", "category": "hijacked"
        }))
        .await
        .unwrap();
        let b = sent(&seen);
        assert_eq!(b["user_id"], "alice", "extra redirected the write to another store");
        assert_eq!(b["content"], "real");
        assert_eq!(b["category"], "notes");
    }
