    use super::rooms_work_tests::{count, refusal, scripted, KEY};
    use super::{
        BulkOpts, Client, DefaultSpeakers, LookOpts, RecallOpts, SearchOpts, SettingsOpts, SleepFileWrite, WriteOpts,
    };

    #[tokio::test(flavor = "current_thread")]
    async fn engram_trims_its_name_and_refuses_a_blank_one() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        for bad in ["", "   ", "\t"] {
            assert_eq!(refusal(mem.engram(bad, "q", "u").await), "name is required.");
            assert_eq!(refusal(mem.engram_with(bad, "q", "u", json!({})).await), "name is required.");
        }
        assert_eq!(count(&seen), 0);
        mem.engram(" timeline ", "q", "u").await.unwrap();
        assert_eq!(seen.lock().unwrap()[0].body["name"], "timeline");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn min_importance_is_0_to_1() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        for bad in [json!(50), json!(-0.1), json!(1.5), json!(true), json!("0.5")] {
            let filters = json!({ "min_importance": bad });
            let recall = RecallOpts { filters: Some(filters.clone()), ..Default::default() };
            for said in [
                refusal(mem.search_with("q", "u", 10, json!({ "filters": filters })).await),
                refusal(mem.search_full_with("q", "u", 10, &SearchOpts::default(), json!({ "filters": filters })).await),
                refusal(mem.recall_opts("q", "u", &recall).await),
            ] {
                assert!(said.starts_with("filters.min_importance must be a number between 0 and 1"), "{said}");
            }
        }
        assert_eq!(count(&seen), 0);
        for ok in [json!(0), json!(0.5), json!(1)] {
            mem.search_with("q", "u", 10, json!({ "filters": { "min_importance": ok } })).await.unwrap();
        }
        let seen = seen.lock().unwrap();
        let got: Vec<_> = seen.iter().map(|s| s.body["filters"]["min_importance"].clone()).collect();
        assert_eq!(got, [json!(0), json!(0.5), json!(1)]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn verify_is_0_to_3_as_max_images_is_0_to_5() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        let opts = SearchOpts { verify: Some(4), ..Default::default() };
        let want = "verify must be an integer between 0 and 3, got 4.";
        assert_eq!(refusal(mem.search_opts("q", "u", 10, &opts).await), want);
        assert_eq!(refusal(mem.search_full("q", "u", 10, &opts).await), want);
        assert_eq!(refusal(mem.search_with("q", "u", 10, json!({"verify": 4})).await), want);
        assert_eq!(refusal(mem.search_self_with("q", "u", 10, json!({"verify": 4})).await), want);
        for bad in [json!(-1), json!(2.5), json!("2"), json!(true)] {
            let said = refusal(mem.search_with("q", "u", 10, json!({ "verify": bad })).await);
            assert!(said.starts_with("verify must be an integer between 0 and 3"), "{said}");
        }
        assert_eq!(count(&seen), 0);
        mem.search_opts("q", "u", 10, &SearchOpts { verify: Some(3), ..Default::default() }).await.unwrap();
        mem.search_with("q", "u", 10, json!({"verify": 0})).await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!([&seen[0].body["verify"], &seen[1].body["verify"]], [&json!(3), &json!(0)]);
    }
    use serde_json::{json, Value};

    #[tokio::test(flavor = "current_thread")]
    async fn sleep_file_answers_keep_only_records_and_always_carry_the_list() {
        let (base, _seen) = scripted(vec![
            (200, r#"{"files": [null, {"slot": 1}, 3], "core": {"slot": 0}}"#),
            (200, r#"{"versions": [{"version": 2}, "x"]}"#),
            (200, r#"{"written": [{"slot": 1}, null]}"#),
        ]);
        let mem = Client::with_base_url(KEY, &base);
        let write = [SleepFileWrite { slot: 1, body: "b".into(), ..Default::default() }];
        let mut got = Vec::new();
        for _ in 0..2 {
            got.push(mem.sleep_files("u").await.unwrap());
            got.push(mem.sleep_file_history(1, "u").await.unwrap());
            got.push(mem.write_sleep_files("s", &write, "u").await.unwrap());
        }
        assert_eq!(
            got,
            [
                json!({"files": [{"slot": 1}], "core": {"slot": 0}}),
                json!({"versions": [{"version": 2}]}),
                json!({"written": [{"slot": 1}]}),
                json!({"files": []}),
                json!({"versions": []}),
                json!({"written": []}),
            ]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_revisions_page_keeps_only_records_as_every_other_page_does() {
        let page = r#"{"revised": 1, "memories": [null, {"id": "a"}, 2, "b"], "has_more": false}"#;
        let (base, _seen) = scripted(vec![(200, page), (200, page), (200, r#"{"revised": 0, "unrevised": 3}"#)]);
        let mem = Client::with_base_url(KEY, &base);
        let want = json!({"revised": 1, "memories": [{"id": "a"}], "has_more": false});
        assert_eq!(mem.revisions_page("u", "revised", None, None, None).await.unwrap(), want);
        assert_eq!(mem.revisions("u").await.unwrap(), want);
        assert_eq!(mem.revisions("u").await.unwrap(), json!({"revised": 0, "unrevised": 3}));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_sends_a_null_metadata_as_an_empty_object() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        mem.add("x", "u", Value::Null).await.unwrap();
        mem.add_with("x", "u", Value::Null, json!({})).await.unwrap();
        mem.add_opts("x", "u", Value::Null, &WriteOpts::default()).await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.iter().map(|s| s.body["metadata"].clone()).collect::<Vec<_>>(), [json!({}), json!({}), json!({})]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn update_trims_the_old_id_and_refuses_a_blank_one() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        for bad in ["", "   ", "\t\n"] {
            assert_eq!(refusal(mem.update(bad, "new", "u").await), "old_memory_id is required.");
            assert_eq!(refusal(mem.update_opts(bad, "new", "u", &WriteOpts::default()).await), "old_memory_id is required.");
            assert_eq!(refusal(mem.update_idempotent(bad, "new", "u", "k-1").await), "old_memory_id is required.");
        }
        assert_eq!(count(&seen), 0);
        mem.update(" m1 ", "new", "u").await.unwrap();
        assert_eq!(seen.lock().unwrap()[0].body["old_memory_id"], "m1");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn update_refuses_a_blank_new_content_and_sends_the_rest_as_given() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        for bad in ["", "   ", "\n"] {
            assert_eq!(refusal(mem.update("m1", bad, "u").await), "new_content is required.");
            assert_eq!(refusal(mem.update_opts("m1", bad, "u", &WriteOpts::default()).await), "new_content is required.");
        }
        assert_eq!(count(&seen), 0);
        mem.update("m1", " the fix ", "u").await.unwrap();
        assert_eq!(seen.lock().unwrap()[0].body["new_content"], " the fix ");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_checks_its_metadata_speaker_as_add_bulk_does() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        for (md, want) in [
            (json!({"speaker": "  "}), "speaker is required."),
            (json!({"speaker": "A".repeat(257)}), "speaker is at most 256 bytes."),
        ] {
            assert_eq!(refusal(mem.add("x", "u", md).await), want);
        }
        assert_eq!(count(&seen), 0);
        mem.add("x", "u", json!({"speaker": "\t Ana \n", "category": "work"})).await.unwrap();
        let bulk = BulkOpts { speaker: Some(" Ana ".into()), ..Default::default() };
        mem.add_bulk_opts("x", "u", "general", &bulk).await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!([&seen[0].body["metadata"], &seen[1].body["metadata"]], [&json!({"speaker": "Ana", "category": "work"}), &json!({"speaker": "Ana"})]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_and_remove_speaker_trim_the_name_and_refuse_a_blank_one() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        assert_eq!(refusal(mem.add_speaker("  ", "u").await), "speaker is required.");
        assert_eq!(refusal(mem.add_speaker(&"A".repeat(257), "u").await), "speaker is at most 256 bytes.");
        assert_eq!(refusal(mem.remove_speaker("  ", "u").await), "speaker is required.");
        assert_eq!(count(&seen), 0);
        mem.add_speaker(" Ana ", "u").await.unwrap();
        mem.remove_speaker(" Ana ", "u").await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!([&seen[0].body["speaker"], &seen[1].body["speaker"]], [&json!("Ana"), &json!("Ana")]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_blank_image_is_refused_as_required_and_only_a_bare_prefix_as_empty_after_it() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        let required = "image.data is required — base64 of the image (a data: URL is fine)";
        for data in ["", "   ", "\n\t"] {
            assert_eq!(refusal(mem.add_with("x", "u", json!({}), json!({"image": {"data": data}})).await), required);
        }
        let bare = refusal(mem.add_with("x", "u", json!({}), json!({"image": {"data": "data:image/png;base64, "}})).await);
        assert_eq!(bare, "image.data is empty after stripping its data: prefix");
        assert_eq!(count(&seen), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_default_or_relabelled_speaker_is_at_most_256_bytes_as_add_speaker_takes() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        let named = |user: String| SettingsOpts {
            default_speakers: Some(DefaultSpeakers { user: Some(Some(user)), ..Default::default() }),
            ..Default::default()
        };
        let long = "A".repeat(257);
        assert_eq!(refusal(mem.set_settings("u", &named(long.clone())).await), "default_speakers.user is at most 256 bytes.");
        assert_eq!(refusal(mem.relabel_speaker("Ana", &long, "u", None).await), "to_speaker is at most 256 bytes.");
        assert_eq!(count(&seen), 0);
        mem.set_settings("u", &named(format!(" {} ", "A".repeat(256)))).await.unwrap();
        assert_eq!(seen.lock().unwrap()[0].body["default_speakers"]["user"], json!("A".repeat(256)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_export_goes_on_past_a_cursor_that_is_a_number_as_python_and_typescript_do() {
        let (base, seen) = scripted(vec![
            (200, r#"{"memories": [{"id": "a"}], "next_cursor": 7}"#),
            (200, r#"{"memories": [{"id": "b"}], "next_cursor": "c2"}"#),
            (200, r#"{"memories": [{"id": "c"}], "next_cursor": 0}"#),
        ]);
        let mem = Client::with_base_url(KEY, &base);
        let ids: Vec<_> = mem.export_memories("u").await.unwrap().into_iter().map(|m| m["id"].clone()).collect();
        assert_eq!(ids, [json!("a"), json!("b"), json!("c")]);
        let seen = seen.lock().unwrap();
        let cursors: Vec<_> = seen.iter().map(|s| s.body.get("cursor").cloned()).collect();
        assert_eq!(cursors, [None, Some(json!(7)), Some(json!("c2"))]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_look_checks_its_tz_without_rooms_as_with_them() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        for bad in [15.0, -14.5, f64::NAN, f64::INFINITY] {
            let opts = LookOpts { tz: Some(bad), ..Default::default() };
            assert!(refusal(mem.open_work("q", "u", &opts).await).contains("between -14 and 14"), "{bad}");
            assert!(refusal(mem.work_search("w", "q", "u", &opts).await).contains("between -14 and 14"), "{bad}");
        }
        assert_eq!(count(&seen), 0);
        mem.open_work("q", "u", &LookOpts { tz: Some(9.0), ..Default::default() }).await.unwrap();
        assert_eq!(seen.lock().unwrap()[0].body, json!({"user_id": "u", "question": "q"}));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_per_call_tz_in_extra_is_checked_before_sending() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        let mut missed = Vec::new();
        for bad in [json!(15), json!(-14.5), json!("9"), json!(true)] {
            let extra = json!({ "tz": bad });
            for (name, said) in [
                ("recall_with", mem.recall_with("q", "u", extra.clone()).await.map(drop)),
                ("engram_with", mem.engram_with("e", "q", "u", extra.clone()).await.map(drop)),
                ("search_with", mem.search_with("q", "u", 10, extra.clone()).await.map(drop)),
                ("search_full_with", mem.search_full_with("q", "u", 10, &SearchOpts::default(), extra.clone()).await.map(drop)),
                ("search_self_with", mem.search_self_with("q", "u", 10, extra.clone()).await.map(drop)),
            ] {
                match said {
                    Err(e) if e.to_string().contains("between -14 and 14") => {}
                    other => missed.push(format!("{name} {bad}: {other:?}")),
                }
            }
        }
        assert_eq!(missed, Vec::<String>::new());
        assert_eq!(count(&seen), 0);
        mem.engram_with("e", "q", "u", json!({ "tz": 5.5 })).await.unwrap();
        assert_eq!(seen.lock().unwrap()[0].body["tz"], 5.5);
    }
