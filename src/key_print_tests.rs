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
    async fn the_two_kinds_of_key_cannot_be_swapped_and_the_error_never_prints_the_key() {
        let provider_key = "sk-ant-api03-guards-000000000000000000";
        let wos_key = "wos-test-guards-0000000000000000000000";
        assert_eq!(
            refusal(Client::new(provider_key).with_retries(0).stats(None).await),
            "api_key starts with sk-, so it is a model provider's key: give the Wontopos key here (it starts with wos-)."
        );
        let said = |what: &str| {
            format!("{what} starts with wos-, so it is a Wontopos key: a Wontopos key never goes to a model provider. Give the provider's own key.")
        };
        assert_eq!(
            refusal(anthropic_caller(AnthropicCallerOpts { api_key: wos_key.into(), ..Default::default() })),
            said("api_key")
        );
        assert_eq!(
            refusal(openai_caller(OpenAiCallerOpts {
                base_url: "https://api.example.com/v1".into(),
                model: "m".into(),
                api_key: Some(wos_key.into()),
                ..Default::default()
            })),
            said("api_key")
        );
        assert_eq!(
            refusal(Client::new(KEY).with_router(RouterOptions { key: Some(wos_key.into()), ..Default::default() })),
            said("RouterOptions.key")
        );
        assert_eq!(
            refusal(Client::new(KEY).with_router(RouterOptions {
                key: Some(LLM_KEY.into()),
                skill_key: Some(wos_key.into()),
                ..Default::default()
            })),
            said("RouterOptions.skill_key")
        );
        let own = "own-gateway-key-000000000000000000";
        assert_eq!(
            refusal(Client::new(own).with_router(RouterOptions { key: Some(own.into()), ..Default::default() })),
            "RouterOptions.key is this client's Wontopos key: a Wontopos key never goes to a model provider. Give the provider's own key."
        );
    }

    #[test]
    fn a_key_prints_as_its_last_4_characters_and_only_from_24_characters_up() {
        let long = "wos-live-0123456789abcdeWXYZ";
        let at24 = "wos-live-0123456789aWXYZ";
        let at23 = "wos-live-0123456789WXYZ";
        assert_eq!((long.len(), at24.len(), at23.len()), (28, 24, 23));
        for (key, shown) in [(long, "***WXYZ"), (at24, "***WXYZ"), (at23, "***")] {
            assert_eq!(mask_key(key), shown);
            let provider = key.replace("wos-", "sk-a");
            let views = [
                format!("{:?}", Client::new(key)),
                format!("{:?}", AnthropicCallerOpts { api_key: provider.clone(), ..Default::default() }),
                format!("{:?}", anthropic_caller(AnthropicCallerOpts { api_key: provider.clone(), ..Default::default() }).unwrap()),
                format!(
                    "{:?}",
                    OpenAiCallerOpts { base_url: "https://api.example.com/v1".into(), api_key: Some(provider.clone()), ..Default::default() }
                ),
                format!(
                    "{:?}",
                    openai_caller(OpenAiCallerOpts {
                        base_url: "https://api.example.com/v1".into(),
                        model: "m".into(),
                        api_key: Some(provider.clone()),
                        ..Default::default()
                    })
                    .unwrap()
                ),
                format!("{:?}", RouterOptions { key: Some(provider.clone()), skill_key: Some(provider.clone()), ..Default::default() }),
            ];
            for view in views {
                assert!(view.contains(&format!("{shown:?}")), "{key}: {view}");
                assert!(!view.contains("0123456789"), "{key}: {view}");
            }
        }
    }

    #[test]
    fn a_printed_base_url_masks_only_the_userinfo_of_its_authority() {
        for (base, shown) in [
            ("https://proxy.example.com/v1?@api.wontopos.com", "https://proxy.example.com/v1?@api.wontopos.com"),
            ("https://proxy.example.com#@api.wontopos.com", "https://proxy.example.com#@api.wontopos.com"),
            ("https://proxy.example.com/team@api.wontopos.com", "https://proxy.example.com/team@api.wontopos.com"),
            ("https://user:pw@proxy.example.com/a@b", "https://***@proxy.example.com/a@b"),
        ] {
            assert_eq!(redacted_base_url(base), shown);
            let dbg = format!("{:?}", Client::with_base_url(KEY, base));
            assert!(dbg.contains(&format!("{shown:?}")), "{base}: {dbg}");
            let opts = format!("{:?}", AnthropicCallerOpts { api_key: LLM_KEY.into(), base_url: Some(base.into()), ..Default::default() });
            assert!(opts.contains(&format!("{shown:?}")), "{base}: {opts}");
        }
    }
