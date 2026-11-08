    use super::*;

    #[test]
    fn utc_day_reads_the_calendar() {
        let at = |s: &str| httpdate::parse_http_date(s).unwrap();
        assert_eq!(utc_day(at("Sat, 07 Nov 2026 00:00:00 GMT")), "2026-11-07");
        assert_eq!(utc_day(at("Thu, 29 Feb 2024 23:59:59 GMT")), "2024-02-29");
        assert_eq!(utc_day(at("Thu, 01 Jan 1970 00:00:00 GMT")), "1970-01-01");
        assert_eq!(utc_day(at("Fri, 31 Dec 9999 23:59:59 GMT")), "9999-12-31");
    }

    #[test]
    fn a_retirement_is_said_once_per_model_and_day() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("sunset", "Sat, 07 Nov 2026 00:00:00 GMT".parse().unwrap());
        assert!(warn_if_retiring("scroll-1-once", &h));
        assert!(!warn_if_retiring("scroll-1-once", &h), "once per model and day");
        h.insert("sunset", "Sun, 08 Nov 2026 00:00:00 GMT".parse().unwrap());
        assert!(warn_if_retiring("scroll-1-once", &h), "a new day is news");
        let mut bad = reqwest::header::HeaderMap::new();
        bad.insert("sunset", "soon".parse().unwrap());
        assert!(!warn_if_retiring("scroll-1-once", &bad), "not a date, nothing said");
        assert!(!warn_if_retiring("scroll-1-once", &reqwest::header::HeaderMap::new()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_image_fetch_says_it_too() {
        use super::tests::{http, mock_server};
        let (base, _) = mock_server(vec![http("200 OK", "Sunset: Sat, 07 Nov 2026 00:00:00 GMT\r\n", "PNGBYTES")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_model("scroll-1-image");
        let (bytes, _) = mem.get_image("u", "11111111-2222-3333-4444-555555555555").await.unwrap();
        assert_eq!(bytes, b"PNGBYTES");
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("sunset", "Sat, 07 Nov 2026 00:00:00 GMT".parse().unwrap());
        assert!(!warn_if_retiring("scroll-1-image", &h), "the image fetch already said it");
    }

    #[test]
    fn recall_parts_read_as_types() {
        let r: RecallResponse = serde_json::from_value(serde_json::json!({
            "short_term": {"turns": [
                {"role": "user", "content": "hi", "timestamp": "2026-10-13T09:00:00Z"},
                "not a turn",
                {"role": "assistant", "content": "hello", "timestamp": null}
            ], "count": 2},
            "long_term": {"memories": [{"id": "m1", "content": "she prefers tea"}], "count": 1},
            "context": {"around_top_memory": ["[before] a", "[match] b", 3, "[after] c"], "count": 4}
        }))
        .unwrap();
        let turns = r.turns();
        assert_eq!(turns.len(), 2);
        assert_eq!((turns[0].role.as_str(), turns[0].content.as_str()), ("user", "hi"));
        assert_eq!(turns[0].timestamp.as_deref(), Some("2026-10-13T09:00:00Z"));
        assert_eq!(turns[1].timestamp, None);
        assert_eq!(r.memories()[0].content, "she prefers tea");
        assert_eq!(r.context_rows(), vec!["[before] a", "[match] b", "[after] c"]);
        let empty: RecallResponse = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(empty.turns().is_empty() && empty.memories().is_empty() && empty.context_rows().is_empty());
    }
