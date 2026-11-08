    use super::{Client, ErrorKind, WosError};

    fn kind_of(e: WosError) -> ErrorKind {
        e.kind()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_argument_the_client_refuses_is_a_bad_request() {
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:9");
        for e in [
            mem.search("q", "alice", 50).await.unwrap_err(),
            mem.search("q", "alice", 1).await.unwrap_err(),
            mem.usage(400).await.unwrap_err(),
            mem.get("alice", "  ").await.unwrap_err(),
        ] {
            assert_eq!(kind_of(e), ErrorKind::BadRequest, "a caller mistake must read as one");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_exhausted_deadline_reads_like_a_connection_failure() {
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:9")
            .with_deadline(std::time::Duration::from_millis(1));
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let e = mem.list_stores().await.unwrap_err();
        assert_eq!(kind_of(e), ErrorKind::Connection);
    }
