use super::*;

#[tokio::test(flavor = "current_thread")]
async fn today_takes_a_clock_of_hours_0_to_23_and_minutes_0_to_59() {
    let (svc, mem) = new();
    let model = Arc::new(Model::default());
    let m: Arc<dyn ModelCaller> = model.clone();
    for (today, want) in [
        ("2026-10-20 24:00", "today must be"),
        ("2026-10-20 23:60", "today must be"),
        ("2026-10-20 99:99", "today must be"),
        ("2026-10-20 23:59", "max_tokens must be"),
        ("2026-10-20 00:00", "max_tokens must be"),
        ("2026-10-20 19:05", "max_tokens must be"),
    ] {
        let opts = SleepOpts { today: Some(today.into()), max_tokens: 0, ..Default::default() };
        let e = wos(mem.sleep("u", m.clone(), &opts).await);
        assert!(e.to_string().contains(want), "{today}: want {want:?}, got {e}");
    }
    assert!(svc.lock().unwrap().seen.is_empty());
    assert!(model.calls().is_empty());
}
