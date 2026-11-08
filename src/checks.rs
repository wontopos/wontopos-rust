use super::*;

pub(crate) fn normalize_store_id(id: &str) -> String {
    id.chars().map(|c| c.to_ascii_lowercase()).map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' { c } else { '_' }).collect()
}

pub(crate) const WARNED_STORE_IDS_MAX: usize = 1024;
pub(crate) static WARNED_STORE_IDS: std::sync::OnceLock<std::sync::Mutex<std::collections::VecDeque<String>>> =
    std::sync::OnceLock::new();

pub(crate) fn check_metadata_keys(metadata: &serde_json::Value) -> Result<(), WosError> {
    if let Some(obj) = metadata.as_object() {
        for k in obj.keys() {
            let folded: String = k.chars().filter(|c| !js_space(*c) && !matches!(c, '_' | '-' | '.')).flat_map(char::to_lowercase).collect();
            if matches!(folded.as_str(), "userid" | "storeid" | "idempotencykey") {
                return Err(refused(format!(
                    "{k:?} is not a metadata field: pass the store as user_id and an \
                     idempotency key through the *_idempotent methods. Nothing was sent."
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn valid_store_id_format(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

pub(crate) fn warn_if_store_id_collapses(id: &str) {
    if id.is_empty() {
        return;
    }
    let valid = valid_store_id_format(id);
    let normalized = normalize_store_id(id);
    if valid && normalized == id {
        return;
    }
    let seen = WARNED_STORE_IDS.get_or_init(|| std::sync::Mutex::new(std::collections::VecDeque::new()));
    let first = seen.lock().map_or(true, |mut g| {
        let new = !g.iter().any(|s| s == id);
        if new {
            g.push_back(id.to_string());
            while g.len() > WARNED_STORE_IDS_MAX {
                g.pop_front();
            }
        }
        new
    });
    if first && !valid {
        eprintln!(
            "wontopos: store id {id:?} is not a valid store id: use 1-64 ASCII letters, digits, \
             '.', '_' and '-', starting with a letter or digit. Creating it is refused (400)."
        );
    } else if first {
        eprintln!(
            "wontopos: store id {id:?} normalizes to {normalized:?}. Ids that differ only by case \
             name this same store; one that differs only by punctuation cannot be created beside \
             it (409) and is not found when used (404). If these ids come from your end users, \
             normalize them yourself first so two people never compete for one name."
        );
    }
}

pub(crate) const KNOWN_FILTER_KEYS: [&str; 7] = ["categories", "event_from", "event_to", "time_from", "time_to", "min_importance", "speakers"];

pub(crate) const KNOWN_METADATA_KEYS: [&str; 4] = ["speaker", "event_date", "category", "conversation_id"];

pub(crate) const WARNED_KEYS_MAX: usize = 1024;
pub(crate) type WarnSet = std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>>;
pub(crate) static WARNED_FILTER_KEYS: WarnSet = std::sync::OnceLock::new();
pub(crate) static WARNED_METADATA_KEYS: WarnSet = std::sync::OnceLock::new();

pub(crate) fn first_sighting(set: &WarnSet, key: &str) -> bool {
    let seen = set.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    let Ok(mut g) = seen.lock() else { return true };
    let inserted = g.insert(key.to_string());
    while g.len() > WARNED_KEYS_MAX {
        let Some(victim) = g.iter().next().cloned() else { break };
        g.remove(&victim);
    }
    inserted
}

pub(crate) fn warn_on_unknown_filters(body: &serde_json::Value) {
    let Some(filters) = body.get("filters").and_then(|f| f.as_object()) else { return };
    for k in filters.keys() {
        if !KNOWN_FILTER_KEYS.contains(&k.as_str()) && first_sighting(&WARNED_FILTER_KEYS, k) {
            eprintln!(
                "wontopos: unknown search filter {k:?} — the API drops keys it does not know, so \
                 this filter has NO effect and the search is wider than you think. Known keys: {}",
                KNOWN_FILTER_KEYS.join(", ")
            );
        }
    }
}

pub(crate) fn check_filters(body: &serde_json::Value) -> Result<(), WosError> {
    warn_on_unknown_filters(body);
    let Some(m) = body.get("filters").and_then(|f| f.get("min_importance")).filter(|m| !m.is_null()) else {
        return Ok(());
    };
    if m.as_f64().is_some_and(|n| (0.0..=1.0).contains(&n)) {
        return Ok(());
    }
    Err(refused(format!("filters.min_importance must be a number between 0 and 1, got {m}.")))
}

pub(crate) fn warn_on_unknown_metadata(metadata: &serde_json::Value) {
    let Some(md) = metadata.as_object() else { return };
    for k in md.keys() {
        if !KNOWN_METADATA_KEYS.contains(&k.as_str()) && first_sighting(&WARNED_METADATA_KEYS, k) {
            eprintln!(
                "wontopos: unknown metadata key {k:?}: the service keeps only {} and drops every \
                 other key, so this value is not stored.",
                KNOWN_METADATA_KEYS.join(", ")
            );
        }
    }
}

#[doc(hidden)]
pub fn _reset_warning_state() {
    if let Some(m) = WARNED_STORE_IDS.get() {
        if let Ok(mut g) = m.lock() { g.clear(); }
    }
    for set in [&WARNED_FILTER_KEYS, &WARNED_METADATA_KEYS, &WARNED_RETIREMENTS] {
        if let Some(m) = set.get() {
            if let Ok(mut g) = m.lock() { g.clear(); }
        }
    }
}

pub(crate) fn valid_idempotency_key(k: &str) -> bool {
    !k.is_empty() && k.len() <= 128 && k.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

pub const SEARCH_LIMIT_MIN: usize = 5;
pub const SEARCH_LIMIT_MAX: usize = 20;
pub const CONTEXT_LIMIT_MIN: usize = 0;
pub const CONTEXT_LIMIT_MAX: usize = 20;

pub(crate) const PAGE_LIMIT_MIN: usize = 5;
pub(crate) const PAGE_LIMIT_MAX: usize = 20;
pub(crate) const LIST_LIMIT_MIN: usize = 1;
pub(crate) const LIST_LIMIT_MAX: usize = 500;
pub(crate) const MAX_IMAGES_MAX: usize = 5;
pub(crate) const VERIFY_MAX: usize = 3;
pub(crate) fn check_search_limit(limit: usize) -> Result<(), WosError> {
    if !(SEARCH_LIMIT_MIN..=SEARCH_LIMIT_MAX).contains(&limit) {
        return Err(refused(format!(
            "limit must be an integer between {SEARCH_LIMIT_MIN} and {SEARCH_LIMIT_MAX}, got {limit}. \
             Out of range is refused rather than adjusted, so a short answer always \
             means the store was short."
        )));
    }
    Ok(())
}

pub(crate) fn page_fields(body: &mut serde_json::Value, limit: Option<usize>, before: Option<&str>, skip_ids: Option<&[String]>) -> Result<(), WosError> {
    if let Some(l) = limit {
        check_range("limit", l, PAGE_LIMIT_MIN, PAGE_LIMIT_MAX)?;
        body["limit"] = serde_json::json!(l);
    }
    if let Some(b) = before {
        body["before"] = serde_json::json!(b);
    }
    if let Some(s) = skip_ids {
        body["skip_ids"] = serde_json::json!(s);
    }
    Ok(())
}

pub(crate) fn check_body_count(body: &serde_json::Value, name: &str, max: usize) -> Result<(), WosError> {
    let Some(v) = body.get(name).filter(|v| !v.is_null()) else { return Ok(()) };
    match v.as_u64() {
        Some(n) if n <= max as u64 => Ok(()),
        _ => Err(WosError::Api { status: 400, message: format!("{name} must be an integer between 0 and {max}, got {v}.") }),
    }
}

pub(crate) fn window_line(obj: &mut serde_json::Map<String, serde_json::Value>, opts: &WriteOpts) -> Result<(), WosError> {
    if opts.window_role.is_some() && opts.window != Some(true) {
        return Err(refused("window_role needs window: it names who said the line in the window."));
    }
    if opts.window == Some(true) {
        obj.insert("window".into(), serde_json::json!(true));
        if let Some(r) = &opts.window_role {
            if r.is_empty() || r.len() > 32 || !r.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(refused("window_role must be 1-32 characters of [A-Za-z0-9_]."));
            }
            obj.insert("window_role".into(), serde_json::json!(r));
        }
    }
    Ok(())
}

pub(crate) fn refuse_window(opts: &WriteOpts, why: &str) -> Result<(), WosError> {
    if opts.window.is_some() || opts.window_role.is_some() {
        return Err(refused(format!("window is for add: {why}.")));
    }
    Ok(())
}

pub(crate) fn refuse_turn_fields(opts: &WriteOpts) -> Result<(), WosError> {
    for (name, v) in [("created_at", &opts.created_at), ("user_speaker", &opts.user_speaker), ("assistant_speaker", &opts.assistant_speaker)] {
        if v.is_some() {
            return Err(refused(format!("{name} is for add_turn_opts: it dates or names a side of a turn.")));
        }
    }
    Ok(())
}

pub(crate) fn turn_fields(obj: &mut serde_json::Map<String, serde_json::Value>, opts: &WriteOpts) -> Result<(), WosError> {
    if let Some(at) = &opts.created_at {
        obj.insert("created_at".into(), serde_json::json!(check_instant(at, "created_at")?));
    }
    for (name, v) in [("user_speaker", &opts.user_speaker), ("assistant_speaker", &opts.assistant_speaker)] {
        if let Some(who) = v {
            obj.insert(name.into(), serde_json::json!(written_speaker(who, name)?));
        }
    }
    Ok(())
}

pub(crate) fn check_speaker(v: &str, name: &str) -> Result<String, WosError> {
    let t = js_trim(v);
    if t.is_empty() {
        return Err(refused(format!("{name} is required.")));
    }
    if t.len() > SPEAKER_MAX_BYTES {
        return Err(refused(format!("{name} is at most {SPEAKER_MAX_BYTES} bytes.")));
    }
    Ok(t.to_string())
}

pub(crate) fn written_speaker(v: &str, name: &str) -> Result<String, WosError> {
    let t = check_speaker(v, name)?;
    one_line_speaker(&t, name)?;
    Ok(t)
}

pub(crate) fn one_line_speaker(name: &str, what: &str) -> Result<(), WosError> {
    if name.chars().any(|c| matches!(c, '\0'..='\x1f' | '\x7f'..='\u{9f}' | '\u{2028}' | '\u{2029}')) {
        return Err(refused(format!("{what} contains a line break or a control character.")));
    }
    Ok(())
}

pub(crate) fn rfc3339_shaped(t: &str) -> bool {
    let b = t.as_bytes();
    let digit = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    let num = |i: usize| u32::from(b[i] - b'0') * 10 + u32::from(b[i + 1] - b'0');
    let fixed = b.len() >= 20
        && [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18].iter().all(|&i| digit(i))
        && [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')].iter().all(|&(i, c)| b[i] == c);
    if !fixed {
        return false;
    }
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while digit(i) {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    let zone = match &b[i..] {
        [b'Z'] => true,
        [s, _, _, b':', _, _] if (*s == b'+' || *s == b'-') && [1, 2, 4, 5].iter().all(|&k| digit(i + k)) => {
            num(i + 1) < 24 && num(i + 4) < 60
        }
        _ => false,
    };
    let year = num(0) * 100 + num(2);
    zone && year >= 1 && num(11) < 24 && num(14) < 60 && num(17) < 60
}

pub(crate) fn check_instant(v: &str, name: &str) -> Result<String, WosError> {
    let t = check_id(v, name)?;
    if !rfc3339_shaped(&t) || chrono::DateTime::parse_from_rfc3339(&t).is_err() {
        return Err(refused(format!(
            "{name} must be RFC3339 with an offset (2024-03-01T10:00:00Z or 2024-03-01T19:00:00+09:00), \
             with a T between date and time; a plain date or a time with no offset is refused, and so is \
             a space for the T, got {t:?}."
        )));
    }
    Ok(t)
}

pub(crate) fn bulk_parts(content: Option<&str>, opts: &BulkOpts) -> Result<serde_json::Map<String, serde_json::Value>, WosError> {
    let mut out = serde_json::Map::new();
    match (content, &opts.segments) {
        (Some(_), Some(_)) => return Err(refused("content and segments are exclusive: pass None as content to send segments.")),
        (None, None) => return Err(refused("add_bulk_opts needs content or segments.")),
        (Some(c), None) => {
            out.insert("content".into(), serde_json::json!(c));
            if let Some(who) = &opts.speaker {
                out.insert("metadata".into(), serde_json::json!({"speaker": written_speaker(who, "speaker")?}));
            }
        }
        (None, Some(segments)) => {
            if opts.speaker.is_some() {
                return Err(refused("speaker is for content: each segment names its own speaker."));
            }
            if segments.is_empty() {
                return Err(refused("segments must hold at least one segment."));
            }
            let mut list = Vec::with_capacity(segments.len());
            for (i, s) in segments.iter().enumerate() {
                if js_trim(&s.content).is_empty() {
                    return Err(refused(format!("segments[{i}].content is required.")));
                }
                let mut one = serde_json::Map::new();
                one.insert("content".into(), serde_json::json!(s.content));
                if let Some(who) = &s.speaker {
                    one.insert("speaker".into(), serde_json::json!(written_speaker(who, &format!("segments[{i}].speaker"))?));
                }
                if let Some(at) = &s.created_at {
                    let at = check_id(at, &format!("segments[{i}].created_at"))?;
                    one.insert("created_at".into(), serde_json::json!(at));
                }
                list.push(serde_json::Value::Object(one));
            }
            out.insert("segments".into(), serde_json::Value::Array(list));
        }
    }
    if let Some(t) = opts.timestamp.as_deref().filter(|t| !t.is_empty()) {
        out.insert("timestamp".into(), serde_json::json!(t));
    }
    Ok(out)
}

pub(crate) fn check_range<T: PartialOrd + std::fmt::Display>(name: &str, n: T, min: T, max: T) -> Result<T, WosError> {
    if n < min || n > max {
        return Err(refused(format!("{name} must be an integer between {min} and {max}, got {n}.")));
    }
    Ok(n)
}

pub(crate) fn nonblank<'a>(v: &'a str, refusal: &str) -> Result<&'a str, WosError> {
    let t = js_trim(v);
    if t.is_empty() {
        return Err(refused(refusal));
    }
    Ok(t)
}

pub(crate) fn check_id(v: &str, name: &str) -> Result<String, WosError> {
    let t = js_trim(v);
    if t.is_empty() {
        return Err(refused(format!("{name} is required.")));
    }
    Ok(t.to_string())
}

pub(crate) fn sleep_slot(slot: u32, what: &str) -> Result<u32, WosError> {
    if slot > 4 {
        return Err(refused(format!("{what} must be 0 (the core) or 1 to 4 (a box), got {slot}.")));
    }
    Ok(slot)
}

pub(crate) fn tz_minutes(tz: f64) -> Result<i32, WosError> {
    if !tz.is_finite() || tz.abs() > 14.0 {
        return Err(tz_refused(&js_num(tz)));
    }
    let minutes = tz * 60.0;
    let whole = minutes.floor();
    Ok((if minutes - whole >= 0.5 { whole + 1.0 } else { whole }) as i32)
}

fn tz_refused(shown: &str) -> WosError {
    refused(format!("tz must be a UTC offset in hours between -14 and 14 (e.g. 9, -4, 5.5), got {shown}."))
}

pub(crate) fn check_body_tz(body: &serde_json::Value) -> Result<(), WosError> {
    match body.get("tz") {
        None | Some(serde_json::Value::Null) => Ok(()),
        Some(serde_json::Value::Number(n)) => tz_minutes(n.as_f64().unwrap_or(f64::NAN)).map(drop),
        Some(serde_json::Value::String(s)) => Err(tz_refused(s)),
        Some(v) => Err(tz_refused(&v.to_string())),
    }
}

pub(crate) fn tz_hours(tz: f64) -> serde_json::Value {
    if tz.fract() == 0.0 { serde_json::json!(tz as i64) } else { serde_json::json!(tz) }
}

pub(crate) const ROOM_PARTS: &str = "man";
pub(crate) const ROOMS_MAX: usize = 9;

pub(crate) fn is_room_name(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 2 && ROOM_PARTS.as_bytes().contains(&b[0]) && (b'0'..=b'2').contains(&b[1])
}

pub(crate) fn room_selector(room: &str) -> Result<serde_json::Value, WosError> {
    if !is_room_name(room) {
        return Err(refused(format!("room must be one room name like \"m0\" or \"n1\", got {}.", js_quote(room))));
    }
    let b = room.as_bytes();
    let slot = ROOM_PARTS.bytes().position(|p| p == b[0]).unwrap_or_default();
    Ok(serde_json::json!({"days_ago": b[1] - b'0', "slot": slot}))
}

pub(crate) fn recall_extra(opts: &RecallOpts) -> Result<serde_json::Map<String, serde_json::Value>, WosError> {
    let mut extra = serde_json::Map::new();
    if let Some(n) = opts.limit {
        check_search_limit(n)?;
        extra.insert("limit".into(), serde_json::json!(n));
    }
    if let Some(n) = opts.context_limit {
        check_range("context_limit", n, CONTEXT_LIMIT_MIN, CONTEXT_LIMIT_MAX)?;
        extra.insert("context_limit".into(), serde_json::json!(n));
    }
    if let Some(room) = &opts.room {
        extra.insert("room".into(), room_selector(room)?);
    }
    if let Some(n) = opts.room_limit {
        extra.insert("room_limit".into(), serde_json::json!(check_range("room_limit", n, 1, 20)?));
    }
    if let Some(n) = opts.room_expand_tokens {
        extra.insert("room_expand_tokens".into(), serde_json::json!(check_range("room_expand_tokens", n, 0, 20_000)?));
    }
    for (k, v) in [("long_term", opts.long_term), ("long_term_cutoff", opts.long_term_cutoff)] {
        if let Some(b) = v {
            extra.insert(k.into(), serde_json::json!(b));
        }
    }
    for (k, v) in [("today_start", &opts.today_start), ("now", &opts.now)] {
        if let Some(t) = v {
            extra.insert(k.into(), serde_json::json!(check_id(t, k)?));
        }
    }
    if let Some(n) = opts.short_term_limit {
        extra.insert("short_term_limit".into(), serde_json::json!(check_range("short_term_limit", n, 0, 500)?));
    }
    if let Some(n) = opts.short_term_budget_tokens {
        let n = check_range("short_term_budget_tokens", n, 0, 8_000)?;
        extra.insert("short_term_budget_tokens".into(), serde_json::json!(n));
    }
    if let Some(n) = opts.max_images {
        extra.insert("max_images".into(), serde_json::json!(n));
    }
    if let Some(f) = opts.filters.as_ref().filter(|f| !f.is_null()) {
        extra.insert("filters".into(), f.clone());
    }
    if let Some(s) = &opts.speaker {
        extra.insert("speaker".into(), serde_json::json!(s));
    }
    if let Some(b) = opts.include_superseded {
        extra.insert("include_superseded".into(), serde_json::json!(b));
    }
    Ok(extra)
}

pub(crate) fn valid_model(model: &str) -> bool {
    !model.is_empty() && model.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

pub(crate) fn normalize_image_b64(s: &str) -> String {
    let s = js_trim(s);
    let body = s.strip_prefix("data:").and_then(|rest| rest.find(',').map(|i| &rest[i + 1..])).unwrap_or(s);
    body.chars().filter(|c| !js_space(*c)).collect()
}

pub(crate) fn normalize_image_field(image: &mut serde_json::Value) -> Result<(), WosError> {
    let obj = image.as_object_mut().ok_or_else(|| refused("image must be an object — {\"data\": \"<base64>\"}"))?;
    let raw = obj.get("data").and_then(|d| d.as_str()).filter(|d| !js_trim(d).is_empty());
    let raw = raw.ok_or_else(|| refused("image.data is required — base64 of the image (a data: URL is fine)"))?;
    let data = normalize_image_b64(raw);
    if data.is_empty() {
        return Err(refused("image.data is empty after stripping its data: prefix"));
    }
    obj.insert("data".into(), serde_json::Value::String(data));
    Ok(())
}
