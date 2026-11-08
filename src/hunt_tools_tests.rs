    use super::rooms_work_tests::{count, refusal, scripted, KEY};
    use super::{Client, ToolContent, WosError};
    use serde_json::json;

    const NOTHING_OPEN: &str = "Nothing is open: memory tools answer within a turn that began with begin().";
    const OPEN_TEA: &str = r#"{"work_id":"w-1","memories":[{"n":1,"id":"m-1","content":"Tea at nine."}]}"#;
    const CLOSED: &str =
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"that store is closed","request_id":"req_1"}}"#;

    fn scroll2(base: &str) -> Client {
        Client::with_base_url(KEY, base).with_model("scroll-2").with_retries(0)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_blank_question_is_refused_before_anything_is_sent() {
        let (base, seen) = scripted(vec![]);
        let tools = scroll2(&base).memory_tools_preset("u", None, "S-2").unwrap().with_speakers(true).with_sleep(true);
        for blank in ["", " \t"] {
            assert_eq!(refusal(tools.begin(blank).await), "question is required.");
        }
        assert_eq!(count(&seen), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_turn_that_failed_to_begin_leaves_nothing_open() {
        let (base, seen) = scripted(vec![(200, OPEN_TEA), (400, CLOSED)]);
        let tools = scroll2(&base).memory_tools_preset("u", None, "S-2").unwrap().with_dates(true);
        tools.begin("tea").await.unwrap();
        assert!(matches!(tools.begin("coffee").await, Err(WosError::Api { status: 400, .. })));
        assert_eq!(tools.work_id(), None);
        assert_eq!(tools.run("memory_timeline", &json!({})).await.unwrap(), NOTHING_OPEN);
        assert_eq!(tools.answered(Some(1)).await.unwrap(), None);
        assert_eq!(count(&seen), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_name_that_reads_as_a_label_once_its_invisible_characters_are_gone_is_quoted() {
        let names = [
            "you\u{200b}", "\u{200d}user", "no\u{ad}te", "U\u{2060}SER", "Yo\u{34f}u", "you\u{fe0f}", "\u{200b} you",
            "y\u{e0020}ou", "Da\u{200b}na",
        ];
        let memories: Vec<serde_json::Value> = names
            .iter()
            .enumerate()
            .map(|(i, s)| json!({"n": i + 1, "id": format!("m-{}", i + 1), "content": "Paid.", "speaker": s}))
            .collect();
        let opened = json!({"work_id": "w-1", "memories": memories}).to_string();
        let turns: Vec<serde_json::Value> = names.iter().map(|s| json!({"role": "user", "speaker": s, "content": "Paid."})).collect();
        let history = json!({"turns": turns}).to_string();
        let (base, _seen) = scripted(vec![(200, opened.as_str()), (200, history.as_str())]);
        let tools =
            scroll2(&base).memory_tools_preset("u", None, "S-2").unwrap().with_speakers(true).with_seen(true).with_assistant("Kai").unwrap();
        let mut shown: Vec<String> = names[..names.len() - 1].iter().map(|s| format!("\"{s}\"")).collect();
        shown.push(names[names.len() - 1].to_string());
        let listed: Vec<String> = shown.iter().enumerate().map(|(i, s)| format!("[{:02}] {s}: Paid.", i + 1)).collect();
        assert_eq!(tools.begin("paid").await.unwrap(), listed.join("\n"));
        let recent: Vec<String> = shown.iter().rev().enumerate().map(|(i, s)| format!("#{} {s}: Paid.", i + 1)).collect();
        assert_eq!(
            tools.run("memory_recent", &json!({})).await.unwrap(),
            format!("The recent conversation, newest first:\n{}", recent.join("\n"))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn work_id_is_none_when_the_open_reply_names_no_work() {
        let replies = [r#"{"work_id": 5, "memories": []}"#, r#"{"work_id": "", "memories": []}"#, r#"{"memories": []}"#, OPEN_TEA];
        let (base, _seen) = scripted(replies.iter().map(|r| (200, *r)).collect());
        let tools = scroll2(&base).memory_tools_preset("u", None, "S-2").unwrap();
        for want in [None, None, None, Some("w-1")] {
            tools.begin("tea").await.unwrap();
            assert_eq!(tools.work_id().as_deref(), want);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_memory_with_an_empty_id_is_neither_a_photo_nor_an_answer() {
        let opened = r#"{"work_id":"w-1","memories":[{"n":1,"id":"","content":"The beach.","image_ref":"img-1"},
            {"n":2,"id":"m-2","content":"The dunes.","image_ref":"img-2"}]}"#;
        let (base, seen) = scripted(vec![(200, opened)]);
        let tools = scroll2(&base).memory_tools_preset("u", None, "S-2").unwrap().with_images(true);
        assert_eq!(tools.begin("beach").await.unwrap(), "[01] The beach.\n[02] (photo) The dunes.");
        assert_eq!(
            tools.run_content("memory_photo", &json!({"n": 1})).await.unwrap(),
            ToolContent::Text("Refused: [01] is not a number this turn gave.".into())
        );
        assert_eq!(refusal(tools.answered(Some(1)).await), "[01] is not a number this turn gave.");
        assert_eq!(count(&seen), 1);
    }
