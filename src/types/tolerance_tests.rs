    use super::memories_from;

    #[test]
    fn a_wrong_typed_field_does_not_delete_the_memory() {
        let v = serde_json::json!([
            {"id": "m1", "content": "kept", "similarity": 0.9},
            {"id": 123,   "content": 42,    "similarity": "high", "is_superseded": "yes"},
            {"id": "m3",  "content": "also kept"}
        ]);
        let got = memories_from(Some(&v));
        assert_eq!(got.len(), 3, "a record was dropped for one unreadable field");
        assert_eq!(got[0].content, "kept");
        assert_eq!(got[2].content, "also kept");
        assert_eq!(got[1].content, "");
        assert_eq!(got[1].id, None);
        assert_eq!(got[1].similarity, 0.0);
        assert!(!got[1].is_superseded);
    }

    #[test]
    fn an_element_that_is_not_an_object_is_still_dropped() {
        let v = serde_json::json!([{"id": "m1", "content": "kept"}, "not an object", 7, null]);
        assert_eq!(memories_from(Some(&v)).len(), 1);
    }

    #[test]
    fn a_null_scalar_still_defaults() {
        let v = serde_json::json!([{"id": "m1", "content": null, "similarity": null}]);
        let got = memories_from(Some(&v));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].content, "");
        assert_eq!(got[0].similarity, 0.0);
    }
