    use super::tests::{http, mock_server, mock_server_recording};
    use super::{Client, ErrorKind, WosError};
    use std::sync::atomic::Ordering;

    fn closed_port() -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let a = l.local_addr().unwrap();
        drop(l);
        format!("http://{a}")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_network_error_never_carries_the_url_or_its_query() {
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &closed_port()).with_retries(0);
        let e = mem.list_speakers("alice.customer@example.com").await.unwrap_err();
        assert!(matches!(e, WosError::Network(_)), "got {e:?}");
        for shown in [format!("{e}"), format!("{e:?}")] {
            assert!(!shown.contains("alice"), "the store id leaked: {shown}");
            assert!(!shown.contains("user_id="), "the query leaked: {shown}");
            assert!(!shown.contains("/api/v1"), "the path leaked: {shown}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_network_error_names_its_cause_and_keeps_its_source() {
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &closed_port()).with_retries(0);
        let e = mem.list_stores().await.unwrap_err();
        assert!(format!("{e}").contains("connection refused"), "got {e}");
        assert!(std::error::Error::source(&e).is_some(), "the transport error must be reachable");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_invalid_base_url_is_a_bad_request_and_is_not_retried() {
        for bad in ["not a url", "ftp://example.com", "http://", "https://"] {
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", bad);
            let t0 = std::time::Instant::now();
            let e = mem.list_stores().await.unwrap_err();
            assert_eq!(e.kind(), ErrorKind::BadRequest, "{bad}: got {e:?}");
            assert!(format!("{e}").contains("invalid base_url"), "{bad}: got {e}");
            assert!(t0.elapsed().as_secs_f64() < 0.3, "{bad}: retried for {:?}", t0.elapsed());
            let e = mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await.unwrap_err();
            assert_eq!(e.kind(), ErrorKind::BadRequest, "{bad} get_image: got {e:?}");
            assert!(t0.elapsed().as_secs_f64() < 0.3, "{bad}: get_image retried for {:?}", t0.elapsed());
        }
    }

    #[test]
    fn a_refused_body_is_a_bad_request() {
        for status in [400u16, 413, 422] {
            let e = WosError::Api { status, message: "x".into() };
            assert_eq!(e.kind(), ErrorKind::BadRequest, "{status}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_413_from_the_service_is_a_bad_request() {
        let (base, _) = mock_server(vec![http(
            "413 Payload Too Large",
            "",
            "{\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"Request body too large (max 10MB)\"}}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::BadRequest);
        assert!(format!("{e}").contains("10MB"), "got {e}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_object_valued_message_falls_back_to_the_type() {
        let (base, _) = mock_server(vec![http(
            "404 Not Found",
            "",
            "{\"type\":\"error\",\"error\":{\"type\":\"not_found_error\",\"message\":{\"text\":\"x\"},\"request_id\":\"r-9\"}}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem.stats("alice").await.unwrap_err();
        let shown = format!("{e}");
        assert!(shown.contains("not_found_error") && shown.contains("r-9"), "got {shown}");
        assert!(!shown.contains("{\"text\""), "got {shown}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_image_errors_carry_the_request_id() {
        let (base, _) = mock_server(vec![http(
            "404 Not Found",
            "",
            "{\"type\":\"error\",\"error\":{\"type\":\"not_found_error\",\"message\":\"Not found.\",\"request_id\":\"req_img\"}}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotFound);
        assert!(format!("{e}").contains("req_img"), "got {e}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_retired_model_is_gone_says_what_to_do_and_is_not_retried() {
        let body = "{\"type\":\"error\",\"error\":{\"type\":\"gone_error\",\"message\":\"Scroll 1 now exists only in memory.\",\"code\":0,\"request_id\":\"req_410\"}}";
        let (base, hits) = mock_server(vec![
            http("410 Gone", "", body),
            http("410 Gone", "", body),
            http("410 Gone", "", body),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem.search("q", "alice", 10).await.unwrap_err();
        assert!(e.is_gone(), "got {e}");
        assert!(!e.is_auth());
        assert_eq!(e.kind(), ErrorKind::Other);
        let shown = format!("{e}");
        assert!(shown.contains("Scroll 1 now exists only in memory."), "{shown}");
        assert!(shown.contains("list_models()"), "{shown}");
        assert!(shown.ends_with("(request_id: req_410)"), "{shown}");
        assert_eq!(hits.load(Ordering::SeqCst), 1, "a 410 is not retried");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_sleep_too_old_to_finish_is_its_own_410_and_codes_are_kept() {
        let expired = "{\"error\":{\"code\":\"sleep_expired\",\"message\":\"This sleep plan is too old to finish.\",\"request_id\":\"req_x\"}}";
        let conflict = "{\"error\":{\"code\":\"self_version_conflict\",\"message\":\"the file changed since it was read\"}}";
        let retired = "{\"error\":{\"code\":\"model_retired\",\"message\":\"Scroll 1 now exists only in memory.\"}}";
        let (base, hits) = mock_server(vec![
            http("410 Gone", "", expired),
            http("409 Conflict", "", conflict),
            http("410 Gone", "", retired),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem.sleep_done("s-1", "alice").await.unwrap_err();
        assert_eq!((e.status(), e.code()), (Some(410), Some("sleep_expired")));
        assert!(!e.is_gone(), "an expired sleep plan is not a retired model");
        assert_eq!(format!("{e}"), "[410] This sleep plan is too old to finish. (request_id: req_x)");
        let e = mem.sleep_files("alice").await.unwrap_err();
        assert_eq!((e.status(), e.code(), e.kind()), (Some(409), Some("self_version_conflict"), ErrorKind::Conflict));
        assert_eq!(format!("{e}"), "[409] the file changed since it was read");
        let e = mem.search("q", "alice", 10).await.unwrap_err();
        assert_eq!(e.code(), Some("model_retired"));
        assert!(e.is_gone() && format!("{e}").contains("This model is retired"), "{e}");
        assert!(matches!(&e, WosError::Api { status: 410, .. }) && e.code() == Some("model_retired"));
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        assert_eq!(crate::refused("no").code(), None, "a refusal made here carries no code");
    }

    #[test]
    fn code_and_conflicts_with_ride_at_the_end_of_the_message() {
        let e = WosError::Api { status: 409, message: "collides (request_id: r1) [code: store_id_conflict] [conflicts_with: team-a]".into() };
        let WosError::Api { status, message } = &e else { unreachable!() };
        assert_eq!((*status, e.code(), e.conflicts_with()), (409, Some("store_id_conflict"), Some("team-a")));
        assert!(message.ends_with(" [code: store_id_conflict] [conflicts_with: team-a]"));
        assert_eq!(format!("{e}"), "[409] collides (request_id: r1)", "Display leaves the tags out");
        let only = |m: &str| {
            let e = WosError::Api { status: 409, message: m.into() };
            (e.code().map(str::to_string), e.conflicts_with().map(str::to_string), e.to_string())
        };
        assert_eq!(only("x [code: c]"), (Some("c".into()), None, "[409] x".into()));
        assert_eq!(only("x [conflicts_with: s]"), (None, Some("s".into()), "[409] x".into()));
        for m in ["x", "x [code: ]", "x [code: a b]", "x [code: c] trailing", "x [conflicts_with: s] [code: c]x", "[code: c]"] {
            let (code, with, _) = only(m);
            assert!(code.is_none() && with.is_none(), "{m:?} carries nothing");
        }
        assert_eq!(only("x [conflicts_with: s] [code: c]"), (Some("c".into()), None, "[409] x [conflicts_with: s]".into()));
        let e = crate::api_error(409, "x".into(), [Some("a b"), Some("team-a"), None, None]);
        assert_eq!((e.code(), e.conflicts_with()), (None, Some("team-a")));
        let e = crate::api_error(409, "said [code: fake]".into(), [None; 4]);
        assert_eq!((e.code(), e.to_string().as_str()), (None, "[409] said [code: fake]."));
        let e = crate::api_error(410, "gone".into(), [Some("sleep_expired"), None, None, None]);
        assert_eq!((e.code(), e.conflicts_with(), e.to_string().as_str()), (Some("sleep_expired"), None, "[410] gone"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_empty_model_name_means_the_default_and_listings_have_their_lists() {
        let (base, reqs) = mock_server_recording(vec![
            http("200 OK", "", "{\"speakers\":[null,1,{\"speaker\":\"Bob\"}]}"),
            http("200 OK", "", "{\"note\":\"no engrams on this model\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_model("");
        let s = mem.list_speakers("alice").await.unwrap();
        assert_eq!(s["speakers"], serde_json::json!([{"speaker": "Bob"}]));
        let e = mem.list_engrams().await.unwrap();
        assert_eq!(e["engrams"], serde_json::json!([]));
        assert_eq!(e["forms"], serde_json::json!([]));
        assert_eq!(e["note"], "no engrams on this model");
        let first = reqs.lock().unwrap()[0].to_lowercase();
        assert!(!first.contains("x-wos-model"), "the service's default answers: {first}");
        let spaced = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_model(" ");
        assert!(spaced.list_speakers("alice").await.is_err(), "whitespace is not the default");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_page_calls_hand_back_their_lists_and_a_last_page_says_null() {
        let (base, _) = mock_server(vec![
            http("200 OK", "", "{}"),
            http("200 OK", "", "{}"),
            http("200 OK", "", "{}"),
            http("200 OK", "", "{}"),
            http("200 OK", "", "{\"memories\":[{\"id\":\"a\"},7,null],\"count\":3,\"next_cursor\":\"C\"}"),
            http("200 OK", "", "{\"memories\":[{\"id\":\"b\"}],\"next_cursor\":\"\"}"),
            http("200 OK", "", "{\"images\":[{\"id\":\"i\"},\"x\"],\"count\":9}"),
            http("200 OK", "", "{\"chain\":[{\"memory_id\":\"c\"},[1]]}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let empty = serde_json::json!({"memories": [], "count": 0, "next_cursor": null});
        assert_eq!(mem.list_memories("u", None, None).await.unwrap(), empty);
        assert_eq!(mem.list_images("u", None, None, None).await.unwrap(), serde_json::json!({"images": []}));
        assert_eq!(mem.by_speaker("Dana", "u", None, None, None).await.unwrap(), serde_json::json!({"memories": []}));
        assert_eq!(mem.lineage("u", "m1").await.unwrap(), serde_json::json!({"chain": []}));
        let page = mem.list_memories("u", None, None).await.unwrap();
        assert_eq!(page, serde_json::json!({"memories": [{"id": "a"}], "count": 3, "next_cursor": "C"}));
        let last = mem.list_memories("u", None, "C").await.unwrap();
        assert_eq!(last, serde_json::json!({"memories": [{"id": "b"}], "count": 1, "next_cursor": null}));
        let images = mem.list_images("u", None, None, None).await.unwrap();
        assert_eq!(images, serde_json::json!({"images": [{"id": "i"}], "count": 9}));
        assert_eq!(mem.lineage("u", "c").await.unwrap(), serde_json::json!({"chain": [{"memory_id": "c"}]}));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn revisions_include_is_checked_before_sending() {
        let (base, hits) = mock_server(vec![]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem.revisions_page("alice", "both", None::<usize>, None::<&str>, None).await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::BadRequest);
        assert!(format!("{e}").contains("include must be \"revised\" or \"unrevised\""), "{e}");
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing may reach the network");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn control_characters_from_the_server_never_reach_the_message() {
        let hostile = "{\"error\":{\"message\":\"boom\\u001b[31mRED\\r\\nInjected: line\\u0000\\u007f\\t!\"}}";
        let raw = "oops\u{1b}[2J\r\nfake log line";
        let (base, _) = mock_server(vec![
            http("500 Internal Server Error", "", hostile),
            http("500 Internal Server Error", "", raw),
            http("500 Internal Server Error", "", hostile),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let a = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        let b = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        let c = mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await.unwrap_err();
        for e in [a, b, c] {
            let shown = format!("{e}");
            assert!(
                !shown.chars().any(|ch| (ch as u32) < 0x20 || ch == '\u{7f}'),
                "a control character survived: {shown:?}"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_image_reports_why_it_could_not_read_an_error_body() {
        let (base, _) = mock_server(vec![format!(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{{}}",
            70 * 1024 * 1024
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await {
            Err(WosError::Api { status: 500, message, .. }) => {
                assert!(message.contains("too large"), "got {message:?}")
            }
            other => panic!("expected a 500 that says why, got {other:?}"),
        }
    }

    #[test]
    fn debug_hides_credentials_in_the_base_url() {
        for base in [
            "https://user:hunter2pass@proxy.example.com",
            "https://hunter2pass@proxy.example.com/prefix",
            "http://u:hunter2pass@[::1]:8080",
            "http://u:hunter2pass@proxy.example.com:notaport",
        ] {
            let dbg = format!("{:?}", Client::with_base_url("wos-test-xxxxxxxxxx", base));
            assert!(!dbg.contains("hunter2pass"), "{base} leaked into {dbg}");
        }
        let dbg = format!("{:?}", Client::with_base_url("wos-test-xxxxxxxxxx", "https://user:pw@proxy.example.com"));
        assert!(dbg.contains("proxy.example.com"), "the host is still useful: {dbg}");
    }

    #[test]
    fn debug_hides_credentials_the_url_parser_does_not_read_as_such() {
        for (base, shown) in [
            ("https://user:s3/cr3t@api.example.com", "https://***@api.example.com"),
            ("https://user:s3cr3t#x@api.example.com", "https://***@api.example.com"),
            ("https://user:s3?cr3t@api.example.com", "https://***@api.example.com"),
            ("user:s3cr3t@api.example.com", "***@api.example.com"),
            ("user:cr3t://x@api.example.com", "***@api.example.com"),
        ] {
            let dbg = format!("{:?}", Client::with_base_url("wos-test-xxxxxxxxxx", base));
            assert!(!dbg.contains("cr3t"), "{base} leaked into {dbg}");
            assert!(dbg.contains(&format!("{shown:?}")), "{base}: got {dbg}");
        }
        for (base, shown) in [
            ("https://user:123/cr3t@api.example.com", "https://user:123/cr3t@api.example.com"),
            ("https://user:p@ss/cr3t@api.example.com", "https://***@ss/cr3t@api.example.com"),
        ] {
            let dbg = format!("{:?}", Client::with_base_url("wos-test-xxxxxxxxxx", base));
            assert!(dbg.contains(&format!("{shown:?}")), "a base the client can use shows the host it sends to: {base}: got {dbg}");
        }
        for base in ["https://api.example.com", "http://127.0.0.1:9/prefix", "not a url"] {
            let dbg = format!("{:?}", Client::with_base_url("wos-test-xxxxxxxxxx", base));
            assert!(dbg.contains(&format!("{base:?}")), "{base}: got {dbg}");
        }
    }

    #[test]
    fn a_base_url_with_no_host_is_refused_before_sending() {
        for bad in ["http://", "https://", "http:", "https:///", "http:/example.net:9", "http:example.net", "HTTPS:api.wontopos.com", "https:///api.wontopos.com", "http:127.0.0.1:9"] {
            match Client::with_base_url("wos-test-xxxxxxxxxx", bad).preflight() {
                Err(WosError::Api { status: 400, message, .. }) => {
                    assert!(message.contains("invalid base_url"), "{bad}: got {message}")
                }
                other => panic!("{bad} must be refused before sending, got {other:?}"),
            }
        }
        for good in ["https://api.wontopos.com", "http://127.0.0.1:9", "https://proxy.example.com/wos/"] {
            assert!(Client::with_base_url("wos-test-xxxxxxxxxx", good).preflight().is_ok(), "{good}");
        }
    }

    #[test]
    fn a_base_url_the_parser_would_reread_is_refused_and_its_ends_are_trimmed() {
        for bad in ["https:/\\evil.example", "http://evil.example\\@127.0.0.1", "https://api.won\ttopos.com", "https://a.example/x y", "https://a.example/\u{0}"] {
            match Client::with_base_url("wos-test-xxxxxxxxxx", bad).preflight() {
                Err(WosError::Api { status: 400, message, .. }) => assert!(message.contains("backslash"), "{bad:?}: got {message}"),
                other => panic!("{bad:?} must be refused before sending, got {other:?}"),
            }
        }
        for padded in ["https://api.wontopos.com\n", " https://api.wontopos.com/ ", "\thttps://api.wontopos.com\r\n"] {
            let c = Client::with_base_url("wos-test-xxxxxxxxxx", padded);
            assert!(c.preflight().is_ok(), "{padded:?}");
            assert_eq!(c.base_url, "https://api.wontopos.com", "{padded:?}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_invalid_base_url_error_does_not_show_its_credentials() {
        for bad in ["https://u:hunter2@host:99999", "u:hunter2@host", "https://u:hunter2@", "ftp://u:hunter2@host"] {
            let e = Client::with_base_url("wos-test-xxxxxxxxxx", bad).list_stores().await.unwrap_err();
            assert!(format!("{e}").contains("invalid base_url"), "{bad}: got {e}");
            assert!(!format!("{e} {e:?}").contains("hunter2"), "{bad} leaked into {e:?}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn new_error_messages_use_plain_punctuation() {
        let e = Client::with_base_url("wos-test-xxxxxxxxxx", "not a url").list_stores().await.unwrap_err();
        assert!(format!("{e}").contains("invalid base_url"), "got {e}");
        assert!(!format!("{e}").contains('\u{2014}'), "got {e}");
        assert_eq!(
            format!("{}", super::page_ceiling()),
            "[200] stopped after 20000 pages \u{2014} the store did not end. This is a truncated answer, not the whole store."
        );
        assert_eq!(
            format!("{}", super::repeated_cursor()),
            "[200] the service handed back a cursor it had already given, so the store did not end. \
             This is a truncated answer, not the whole store."
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_image_checks_the_model_name_before_sending() {
        let (base, hits) = mock_server(vec![]);
        for bad in ["bad model", "bad\r\nX-Evil: 1"] {
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_model(bad);
            match mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await {
                Err(WosError::Api { status: 400, message, .. }) => {
                    assert!(message.contains("invalid model name"), "{bad:?}: got {message}")
                }
                other => panic!("{bad:?} should be refused locally, got {other:?}"),
            }
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing may reach the network");
    }

    #[test]
    fn an_empty_message_falls_back_to_the_type_and_keeps_the_request_id() {
        let (m, rid) = super::parse_error(
            "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"\",\"request_id\":\"req_e\"}}",
        );
        assert_eq!(m, "invalid_request_error");
        assert_eq!(rid.as_deref(), Some("req_e"));

        let raw = "{\"error\":{\"message\":{\"a\":1},\"request_id\":\"req_o\"}}";
        let (m, rid) = super::parse_error(raw);
        assert_eq!(m, raw);
        assert_eq!(rid.as_deref(), Some("req_o"));

        let (_, rid) = super::parse_error("{\"error\":{\"type\":\"t\",\"message\":\"m\",\"request_id\":\"\"}}");
        assert_eq!(rid, None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_error_with_nothing_to_say_names_its_status() {
        let (base, _) = mock_server(vec![
            http("502 Bad Gateway", "", ""),
            http("400 Bad Request", "", "{\"error\":\"\"}"),
            http("400 Bad Request", "", "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"\",\"request_id\":\"req_e\"}}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_retries(0);
        let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(format!("{e}"), "[502] HTTP 502");
        let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(format!("{e}"), "[400] HTTP 400");
        let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(format!("{e}"), "[400] invalid_request_error (request_id: req_e)");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_blank_error_field_is_skipped_and_a_top_level_message_is_read() {
        let cases = [
            (
                "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"\",\"request_id\":\"req_e\"}}",
                "[400] invalid_request_error (request_id: req_e)",
            ),
            ("{\"error\":\"\"}", "[400] HTTP 400"),
            ("{\"error\":{\"type\":\"t\",\"message\":\"  \",\"request_id\":\"  \"}}", "[400] t"),
            ("{\"error\":\"  \"}", "[400] HTTP 400"),
            ("  ", "[400] HTTP 400"),
            ("{\"message\":\"m\"}", "[400] m"),
            ("{\"message\":\"\"}", "[400] {\"message\":\"\"}"),
        ];
        let (base, _) = mock_server(cases.iter().map(|(body, _)| http("400 Bad Request", "", body)).collect());
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_retries(0);
        for (body, want) in cases {
            let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
            assert_eq!(format!("{e}"), want, "{body}");
        }
    }
