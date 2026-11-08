    use super::tests::{http, mock_server};
    use super::Client;
    use std::sync::atomic::Ordering;

    #[tokio::test(flavor = "current_thread")]
    async fn lists_keep_only_objects() {
        let (base, _) = mock_server(vec![
            http("200 OK", "", "{\"models\":[{\"id\":\"tablet-2\"},null,1,\"x\",[]]}"),
            http("200 OK", "", "{\"collections\":[{\"user_id\":\"a\"},null,{\"user_id\":\"b\"},false]}"),
            http("200 OK", "", "{\"turns\":[{\"user\":\"hi\"},null,\"t\"]}"),
            http("200 OK", "", "{\"memories\":[{\"id\":\"1\"},null,7],\"next_cursor\":null}"),
            http("200 OK", "", "{\"images\":[{\"id\":\"a\"},null,\"b\"],\"has_more\":false}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert_eq!(mem.list_models().await.unwrap().len(), 1);
        assert_eq!(mem.list_stores().await.unwrap().len(), 2);
        assert_eq!(mem.history("alice").await.unwrap().len(), 1);
        assert_eq!(mem.list_all_memories("alice").await.unwrap().len(), 1);
        assert_eq!(mem.list_all_images("alice", None).await.unwrap().len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_empty_cursor_ends_the_walk() {
        let (base, hits) = mock_server(vec![
            http("200 OK", "", "{\"memories\":[{\"id\":\"1\"}],\"next_cursor\":\"\"}"),
            http("200 OK", "", "{\"memories\":[{\"id\":\"1\"}],\"next_cursor\":null}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert_eq!(mem.list_all_memories("alice").await.unwrap().len(), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        let (base, hits) = mock_server(vec![
            http("200 OK", "", "{\"images\":[{\"id\":\"a\"}],\"has_more\":true,\"next_before\":\"\"}"),
            http("200 OK", "", "{\"images\":[{\"id\":\"a\"}],\"has_more\":false}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert_eq!(mem.list_all_images("alice", None).await.unwrap().len(), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_repeated_cursor_after_a_page_with_no_objects_ends_the_walk() {
        let (base, _) = mock_server(vec![
            http("200 OK", "", "{\"memories\":[{\"id\":\"1\"}],\"next_cursor\":\"C\"}"),
            http("200 OK", "", "{\"memories\":[null,7],\"next_cursor\":\"C\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert_eq!(mem.list_all_memories("alice").await.unwrap().len(), 1);

        let page1 = "{\"images\":[{\"id\":\"a\"}],\"has_more\":true,\"next_before\":\"T\",\"next_skip_ids\":[\"a\"]}";
        let page2 = "{\"images\":[null,\"b\"],\"has_more\":true,\"next_before\":\"T\",\"next_skip_ids\":[\"a\"]}";
        let (base, _) = mock_server(vec![http("200 OK", "", page1), http("200 OK", "", page2)]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert_eq!(mem.list_all_images("alice", None).await.unwrap().len(), 1);
    }
