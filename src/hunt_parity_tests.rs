    use super::rooms_work_tests::{count, refusal, scripted, KEY};
    use super::{Client, SleepFileWrite};
    use serde_json::json;

    #[tokio::test(flavor = "current_thread")]
    async fn a_sleep_file_answer_that_leaves_its_list_out_or_mixes_in_non_objects_holds_only_its_objects() {
        let (base, _seen) = scripted(vec![
            (200, r#"{"core": {"slot": 0}, "files": [null, 3, {"slot": 1}]}"#),
            (200, r#"{"slot": 2}"#),
            (200, r#"{"written": "x"}"#),
        ]);
        let mem = Client::with_base_url(KEY, &base).with_retries(0);
        assert_eq!(mem.sleep_files("u").await.unwrap()["files"], json!([{"slot": 1}]));
        assert_eq!(mem.sleep_file_history(2, "u").await.unwrap()["versions"], json!([]));
        let write = SleepFileWrite { slot: 1, title: "t".into(), body: "b".into(), ..Default::default() };
        assert_eq!(mem.write_sleep_files("s1", &[write], "u").await.unwrap()["written"], json!([]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn answered_with_no_number_closes_the_work_without_an_answer() {
        let opened = r#"{"work_id": "w1", "memories": [{"n": 1, "id": "m1", "content": "x"}]}"#;
        let (base, seen) = scripted(vec![(200, opened), (200, r#"{"closed": true}"#)]);
        let mem = Client::with_base_url(KEY, &base).with_retries(0);
        let tools = mem.memory_tools_preset("u", None, "S-2").unwrap();
        tools.begin("q").await.unwrap();
        assert_eq!(tools.answered(None).await.unwrap(), Some(json!({"closed": true})));
        {
            let log = seen.lock().unwrap();
            assert_eq!(log[1].path, "/api/v1/memory/work/close");
            assert_eq!(log[1].body, json!({"user_id": "u", "work_id": "w1"}));
        }
        assert_eq!(tools.answered(None).await.unwrap(), None, "nothing is open any more");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_key_with_a_stray_newline_a_no_break_space_or_a_control_character_is_refused_for_what_it_holds() {
        let (base, seen) = scripted(vec![]);
        let spaced = "api_key contains whitespace - check for a stray newline or paste error";
        for (key, said) in [
            ("wos-test\nxxxxxxxxxx", spaced),
            ("wos-test\u{a0}xxxxxxxxxx", spaced),
            ("wos-test\u{1}xxxxxxxxxx", "api_key contains a control character - check for a paste error"),
        ] {
            let mem = Client::with_base_url(key, &base).with_retries(0);
            assert_eq!(refusal(mem.stats("u").await), said, "{key:?}");
        }
        let mem = Client::with_base_url("wos-test\u{85}xxxxxxxxxx", &base).with_retries(0);
        let said = refusal(mem.stats("u").await);
        assert!(said.starts_with("api_key contains a non-ASCII character ("), "{said}");
        assert_eq!(count(&seen), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_answer_of_only_whitespace_is_refused_as_an_empty_body() {
        let (base, _seen) = scripted(vec![(200, "  \n")]);
        let mem = Client::with_base_url(KEY, &base).with_retries(0);
        let e = mem.stats("u").await.unwrap_err();
        assert_eq!(e.status(), Some(200));
        assert!(e.to_string().starts_with("[200] empty response body"), "{e}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_blank_store_id_is_refused_without_a_store_id_warning() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base).with_retries(0);
        let blank = "\u{3000}";
        assert!(refusal(mem.delete_store(blank).await).starts_with("user_id is required"));
        assert!(refusal(mem.delete_all(blank).await).starts_with("user_id is required"));
        assert!(refusal(mem.forget_speaker(["Dana"], blank, false).await).starts_with("user_id is required"));
        let warned = super::WARNED_STORE_IDS.get().is_some_and(|m| m.lock().unwrap().iter().any(|s| s == blank));
        assert!(!warned, "a refused blank id was warned about as a store id");
        assert_eq!(count(&seen), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_page_size_out_of_range_is_refused_under_its_own_name() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base).with_retries(0);
        assert_eq!(
            refusal(mem.list_all_images("u", 3).await),
            "page_size must be an integer between 5 and 20, got 3."
        );
        assert_eq!(
            refusal(mem.export_images("u", 21).await),
            "page_size must be an integer between 5 and 20, got 21."
        );
        assert_eq!(count(&seen), 0);
    }
