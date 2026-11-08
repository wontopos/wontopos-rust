    use super::tests::{http, mock_server};
    use super::{Client, SearchOpts, WosError};
    use std::sync::atomic::Ordering;

    fn refused(r: Result<(), WosError>, range: &str, what: &str) {
        match r {
            Err(WosError::Api { status: 400, message, .. }) => {
                assert!(message.contains(range), "{what}: got {message}")
            }
            other => panic!("{what} should be refused locally, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn image_speaker_and_revision_pages_are_5_to_20() {
        let (base, hits) = mock_server(vec![]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for bad in [0usize, 4, 21, 100, usize::MAX] {
            refused(mem.list_images("alice", bad, None, None).await.map(|_| ()), "between 5 and 20", "list_images");
            refused(mem.list_all_images("alice", bad).await.map(|_| ()), "between 5 and 20", "list_all_images");
            refused(mem.export_images("alice", bad).await.map(|_| ()), "between 5 and 20", "export_images");
            refused(mem.iter_images("alice", bad).await.map(|_| ()), "between 5 and 20", "iter_images");
            refused(mem.by_speaker("Bob", "alice", bad, None, None).await.map(|_| ()), "between 5 and 20", "by_speaker");
            refused(
                mem.revisions_page("alice", "revised", bad, None, None).await.map(|_| ()),
                "between 5 and 20",
                "revisions_page",
            );
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing may reach the network");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_ends_of_each_range_are_sent() {
        let ok = || http("200 OK", "", "{\"images\":[],\"memories\":[],\"has_more\":false}");
        let (base, hits) = mock_server((0..12).map(|_| ok()).collect());
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for n in [5usize, 20] {
            mem.list_images("alice", n, None, None).await.unwrap();
            mem.list_all_images("alice", n).await.unwrap();
            mem.by_speaker("Bob", "alice", n, None, None).await.unwrap();
            mem.revisions_page("alice", "revised", n, None, None).await.unwrap();
        }
        mem.list_memories("alice", 1, None).await.unwrap();
        mem.list_memories("alice", 500, None).await.unwrap();
        mem.search_opts("q", "alice", 10, &SearchOpts { max_images: Some(0), ..Default::default() }).await.unwrap();
        mem.search_opts("q", "alice", 10, &SearchOpts { max_images: Some(5), ..Default::default() }).await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 12);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn max_images_is_0_to_5() {
        let (base, hits) = mock_server(vec![]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for bad in [6usize, 100] {
            let opts = SearchOpts { max_images: Some(bad), ..Default::default() };
            refused(mem.search_opts("q", "alice", 10, &opts).await.map(|_| ()), "between 0 and 5", "search_opts");
            refused(mem.search_full("q", "alice", 10, &opts).await.map(|_| ()), "between 0 and 5", "search_full");
        }
        for bad in [serde_json::json!(true), serde_json::json!(6), serde_json::json!(-1), serde_json::json!(2.5), serde_json::json!("3")] {
            refused(
                mem.search_with("q", "alice", 10, serde_json::json!({ "max_images": bad })).await.map(|_| ()),
                "between 0 and 5",
                "search_with",
            );
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing may reach the network");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_null_max_images_is_read_as_absent() {
        let ok = || http("200 OK", "", "{\"memories\":[],\"self_memories\":[]}");
        let (base, hits) = mock_server((0..3).map(|_| ok()).collect());
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let none: Option<usize> = None;
        let extra = || serde_json::json!({ "max_images": none });
        mem.search_with("q", "alice", 10, extra()).await.expect("search_with");
        mem.search_self_with("q", "alice", 10, extra()).await.expect("search_self_with");
        mem.search_full_with("q", "alice", 10, &SearchOpts::default(), extra()).await.expect("search_full_with");
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_memories_limit_is_1_to_500() {
        let (base, hits) = mock_server(vec![]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for bad in [0usize, 501, usize::MAX] {
            refused(mem.list_memories("alice", bad, None).await.map(|_| ()), "between 1 and 500", "list_memories");
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing may reach the network");
    }
