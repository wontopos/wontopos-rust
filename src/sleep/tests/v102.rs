use super::*;

fn py() -> Value {
    serde_json::from_str(include_str!("../../testdata/sleep_py_102.json")).unwrap()
}

fn cases(g: &Value, key: &str) -> Vec<Value> {
    g[key].as_array().unwrap().clone()
}

#[test]
fn what_python_makes_of_what_the_sleep_shows_rust_makes_the_same() {
    let g = py();
    let today = sleep_date("2026-10-05").unwrap();
    for c in cases(&g, "age") {
        assert_eq!(sleep_age(c[0].as_str(), today), c[1].as_str().unwrap(), "age of {}", c[0]);
    }
    for c in cases(&g, "kept") {
        assert_eq!(sleep_kept(c[0].as_str().unwrap()), c[1].as_str().unwrap(), "kept {}", c[0]);
    }
    for c in cases(&g, "named") {
        assert_eq!(sleep_named(c[0].as_str().unwrap()), c[1].as_str().unwrap(), "named {}", c[0]);
    }
    for c in cases(&g, "who") {
        assert_eq!(sleep_who(c[0].as_str().unwrap(), c[1].as_str().unwrap()), c[2].as_str().unwrap(), "who {} {}", c[0], c[1]);
    }
    for c in cases(&g, "core") {
        assert_eq!(sleep_core(c[0].as_str().unwrap()), c[1].as_str().unwrap(), "core {}", c[0]);
    }
    for c in cases(&g, "render_more") {
        let files: Vec<SleepFile> = c[0].as_array().unwrap().iter().map(sleep_file).collect();
        let view: SleepView = files.try_into().unwrap();
        assert_eq!(sleep_render(&view, today), c[1].as_str().unwrap());
        assert_eq!(sleep_box_index(&view, today), c[2].as_str().unwrap());
        let boxes: Vec<String> = (1..=4).map(|k| sleep_box_text(k, &view[k], today)).collect();
        let want: Vec<String> = c[3].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        assert_eq!(boxes, want);
    }
}
