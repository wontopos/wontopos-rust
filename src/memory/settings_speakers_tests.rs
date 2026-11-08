    use super::rooms_work_tests::{count, refusal, scripted, KEY};
    use super::tests::{http, mock_server, mock_server_recording};
    use super::{
        BulkOpts, Client, DefaultSpeakers, ErrorKind, RecallOpts, RecallResponse, Rooms, Segment, SettingsOpts,
        SleepReadOpts, WriteOpts, READ_POSTS,
    };
    use serde_json::{json, Value};
    use std::sync::atomic::Ordering;

    #[tokio::test(flavor = "current_thread")]
    async fn recall_sends_each_option_as_given_and_nothing_it_was_not_given() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base).with_tz(9.0).unwrap();
        let every = RecallOpts {
            room: Some("n1".into()),
            room_limit: Some(20),
            room_expand_tokens: Some(0),
            long_term: Some(true),
            long_term_cutoff: Some(false),
            today_start: Some(" 2026-10-19T13:00:00Z ".into()),
            now: Some("2026-10-20T01:00:00Z".into()),
            short_term_limit: Some(0),
            short_term_budget_tokens: Some(8000),
            max_images: Some(5),
            filters: Some(json!({"categories": ["work"]})),
            speaker: Some("Ana".into()),
            include_superseded: Some(true),
            ..Default::default()
        };
        mem.recall_opts("q", "u", &every).await.unwrap();
        mem.recall_opts("q", "u", &RecallOpts::default()).await.unwrap();
        let one = RecallOpts { room: Some("m0".into()), ..Default::default() };
        mem.recall_rooms("q", "u", ["a1"], &one).await.unwrap();
        let plain = Client::with_base_url(KEY, &base);
        plain.recall_opts("q", "u", &one).await.unwrap();
        plain.recall_opts("q", "u", &RecallOpts { filters: Some(Value::Null), ..Default::default() }).await.unwrap();
        let seen = seen.lock().unwrap();
        assert!(seen.iter().all(|s| s.path == "/api/v1/memory/recall"));
        assert_eq!(
            seen[0].body,
            json!({
                "user_id": "u", "query": "q", "tz": 9, "tz_offset_minutes": 540,
                "room": {"days_ago": 1, "slot": 2}, "room_limit": 20, "room_expand_tokens": 0,
                "long_term": true, "long_term_cutoff": false,
                "today_start": "2026-10-19T13:00:00Z", "now": "2026-10-20T01:00:00Z",
                "short_term_limit": 0, "short_term_budget_tokens": 8000, "max_images": 5,
                "filters": {"categories": ["work"]}, "speaker": "Ana", "include_superseded": true
            })
        );
        assert_eq!(seen[1].body, json!({"user_id": "u", "query": "q", "tz": 9}), "nothing it was not given");
        assert_eq!(
            seen[2].body,
            json!({"user_id": "u", "query": "q", "tz": 9, "rooms": ["a1"], "room": {"days_ago": 0, "slot": 0},
                   "tz_offset_minutes": 540})
        );
        assert_eq!(seen[3].body, json!({"user_id": "u", "query": "q", "room": {"days_ago": 0, "slot": 0}}));
        assert_eq!(seen[4].body, json!({"user_id": "u", "query": "q"}), "a null filter is not sent");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn recall_refuses_an_option_out_of_range_before_sending() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        let opts = |set: fn(&mut RecallOpts)| {
            let mut o = RecallOpts::default();
            set(&mut o);
            o
        };
        for (o, want) in [
            (opts(|o| o.room = Some("all".into())), "room must be one room name"),
            (opts(|o| o.room = Some("m3".into())), "room must be one room name"),
            (opts(|o| o.room_limit = Some(0)), "room_limit must be an integer between 1 and 20, got 0"),
            (opts(|o| o.room_limit = Some(21)), "room_limit must be an integer between 1 and 20, got 21"),
            (opts(|o| o.room_expand_tokens = Some(20_001)), "room_expand_tokens must be an integer between 0 and 20000"),
            (opts(|o| o.short_term_limit = Some(501)), "short_term_limit must be an integer between 0 and 500"),
            (opts(|o| o.short_term_budget_tokens = Some(8_001)), "short_term_budget_tokens must be an integer between 0 and 8000"),
            (opts(|o| o.max_images = Some(6)), "max_images must be an integer between 0 and 5"),
            (opts(|o| o.today_start = Some(" ".into())), "today_start is required"),
            (opts(|o| o.now = Some(String::new())), "now is required"),
        ] {
            let m = refusal(mem.recall_opts("q", "u", &o).await);
            assert!(m.contains(want), "want {want:?}, got {m}");
        }
        let room = RecallOpts { room: Some("m0".into()), ..Default::default() };
        assert!(refusal(mem.recall_rooms("q", "u", Rooms::None, &room).await).contains("pass one or the other"));
        let m = refusal(mem.recall_with("q", "u", json!({"max_images": 9})).await);
        assert!(m.contains("max_images must be an integer between 0 and 5"), "{m}");
        assert_eq!(count(&seen), 0);
    }

    #[test]
    fn a_turn_names_who_said_it() {
        let r: RecallResponse = serde_json::from_value(json!({"short_term": {"turns": [
            {"role": "user", "content": "hi", "timestamp": "2026-10-19T09:00:00Z", "speaker": "Ana"},
            {"role": "assistant", "content": "hello", "speaker": null}
        ]}}))
        .unwrap();
        let turns = r.turns();
        assert_eq!(turns[0].speaker.as_deref(), Some("Ana"));
        assert_eq!(turns[1].speaker, None);
        assert!(turns[0].extra.get("speaker").is_none(), "read as a field, not left in extra");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn settings_are_read_as_sent_and_set_with_only_what_was_given() {
        let back = r#"{"user_id":"u","time_zone":"Asia/Seoul","default_speakers":{"user":"Ana","assistant":null},"history":[]}"#;
        let (base, seen) = scripted(vec![(200, back)]);
        let mem = Client::with_base_url(KEY, &base);
        assert_eq!(mem.settings("u").await.unwrap(), serde_json::from_str::<Value>(back).unwrap());
        let zone = SettingsOpts { time_zone: Some(" Asia/Seoul ".into()), ..Default::default() };
        mem.set_settings("u", &zone).await.unwrap();
        let both = SettingsOpts {
            time_zone: Some("Europe/Paris".into()),
            default_speakers: Some(DefaultSpeakers { user: Some(Some("Ana".into())), assistant: Some(None) }),
            confirmed_by_user: Some(true),
        };
        mem.set_settings("u", &both).await.unwrap();
        let user_only = DefaultSpeakers { user: Some(Some("Ana".into())), ..Default::default() };
        mem.set_settings(None, &SettingsOpts { default_speakers: Some(user_only), ..Default::default() }).await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!((seen[0].method.as_str(), seen[0].path.as_str()), ("GET", "/api/v1/memory/settings?user_id=u"));
        assert_eq!(seen[0].body, Value::Null, "a read sends no body");
        assert!(seen[1..].iter().all(|s| s.method == "POST" && s.path == "/api/v1/memory/settings"));
        assert_eq!(seen[1].body, json!({"user_id": "u", "time_zone": "Asia/Seoul"}));
        assert_eq!(
            seen[2].body,
            json!({"user_id": "u", "time_zone": "Europe/Paris", "default_speakers": {"user": "Ana", "assistant": null},
                   "confirmed_by_user": true})
        );
        assert_eq!(seen[3].body, json!({"user_id": "default", "default_speakers": {"user": "Ana"}}));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn set_settings_refuses_nothing_to_change_and_a_blank_name_before_sending() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        let blank = DefaultSpeakers { assistant: Some(Some(" ".into())), ..Default::default() };
        for (o, want) in [
            (SettingsOpts::default(), "needs time_zone or default_speakers"),
            (SettingsOpts { confirmed_by_user: Some(true), ..Default::default() }, "needs time_zone or default_speakers"),
            (
                SettingsOpts { default_speakers: Some(DefaultSpeakers::default()), ..Default::default() },
                "default_speakers must name user, assistant or both",
            ),
            (
                SettingsOpts {
                    time_zone: Some("Europe/Lisbon".into()),
                    default_speakers: Some(DefaultSpeakers::default()),
                    ..Default::default()
                },
                "default_speakers must name user, assistant or both",
            ),
            (SettingsOpts { time_zone: Some("  ".into()), ..Default::default() }, "time_zone is required"),
            (
                SettingsOpts { default_speakers: Some(blank), ..Default::default() },
                "default_speakers.assistant must be a name, or Some(None) to clear it",
            ),
        ] {
            let m = refusal(mem.set_settings("u", &o).await);
            assert!(m.contains(want), "want {want:?}, got {m}");
        }
        assert_eq!(count(&seen), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_time_zone_change_not_confirmed_is_a_conflict_in_the_services_words_sent_once() {
        let said = r#"{"error":{"code":"time_zone_change_unconfirmed","message":"This store is on Asia/Seoul. Changing it needs confirmed_by_user: true."}}"#;
        let (base, seen) = scripted(vec![(409, said)]);
        let mem = Client::with_base_url(KEY, &base);
        let paris = SettingsOpts { time_zone: Some("Europe/Paris".into()), ..Default::default() };
        let e = mem.set_settings("u", &paris).await.unwrap_err();
        assert_eq!((e.status(), e.kind()), (Some(409), ErrorKind::Conflict));
        assert!(e.to_string().contains("This store is on Asia/Seoul. Changing it needs confirmed_by_user: true."), "{e}");
        assert_eq!(count(&seen), 1, "not sent again");
    }

    const PARTIAL: &str = r#"{"status":"partial","complete":false,"left":5,"again":"send the same request again"}"#;

    #[tokio::test(flavor = "current_thread")]
    async fn relabel_sends_a_partial_answer_again_three_more_times_at_most_and_returns_the_last() {
        let done = r#"{"status":"relabelled","complete":true}"#;
        let (base, seen) = scripted(vec![(200, PARTIAL), (200, PARTIAL), (200, PARTIAL), (200, PARTIAL), (200, done)]);
        let mem = Client::with_base_url(KEY, &base);
        let last = mem.relabel_speaker(" Ana ", "Anna", "u", None).await.unwrap();
        assert_eq!((&last["complete"], last["again"].is_string()), (&json!(false), true), "the last answer, as it came");
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 4, "the first send and three more");
            for s in seen.iter() {
                assert_eq!(s.path, "/api/v1/memory/speakers/relabel");
                assert_eq!(s.body, json!({"user_id": "u", "from": "Ana", "to": "Anna"}), "the same request each time");
            }
        }
        let (base, seen) = scripted(vec![(200, PARTIAL), (200, r#"{"status":"relabelled","complete":true,"relabelled":3}"#)]);
        let ids = vec!["a".to_string(), "b".to_string()];
        let r = Client::with_base_url(KEY, &base).relabel_speaker("Ana", "Anna", "u", Some(&ids)).await.unwrap();
        assert_eq!(r["relabelled"], 3);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1].body, json!({"user_id": "u", "from": "Ana", "to": "Anna", "memory_ids": ["a", "b"]}));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn relabel_checks_its_names_and_ids_before_sending() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        let none: Vec<String> = Vec::new();
        let many: Vec<String> = (0..1001).map(|i| i.to_string()).collect();
        for (m, want) in [
            (refusal(mem.relabel_speaker(" ", "Anna", "u", None).await), "from_speaker is required"),
            (refusal(mem.relabel_speaker("Ana", "", "u", None).await), "to_speaker is required"),
            (refusal(mem.relabel_speaker("Ana", "Anna", "u", Some(&none)).await), "between 1 and 1000 ids, got 0"),
            (refusal(mem.relabel_speaker("Ana", "Anna", "u", Some(&many)).await), "between 1 and 1000 ids, got 1001"),
        ] {
            assert!(m.contains(want), "want {want:?}, got {m}");
        }
        assert_eq!(count(&seen), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn forget_speaker_sends_a_partial_answer_again_but_never_a_preview() {
        let (base, seen) = scripted(vec![(200, PARTIAL), (200, PARTIAL), (200, PARTIAL), (200, PARTIAL)]);
        let mem = Client::with_base_url(KEY, &base);
        let last = mem.forget_speaker(["Ana", " Bob "], "u", false).await.unwrap();
        assert_eq!(last["complete"], false);
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 4, "the first send and three more");
            for s in seen.iter() {
                assert_eq!(s.path, "/api/v1/memory/forget");
                assert_eq!(s.body, json!({"user_id": "u", "speakers": ["Ana", "Bob"]}));
            }
        }
        let (base, seen) = scripted(vec![(200, PARTIAL)]);
        let names = vec![String::from("Ana")];
        Client::with_base_url(KEY, &base).forget_speaker(&names, "u", true).await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].body, json!({"user_id": "u", "speakers": ["Ana"], "preview": true}));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn forget_speaker_refuses_a_blank_store_or_name_before_sending() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base).with_user("alice");
        let nobody: [&str; 0] = [];
        for (m, want) in [
            (refusal(mem.forget_speaker(["Ana"], " ", false).await), "user_id is required (non-blank) for forget_speaker"),
            (refusal(mem.forget_speaker(nobody, "u", false).await), "needs at least one speaker name"),
            (refusal(mem.forget_speaker(["Ana", "  "], "u", true).await), "speakers[1] is required"),
        ] {
            assert!(m.contains(want), "want {want:?}, got {m}");
        }
        assert_eq!(count(&seen), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_speakers_name_and_a_turns_time_are_trimmed_as_python_and_typescript_trim_them_in_every_call() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        let names = DefaultSpeakers { user: Some(Some("\u{feff}Dana ".into())), assistant: Some(Some("Kai\u{3000}".into())) };
        mem.set_settings("u", &SettingsOpts { default_speakers: Some(names), ..Default::default() }).await.unwrap();
        mem.relabel_speaker("\u{feff}Dana", "Kai\u{3000}", "u", None).await.unwrap();
        mem.forget_speaker(["\u{feff}Dana", "\u{3000}Kai\u{2028}"], "u", true).await.unwrap();
        mem.by_speaker("Dana\u{feff}", "u", None, None, None).await.unwrap();
        let turn = WriteOpts {
            created_at: Some("\u{feff}2026-05-02T09:00:00Z".into()),
            user_speaker: Some("Dana\u{feff}".into()),
            ..Default::default()
        };
        mem.add_turn_opts("q", "a", "u", &turn).await.unwrap();
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen[0].body["default_speakers"], json!({"user": "Dana", "assistant": "Kai"}));
            assert_eq!((&seen[1].body["from"], &seen[1].body["to"]), (&json!("Dana"), &json!("Kai")));
            assert_eq!(seen[2].body["speakers"], json!(["Dana", "Kai"]));
            assert_eq!(seen[3].body["speaker"], "Dana");
            assert_eq!((&seen[4].body["created_at"], &seen[4].body["user_speaker"]), (&json!("2026-05-02T09:00:00Z"), &json!("Dana")));
        }
        let bom = SettingsOpts {
            default_speakers: Some(DefaultSpeakers { user: Some(Some("\u{feff}".into())), ..Default::default() }),
            ..Default::default()
        };
        let late = WriteOpts { created_at: Some("2026-05-02T09:00:00Z\u{85}".into()), ..Default::default() };
        let nel = SettingsOpts {
            default_speakers: Some(DefaultSpeakers { assistant: Some(Some("Kai\u{85}".into())), ..Default::default() }),
            ..Default::default()
        };
        for (m, want) in [
            (refusal(mem.set_settings("u", &nel).await), "default_speakers.assistant contains a line break or a control character."),
            (refusal(mem.relabel_speaker("Dana", "Kai\u{85}", "u", None).await), "to_speaker contains a line break or a control character."),
            (refusal(mem.set_settings("u", &bom).await), "default_speakers.user must be a name"),
            (refusal(mem.relabel_speaker("\u{feff}", "Kai", "u", None).await), "from_speaker is required"),
            (refusal(mem.relabel_speaker("Dana", " \u{feff}", "u", None).await), "to_speaker is required"),
            (refusal(mem.forget_speaker(["Dana", "\u{feff}"], "u", true).await), "speakers[1] is required"),
            (refusal(mem.by_speaker("\u{feff}", "u", None, None, None).await), "speaker is required"),
            (refusal(mem.add_turn_opts("q", "a", "u", &late).await), "created_at must be RFC3339"),
        ] {
            assert!(m.contains(want), "want {want:?}, got {m}");
        }
        assert_eq!(count(&seen), 5);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_segment_is_blank_by_the_whitespace_python_and_typescript_trim() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        let blank = segments(vec![segment("y", None, None), segment("\u{feff} ", None, None)]);
        let m = refusal(mem.add_bulk_opts(None, "u", "chat", &blank).await);
        assert!(m.contains("segments[1].content is required"), "{m}");
        assert_eq!(count(&seen), 0);
        mem.add_bulk_opts(None, "u", "chat", &segments(vec![segment("\u{85}", None, None)])).await.unwrap();
        assert_eq!(seen.lock().unwrap()[0].body["segments"], json!([{"content": "\u{85}"}]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn seen_and_clear_window_go_to_their_routes() {
        let lines = r#"{"line":{"role":"user"},"seen":[{"id":"a","status":"current"},7],"count":2}"#;
        let (base, seen) = scripted(vec![(200, lines), (200, r#"{"status":"ok","cleared":3,"kept":0}"#)]);
        let mem = Client::with_base_url(KEY, &base);
        let r = mem.seen(" 2026-10-19T09:00:00.123Z ", "u").await.unwrap();
        assert_eq!(r["seen"].as_array().unwrap().len(), 1, "a row that is not an object is dropped");
        assert_eq!(mem.clear_window("u", None).await.unwrap()["cleared"], 3);
        mem.clear_window("u", "2026-10-19T10:00:00Z").await.unwrap();
        assert!(refusal(mem.seen("  ", "u").await).contains("at is required"));
        assert!(refusal(mem.clear_window("u", " ").await).contains("up_to is required"));
        let seen = seen.lock().unwrap();
        let paths: Vec<&str> = seen.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, ["/api/v1/memory/seen", "/api/v1/memory/clear-window", "/api/v1/memory/clear-window"]);
        assert_eq!(seen[0].body, json!({"user_id": "u", "at": "2026-10-19T09:00:00.123Z"}));
        assert_eq!(seen[1].body, json!({"user_id": "u"}));
        assert_eq!(seen[2].body, json!({"user_id": "u", "up_to": "2026-10-19T10:00:00Z"}));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_sleep_goes_on_by_its_id_and_its_evidence_is_checked() {
        let (base, seen) = scripted(vec![(200, r#"{"memories":[],"sleep_id":"s-1"}"#), (200, r#"{"ok":true,"unknown":[]}"#)]);
        let mem = Client::with_base_url(KEY, &base).with_tz(9.0).unwrap();
        let next = SleepReadOpts { sleep_id: Some(" s-1 ".into()), days_ago: Some(1), ..Default::default() };
        mem.sleep_read("u", &next).await.unwrap();
        assert_eq!(mem.sleep_verify("s-1", ["a", "b"], "u").await.unwrap()["ok"], true);
        mem.sleep_verify("s-1", Vec::<String>::new(), "u").await.unwrap();
        mem.sleep_verify("s-1", vec!["\u{e9}".repeat(64); 200], "u").await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].body, json!({"user_id": "u", "tz_offset_minutes": 540, "days_ago": 1, "sleep_id": "s-1"}));
        assert_eq!(seen[1].path, "/api/v1/memory/sleep/verify");
        assert_eq!(seen[1].body, json!({"user_id": "u", "sleep_id": "s-1", "evidence": ["a", "b"]}));
        assert_eq!(seen[2].body, json!({"user_id": "u", "sleep_id": "s-1", "evidence": []}), "no evidence is sent too");
        assert_eq!(seen[3].body["evidence"].as_array().unwrap().len(), 200);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sleep_evidence_out_of_bounds_is_refused_before_sending() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base).with_tz(9.0).unwrap();
        let long = "x".repeat(65);
        let blank_id = SleepReadOpts { sleep_id: Some(String::new()), ..Default::default() };
        for (m, want) in [
            (refusal(mem.sleep_verify("s-1", vec!["a"; 201], "u").await), "evidence must hold between 0 and 200 entries, got 201"),
            (refusal(mem.sleep_verify("s-1", ["a", ""], "u").await), "evidence[1] must be between 1 and 64 characters, got 0"),
            (refusal(mem.sleep_verify("s-1", [long.as_str()], "u").await), "evidence[0] must be between 1 and 64 characters, got 65"),
            (refusal(mem.sleep_verify(" ", ["a"], "u").await), "sleep_id is required"),
            (refusal(mem.sleep_read("u", &blank_id).await), "sleep_id is required"),
        ] {
            assert!(m.contains(want), "want {want:?}, got {m}");
        }
        assert_eq!(count(&seen), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_new_reads_are_sent_again_after_a_503_and_clearing_the_window_is_not() {
        for path in ["/api/v1/memory/sleep/verify", "/api/v1/memory/seen", "/api/v1/memory/channel/read"] {
            assert!(READ_POSTS.contains(&path), "{path}");
        }
        let busy = || http("503 Service Unavailable", "Retry-After: 0\r\n", "{}");
        let (base, hits) = mock_server(vec![busy(), http("200 OK", "", r#"{"seen":[]}"#)]);
        Client::with_base_url(KEY, &base).seen("2026-10-19T09:00:00Z", "u").await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        let (base, hits) = mock_server(vec![busy(), http("200 OK", "", r#"{"ok":true}"#)]);
        Client::with_base_url(KEY, &base).sleep_verify("s-1", ["a"], "u").await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        let (base, hits) = mock_server(vec![busy(), http("200 OK", "", "{}")]);
        let e = Client::with_base_url(KEY, &base).clear_window("u", None).await.unwrap_err();
        assert_eq!(e.status(), Some(503));
        assert_eq!(hits.load(Ordering::SeqCst), 1, "a write is not sent again");
    }

    fn segment(content: &str, speaker: Option<&str>, created_at: Option<&str>) -> Segment {
        Segment { content: content.into(), speaker: speaker.map(String::from), created_at: created_at.map(String::from) }
    }

    fn segments(list: Vec<Segment>) -> BulkOpts {
        BulkOpts { segments: Some(list), ..Default::default() }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_bulk_opts_sends_segments_or_one_content_with_its_speaker() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        let chat = segments(vec![
            segment("I moved to Valencia.", Some("Ana"), Some(" 2024-03-01T10:00:00Z ")),
            segment("How is it?", None, None),
        ]);
        mem.add_bulk_opts(None, "u", "chat", &chat).await.unwrap();
        let spoken = BulkOpts { speaker: Some("Ana".into()), ..Default::default() };
        mem.add_bulk_opts("a long note", "u", "notes", &spoken).await.unwrap();
        mem.add_bulk_opts("plain", None, "notes", &BulkOpts::default()).await.unwrap();
        {
            let seen = seen.lock().unwrap();
            assert!(seen.iter().all(|s| s.path == "/api/v1/memory/bulk-store"));
            assert_eq!(
                seen[0].body,
                json!({"user_id": "u", "category": "chat", "segments": [
                    {"content": "I moved to Valencia.", "speaker": "Ana", "created_at": "2024-03-01T10:00:00Z"},
                    {"content": "How is it?"}
                ]}),
                "no content beside the segments"
            );
            assert_eq!(
                seen[1].body,
                json!({"user_id": "u", "category": "notes", "content": "a long note", "metadata": {"speaker": "Ana"}})
            );
            assert_eq!(seen[2].body, json!({"user_id": "default", "category": "notes", "content": "plain"}));
        }
        let (base, raw) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        Client::with_base_url(KEY, &base).add_bulk_opts(None, "u", "chat", &chat).await.unwrap();
        assert!(raw.lock().unwrap()[0].to_ascii_lowercase().contains("idempotency-key: sdk-"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_bulk_opts_refuses_both_neither_or_a_blank_segment_before_sending() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        let spoken = BulkOpts { speaker: Some("Ana".into()), ..segments(vec![segment("x", None, None)]) };
        for (m, want) in [
            (refusal(mem.add_bulk_opts("x", "u", "chat", &segments(vec![segment("y", None, None)])).await), "content and segments are exclusive"),
            (refusal(mem.add_bulk_opts(None, "u", "chat", &BulkOpts::default()).await), "needs content or segments"),
            (refusal(mem.add_bulk_opts(None, "u", "chat", &segments(vec![])).await), "at least one segment"),
            (
                refusal(mem.add_bulk_opts(None, "u", "chat", &segments(vec![segment("x", None, None), segment(" ", None, None)])).await),
                "segments[1].content is required",
            ),
            (
                refusal(mem.add_bulk_opts(None, "u", "chat", &segments(vec![segment("x", None, Some(""))])).await),
                "segments[0].created_at is required",
            ),
            (refusal(mem.add_bulk_opts(None, "u", "chat", &spoken).await), "each segment names its own speaker"),
        ] {
            assert!(m.contains(want), "want {want:?}, got {m}");
        }
        assert_eq!(count(&seen), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_bulk_opts_dates_the_backfill_and_trims_each_speaker() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        let dated = BulkOpts { timestamp: Some("2024-03-01T10:00:00Z".into()), speaker: Some(" Ana ".into()), ..Default::default() };
        mem.add_bulk_opts("a long note", "u", "notes", &dated).await.unwrap();
        let chat = BulkOpts { timestamp: Some("2024-03-01".into()), ..segments(vec![segment("hi", Some(" Bob "), None)]) };
        mem.add_bulk_opts(None, "u", "chat", &chat).await.unwrap();
        let blank = BulkOpts { timestamp: Some(String::new()), ..Default::default() };
        mem.add_bulk_opts("x", "u", "notes", &blank).await.unwrap();
        let long = "a".repeat(257);
        for (m, want) in [
            (refusal(mem.add_bulk_opts(None, "u", "chat", &segments(vec![segment("x", Some(" "), None)])).await), "segments[0].speaker is required"),
            (refusal(mem.add_bulk_opts(None, "u", "chat", &segments(vec![segment("x", Some(&long), None)])).await), "at most 256 bytes"),
            (
                refusal(mem.add_bulk_opts("x", "u", "chat", &BulkOpts { speaker: Some("".into()), ..Default::default() }).await),
                "speaker is required",
            ),
        ] {
            assert!(m.contains(want), "want {want:?}, got {m}");
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert_eq!(
            seen[0].body,
            json!({"user_id": "u", "category": "notes", "content": "a long note", "metadata": {"speaker": "Ana"}, "timestamp": "2024-03-01T10:00:00Z"})
        );
        assert_eq!(seen[1].body["timestamp"], "2024-03-01", "a plain date goes as it is");
        assert_eq!(seen[1].body["segments"], json!([{"content": "hi", "speaker": "Bob"}]));
        assert!(seen[2].body.get("timestamp").is_none(), "an empty timestamp is not sent");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_turn_opts_sends_when_it_was_said_and_who_said_each_half() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        let turn = WriteOpts {
            created_at: Some(" 2024-03-01T11:30:00+09:00 ".into()),
            user_speaker: Some(" Alice ".into()),
            assistant_speaker: Some("Wren".into()),
            ..Default::default()
        };
        mem.add_turn_opts("Lunch at noon?", "Noon works.", "u", &turn).await.unwrap();
        let dated = WriteOpts { created_at: Some("2024-03-01T02:30:00Z".into()), ..Default::default() };
        mem.add_turn_opts("q", "a", "u", &dated).await.unwrap();
        mem.add_turn("q", "a", "u").await.unwrap();
        {
            let seen = seen.lock().unwrap();
            assert!(seen.iter().all(|s| s.path == "/api/v1/memory/store-turn"));
            assert_eq!(
                seen[0].body,
                json!({
                    "user_id": "u", "user_msg": "Lunch at noon?", "assistant_msg": "Noon works.",
                    "created_at": "2024-03-01T11:30:00+09:00", "user_speaker": "Alice", "assistant_speaker": "Wren",
                })
            );
            assert_eq!(seen[1].body["created_at"], "2024-03-01T02:30:00Z");
            for k in ["created_at", "user_speaker", "assistant_speaker"] {
                assert!(seen[2].body.get(k).is_none(), "{k} is not sent unless set");
            }
        }
        let opts = |f: fn(&mut WriteOpts)| {
            let mut o = WriteOpts::default();
            f(&mut o);
            o
        };
        for (o, want) in [
            (opts(|o| o.user_speaker = Some("  ".into())), "user_speaker is required"),
            (opts(|o| o.assistant_speaker = Some("é".repeat(129))), "assistant_speaker is at most 256 bytes"),
            (opts(|o| o.created_at = Some("".into())), "created_at is required"),
            (opts(|o| o.created_at = Some("yesterday".into())), "created_at must be RFC3339"),
            (opts(|o| o.created_at = Some("2024-02-30".into())), "created_at must be RFC3339"),
            (opts(|o| o.created_at = Some("2024-03-01T25:00:00Z".into())), "created_at must be RFC3339"),
            (opts(|o| o.created_at = Some("2024-03-01".into())), "a plain date or a time with no offset is refused"),
            (opts(|o| o.created_at = Some(" 2024-03-01 ".into())), "created_at must be RFC3339 with an offset"),
            (opts(|o| o.created_at = Some("2024-03-01T10:00:00".into())), "created_at must be RFC3339 with an offset"),
            (opts(|o| o.created_at = Some("2024-03-01T10:00".into())), "created_at must be RFC3339"),
            (opts(|o| o.created_at = Some("2024-03-01 10:00:00+00:00".into())), "a space for the T"),
            (opts(|o| o.created_at = Some("2024-03-01 10:00:00Z".into())), "a space for the T"),
            (opts(|o| o.created_at = Some("2024-03-01t10:00:00Z".into())), "with a T between date and time"),
            (opts(|o| o.created_at = Some("2024-03-01T10:00:00z".into())), "created_at must be RFC3339"),
            (opts(|o| o.created_at = Some("2024-03-01T23:59:60Z".into())), "created_at must be RFC3339"),
            (opts(|o| o.created_at = Some("2024-03-01T19:00:00\u{2212}05:00".into())), "created_at must be RFC3339"),
            (opts(|o| o.created_at = Some("0000-01-01T00:00:00Z".into())), "created_at must be RFC3339"),
            (opts(|o| o.created_at = Some("2024-03-01T10:00:00+24:00".into())), "created_at must be RFC3339"),
        ] {
            let m = refusal(mem.add_turn_opts("q", "a", "u", &o).await);
            assert!(m.contains(want), "want {want:?}, got {m}");
        }
        mem.add_turn_opts("q", "a", "u", &opts(|o| o.user_speaker = Some("é".repeat(128)))).await.unwrap();
        let named = opts(|o| o.user_speaker = Some("Alice".into()));
        assert!(refusal(mem.add_opts("x", "u", json!({}), &named).await).contains("user_speaker is for add_turn_opts"));
        let dated = opts(|o| o.created_at = Some("2024-03-01".into()));
        assert!(refusal(mem.update_opts("id", "x", "u", &dated).await).contains("created_at is for add_turn_opts"));
        assert_eq!(count(&seen), 4);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn names_and_ids_are_trimmed_as_javascript_trims_them() {
        let (base, seen) = scripted(vec![]);
        let mem = Client::with_base_url(KEY, &base);
        let turn = WriteOpts {
            user_speaker: Some("\u{feff}Ana\u{3000}".into()),
            assistant_speaker: Some("Wren\u{3000}".into()),
            ..Default::default()
        };
        mem.add_turn_opts("q", "a", "u", &turn).await.unwrap();
        mem.add_bulk_opts(None, "u", "chat", &segments(vec![segment("hi", Some("\u{2028}Bob\u{feff}"), None)])).await.unwrap();
        mem.relabel_speaker("\u{feff}Ana", "Bob\u{3000}", "u", None).await.unwrap();
        let nel = WriteOpts { assistant_speaker: Some("Wren\u{85}".into()), ..Default::default() };
        assert!(refusal(mem.add_turn_opts("q", "a", "u", &nel).await).contains("assistant_speaker contains a line break"));
        assert!(refusal(mem.relabel_speaker("Ana", "Bob\u{85}", "u", None).await).contains("to_speaker contains a line break"));
        mem.get("u", "\u{feff}m-1\u{85}").await.unwrap();
        mem.add_with("x", "u", json!({}), json!({"image": {"data": "\u{feff}QU JD\u{85}"}})).await.unwrap();
        assert!(refusal(mem.get("\u{feff}", "m-1").await).contains("blank"));
        {
            let seen = seen.lock().unwrap();
            assert_eq!((&seen[0].body["user_speaker"], &seen[0].body["assistant_speaker"]), (&json!("Ana"), &json!("Wren")));
            assert_eq!(seen[1].body["segments"][0]["speaker"], "Bob");
            assert_eq!((&seen[2].body["from"], &seen[2].body["to"]), (&json!("Ana"), &json!("Bob")));
            assert_eq!((&seen[3].body["user_id"], &seen[3].body["memory_id"]), (&json!("u"), &json!("m-1\u{85}")));
            assert_eq!(seen[4].body["image"]["data"], "QUJD\u{85}");
        }
        assert_eq!(count(&seen), 5);
        let tools = || mem.memory_tools_preset("u", None, "S-2").unwrap();
        assert_eq!(tools().with_assistant("\u{feff}Wren").unwrap().assistant.as_deref(), Some("Wren"));
        assert!(tools().with_assistant("Wren\u{85}").unwrap_err().to_string().contains("control character"));
    }
