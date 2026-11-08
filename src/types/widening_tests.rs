    use super::RecallResponse;

    #[test]
    fn a_field_this_struct_does_not_name_still_arrives() {
        let v = serde_json::json!({
            "short_term": {"turns": [], "count": 0},
            "long_term": {"memories": [], "count": 0},
            "context": {"around_top_memory": [], "count": 0},
            "a_field_added_after_this_struct_was_written": {"n": 7},
            "verify_used": 2
        });
        let got: RecallResponse = serde_json::from_value(v).unwrap();
        assert_eq!(got.extra["verify_used"], 2, "a named-later field was dropped");
        assert_eq!(got.extra["a_field_added_after_this_struct_was_written"]["n"], 7);
        assert_eq!(got.short_term["count"], 0);
    }

    #[test]
    fn a_reply_with_nothing_extra_leaves_it_empty() {
        let v = serde_json::json!({"short_term": {}, "long_term": {}, "context": {}});
        let got: RecallResponse = serde_json::from_value(v).unwrap();
        assert!(got.extra.is_empty());
    }
