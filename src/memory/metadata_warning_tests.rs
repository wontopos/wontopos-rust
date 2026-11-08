    use super::tests::{http, mock_server};
    use super::{Client, WARNED_METADATA_KEYS};

    fn warned(key: &str) -> bool {
        WARNED_METADATA_KEYS
            .get()
            .map(|m| m.lock().unwrap().contains(key))
            .unwrap_or(false)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn every_add_warns_on_an_unknown_metadata_key() {
        let ok = || http("200 OK", "", "{\"id\":\"m1\"}");
        let (base, _) = mock_server(vec![ok(), ok(), ok(), ok(), ok()]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let md = |k: &str| serde_json::json!({ k: "x", "speaker": "me", "event_date": "2026-03-14",
                                                 "category": "work", "conversation_id": "c1" });
        mem.add("x", "alice", md("speakr_mdw_add")).await.unwrap();
        mem.store("x", "alice", md("speakr_mdw_store")).await.unwrap();
        mem.add_idempotent("x", "alice", md("speakr_mdw_idem"), "k1").await.unwrap();
        mem.add_with("x", "alice", md("speakr_mdw_with"), serde_json::json!({})).await.unwrap();
        mem.add_with_idempotent("x", "alice", md("speakr_mdw_withidem"), serde_json::json!({}), "k2")
            .await
            .unwrap();
        for k in ["speakr_mdw_add", "speakr_mdw_store", "speakr_mdw_idem", "speakr_mdw_with", "speakr_mdw_withidem"] {
            assert!(warned(k), "{k} was not warned about");
        }
        for k in ["speaker", "event_date", "category", "conversation_id"] {
            assert!(!warned(k), "{k} is kept by the service and must not warn");
        }
    }
