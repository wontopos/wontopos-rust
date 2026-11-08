    use super::*;

    #[test]
    fn a_passed_blank_store_id_is_rejected_rather_than_redirected() {
        let c = Client::new("k").with_user("alice");
        for blank in ["", " ", "\t", "\n", "   "] {
            let e = c.uid(Some(blank)).expect_err("a blank id must not resolve to a store");
            match e {
                WosError::Api { status, ref message, .. } => {
                    assert_eq!(status, 400, "client-side rejection uses 400, got {status}");
                    assert!(
                        message.contains("passed but is blank"),
                        "message should name the actual mistake, got: {message}"
                    );
                }
                other => panic!("expected a 400 Api error, got {other:?}"),
            }
        }
    }

    #[test]
    fn an_omitted_store_id_still_uses_the_client_default() {
        let c = Client::new("k").with_user("alice");
        assert_eq!(c.uid(None).unwrap(), "alice");
    }

    #[test]
    fn a_real_store_id_is_sent_exactly_as_written() {
        let c = Client::new("k").with_user("alice");
        assert_eq!(c.uid(Some("bob")).unwrap(), "bob");
        assert_eq!(c.uid(Some(" bob ")).unwrap(), " bob ");
    }
