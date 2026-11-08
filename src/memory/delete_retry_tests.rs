    use super::tests::{dropping_server, http, mock_server};
    use super::{Client, ErrorKind};

    const NOTE: &str = "(an earlier attempt may already have deleted it)";
    const GONE: &str = "{\"type\":\"error\",\"error\":{\"type\":\"not_found_error\",\"message\":\"Store not found.\",\"request_id\":\"req_gone\"}}";

    #[tokio::test(flavor = "current_thread")]
    async fn a_404_after_an_ambiguous_retry_says_so() {
        for first in ["502 Bad Gateway", "504 Gateway Timeout", "503 Service Unavailable", "408 Request Timeout"] {
            let (base, _) = mock_server(vec![
                http(first, "Retry-After: 0\r\n", "{\"error\":\"late\"}"),
                http("404 Not Found", "", GONE),
                http(first, "Retry-After: 0\r\n", "{\"error\":\"late\"}"),
                http("404 Not Found", "", GONE),
            ]);
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
            for e in [
                mem.delete_store("tenant_a").await.unwrap_err(),
                mem.remove_speaker("Bob", "tenant_a").await.unwrap_err(),
            ] {
                assert_eq!(e.kind(), ErrorKind::NotFound, "after {first}: {e}");
                let shown = format!("{e}");
                assert!(shown.ends_with(NOTE), "after {first}: {shown}");
                assert!(shown.contains("req_gone"), "after {first}: {shown}");
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_404_after_a_dropped_response_says_so() {
        let (base, _) = dropping_server(1, http("404 Not Found", "", GONE));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem
            .forget_image("tenant_a", "11111111-1111-1111-1111-111111111111", false)
            .await
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotFound);
        assert!(format!("{e}").ends_with(NOTE), "got {e}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_preview_404_after_an_ambiguous_retry_says_nothing_more() {
        let (base, hits) = mock_server(vec![
            http("503 Service Unavailable", "Retry-After: 0\r\n", "{\"error\":\"late\"}"),
            http("404 Not Found", "", GONE),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem
            .forget_image("tenant_a", "11111111-1111-1111-1111-111111111111", true)
            .await
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotFound, "got {e}");
        assert!(!format!("{e}").contains("earlier attempt"), "got {e}");
        assert!(format!("{e}").contains("req_gone"), "got {e}");
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);

        let (base, _) = mock_server(vec![
            http("503 Service Unavailable", "Retry-After: 0\r\n", "{\"error\":\"late\"}"),
            http("404 Not Found", "", GONE),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem
            .forget_image("tenant_a", "11111111-1111-1111-1111-111111111111", false)
            .await
            .unwrap_err();
        assert!(format!("{e}").ends_with(NOTE), "got {e}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_404_without_an_ambiguous_attempt_says_nothing_more() {
        let (base, _) = mock_server(vec![
            http("404 Not Found", "", GONE),
            http("429 Too Many Requests", "Retry-After: 0\r\n", "{\"error\":\"slow\"}"),
            http("404 Not Found", "", GONE),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for e in [
            mem.delete_store("tenant_a").await.unwrap_err(),
            mem.delete_store("tenant_a").await.unwrap_err(),
        ] {
            assert_eq!(e.kind(), ErrorKind::NotFound);
            assert!(!format!("{e}").contains("earlier attempt"), "got {e}");
        }
    }
