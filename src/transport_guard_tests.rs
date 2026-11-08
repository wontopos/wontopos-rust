    use super::*;

    const KEY: &str = "wos-test-xxxxxxxxxx";
    const LLM_KEY: &str = "sk-ant-test-guards-000000000000000000";

    fn refusal(r: Result<impl std::fmt::Debug, WosError>) -> String {
        match r {
            Err(WosError::Api { status: 400, message, .. }) => message,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_base_url_with_a_query_or_an_anchor_is_refused_without_printing_it() {
        for bad in ["http://127.0.0.1:1/?region=eu", "http://127.0.0.1:1#top", "http://127.0.0.1:1/wos?token=abc123", "http://127.0.0.1:1?"] {
            let message = refusal(Client::with_base_url(KEY, bad).with_retries(0).stats(None).await);
            assert!(message.starts_with("invalid base_url (it has a ? query or a # anchor)"), "{bad}: {message}");
            assert!(!message.contains("abc123") && !message.contains("region"), "{message}");
        }
    }

    #[test]
    fn a_provider_base_with_a_query_or_an_anchor_is_refused_without_printing_it() {
        for bad in ["https://api.example.com/v1?token=abc123", "https://api.example.com/v1#x", "https://api.example.com?"] {
            let made = [
                anthropic_caller(AnthropicCallerOpts { api_key: LLM_KEY.into(), base_url: Some(bad.into()), ..Default::default() }).map(|_| ()),
                openai_caller(OpenAiCallerOpts { base_url: bad.into(), model: "m".into(), ..Default::default() }).map(|_| ()),
                Client::new(KEY)
                    .with_router(RouterOptions { key: Some(LLM_KEY.into()), provider_base_url: Some(bad.into()), ..Default::default() })
                    .map(|_| ()),
            ];
            for r in made {
                let message = refusal(r);
                assert!(message.starts_with("the model provider's base URL has a ? query or a # anchor"), "{bad}: {message}");
                assert!(!message.contains("abc123"), "{message}");
            }
        }
    }

    struct AtBase(String);

    impl ModelCaller for AtBase {
        fn call<'a>(&'a self, _call: &'a ModelCall) -> BoxFuture<'a, Result<ModelReply, ModelCallError>> {
            unreachable!()
        }
        fn base_url(&self) -> Option<&str> {
            Some(&self.0)
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_base_url_refusal_never_prints_its_query_its_anchor_or_its_userinfo() {
        for bad in ["http://me:hunter2@127.0.0.1:1/v 1?token=abc123#x", "http://127.0.0.1:1/v1?token=abc 123", "http://127.0.0.1:1/v1#abc\u{1}123"] {
            let message = refusal(Client::with_base_url(KEY, bad).with_retries(0).stats(None).await);
            assert!(message.starts_with("invalid base_url (it contains whitespace, a backslash or a control character)"), "{bad:?}: {message}");
            assert!(!message.contains("abc") && !message.contains("hunter2"), "{message}");
        }
        let said = "the model provider's base URL is not an http(s) URL: ";
        for (bad, shown) in [
            ("https://me:hunter2@api.example.com/v 1?token=abc123#x", "\"https://***@api.example.com/v 1?\""),
            ("https://api.example.com/v1?token=abc 123", "\"https://api.example.com/v1?\""),
            ("https://api.example.com/v1#abc\\123", "\"https://api.example.com/v1#\""),
        ] {
            let caller = || -> std::sync::Arc<dyn ModelCaller> { std::sync::Arc::new(AtBase(bad.into())) };
            let made = Client::new(KEY).with_router(RouterOptions { router_caller: Some(caller()), ..Default::default() });
            assert_eq!(refusal(made.map(|_| ())), format!("{said}{shown}"), "{bad:?}");
            match Client::new(KEY).sleep("u", caller(), &SleepOpts::default()).await {
                Err(SleepError::Wos(WosError::Api { status: 400, message, .. })) => assert_eq!(message, format!("{said}{shown}"), "{bad:?}"),
                other => panic!("expected a refusal, got {other:?}"),
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_password_holding_a_question_mark_or_a_hash_is_masked_before_the_url_is_cut() {
        let said = "the model provider's base URL is not an http(s) URL: ";
        for (bad, shown) in [
            ("https://me:hun?ter2@api.example.com/v 1", "\"https://***@api.example.com/v 1\""),
            ("https://me:hun#ter2@api.example.com/v1", "\"https://***@api.example.com/v1\""),
        ] {
            let caller = || -> std::sync::Arc<dyn ModelCaller> { std::sync::Arc::new(AtBase(bad.into())) };
            let made = Client::new(KEY).with_router(RouterOptions { router_caller: Some(caller()), ..Default::default() });
            assert_eq!(refusal(made.map(|_| ())), format!("{said}{shown}"), "{bad:?}");
            match Client::new(KEY).sleep("u", caller(), &SleepOpts::default()).await {
                Err(SleepError::Wos(WosError::Api { status: 400, message, .. })) => assert_eq!(message, format!("{said}{shown}"), "{bad:?}"),
                other => panic!("expected a refusal, got {other:?}"),
            }
        }
    }

    #[test]
    fn the_query_refusal_tells_each_caller_the_base_url_it_takes() {
        let said = "the model provider's base URL has a ? query or a # anchor: give ";
        for bad in ["https://api.openai.com/v1?api-version=1", "https://api.openai.com/v1#chat"] {
            let openai = openai_caller(OpenAiCallerOpts { base_url: bad.into(), model: "m".into(), ..Default::default() });
            assert_eq!(refusal(openai), format!("{said}the base URL without them, e.g. https://api.openai.com/v1."), "{bad}");
            let made = [
                anthropic_caller(AnthropicCallerOpts { api_key: LLM_KEY.into(), base_url: Some(bad.into()), ..Default::default() }).map(|_| ()),
                Client::new(KEY)
                    .with_router(RouterOptions { key: Some(LLM_KEY.into()), provider_base_url: Some(bad.into()), ..Default::default() })
                    .map(|_| ()),
            ];
            for r in made {
                assert_eq!(refusal(r), format!("{said}the API origin without them, e.g. https://api.anthropic.com."), "{bad}");
            }
        }
    }

    #[test]
    fn the_plain_http_warning_comes_once_per_base_url() {
        let base = "http://wos-proxy-once.example.com";
        assert!(plain_http_once("base_url", base), "the first client on a base warns");
        assert!(!plain_http_once("base_url", base), "a second does not");
        let _ = Client::with_base_url(KEY, "http://wos-proxy-twice.example.com/");
        let _ = Client::with_base_url(KEY, "http://wos-proxy-twice.example.com");
        assert!(!plain_http_once("base_url", "http://wos-proxy-twice.example.com"), "the clients already warned");
        assert!(!plain_http_once("base_url", "http://127.0.0.1:9"), "loopback stays quiet");
    }

    #[test]
    fn a_model_key_over_plain_http_to_a_remote_host_warns_once_per_base() {
        let opts = |base: &str| AnthropicCallerOpts { api_key: LLM_KEY.into(), base_url: Some(base.into()), ..Default::default() };
        anthropic_caller(opts("http://llm-a.example.com")).unwrap();
        anthropic_caller(opts("http://llm-a.example.com/")).unwrap();
        assert!(!plain_http_once("provider", "http://llm-a.example.com"), "the caller already warned");
        let oai = |base: &str, key: Option<&str>| OpenAiCallerOpts {
            base_url: base.into(),
            model: "m".into(),
            api_key: key.map(str::to_string),
            ..Default::default()
        };
        openai_caller(oai("http://llm-b.example.com/v1", Some(LLM_KEY))).unwrap();
        assert!(!plain_http_once("provider", "http://llm-b.example.com/v1"), "a keyed OpenAI caller warned");
        openai_caller(oai("http://llm-c.example.com/v1", None)).unwrap();
        assert!(plain_http_once("provider", "http://llm-c.example.com/v1"), "a caller with no key sends none, so it did not warn");
        Client::new(KEY)
            .with_router(RouterOptions { key: Some(LLM_KEY.into()), provider_base_url: Some("http://llm-d.example.com".into()), ..Default::default() })
            .unwrap();
        assert!(!plain_http_once("provider", "http://llm-d.example.com"), "the router's key caller warned");
        anthropic_caller(opts("http://127.0.0.1:9")).unwrap();
        assert!(!plain_http_once("provider", "http://127.0.0.1:9"), "loopback stays quiet");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_content_encoding_is_never_inflated() {
        let (base, _) = super::tests::mock_server(vec![super::tests::http(
            "200 OK",
            "Content-Encoding: gzip, gzip\r\n",
            "{\"total_memories\":1}",
        )]);
        let got = Client::with_base_url(KEY, &base).with_retries(0).stats(None).await.unwrap();
        assert_eq!(got["total_memories"], 1, "the body is read as it came, so no encoding is decoded");
    }

    #[test]
    fn the_api_key_header_is_marked_sensitive() {
        assert!(Client::new(KEY).key_header().is_sensitive());
        assert_eq!(Client::new(KEY).key_header().to_str().unwrap(), KEY);
    }
