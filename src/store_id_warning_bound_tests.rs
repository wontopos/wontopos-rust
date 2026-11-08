    use super::*;

    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn reset_store_ids() {
        if let Some(m) = WARNED_STORE_IDS.get() {
            m.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
    }

    #[test]
    fn warned_ids_stay_bounded() {
        let _g = serial();
        reset_store_ids();
        for i in 0..(WARNED_STORE_IDS_MAX * 3) {
            warn_if_store_id_collapses(&format!("user.{i}@example.com"));
        }
        let n = WARNED_STORE_IDS.get().unwrap().lock().unwrap().len();
        assert!(
            n <= WARNED_STORE_IDS_MAX,
            "warning record exceeded its cap: {n} > {WARNED_STORE_IDS_MAX}"
        );
    }

    #[test]
    fn a_late_first_collision_is_still_recorded() {
        let _g = serial();
        reset_store_ids();
        for i in 0..(WARNED_STORE_IDS_MAX * 2) {
            warn_if_store_id_collapses(&format!("user.{i}@example.com"));
        }
        warn_if_store_id_collapses("zzz.late@example.com");
        let g = WARNED_STORE_IDS.get().unwrap().lock().unwrap();
        assert!(g.iter().any(|s| s == "zzz.late@example.com"), "late collision was not recorded");
        assert!(g.len() <= WARNED_STORE_IDS_MAX);
    }

    #[test]
    fn an_invalid_id_in_normal_form_is_still_warned_about() {
        let _g = serial();
        let ids = ["_invalid_probe_a".to_string(), "y".repeat(65)];
        for id in &ids {
            warn_if_store_id_collapses(id);
        }
        let g = WARNED_STORE_IDS.get().unwrap().lock().unwrap();
        assert!(ids.iter().all(|id| g.iter().any(|s| s == id)));
    }

    #[test]
    fn clean_ids_are_not_recorded() {
        let _g = serial();
        let ids: Vec<String> = (0..100).map(|i| format!("clean_id_probe_{i}")).collect();
        for id in &ids {
            warn_if_store_id_collapses(id);
        }
        if let Some(m) = WARNED_STORE_IDS.get() {
            let g = m.lock().unwrap();
            assert!(
                !g.iter().any(|s| ids.contains(s)),
                "an already-canonical id has no reason to be recorded"
            );
        }
    }
