use super::*;

pub(crate) const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const NO_BODY_STATUS: [u16; 3] = [204, 205, 304];

pub(crate) static WARNED_RETIREMENTS: WarnSet = std::sync::OnceLock::new();
pub(crate) const WARNED_RETIREMENTS_MAX: usize = 64;

pub(crate) fn utc_day(t: std::time::SystemTime) -> String {
    let (y, m, d) = civil_from_days(t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

pub(crate) fn http_date(s: &str) -> Option<std::time::SystemTime> {
    let s = s.trim();
    let Some((day, rest)) = s.split_once(", ").filter(|(day, rest)| day.len() > 3 && rest.len() == 22) else {
        return httpdate::parse_http_date(s).ok();
    };
    let (dd, mon, yy, time) = (rest.get(..2)?, rest.get(3..6)?, rest.get(7..9)?, rest.get(10..)?);
    if rest.get(2..3) != Some("-") || rest.get(6..7) != Some("-") || !yy.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let yy: i64 = yy.parse().ok()?;
    let now: i64 = httpdate::fmt_http_date(std::time::SystemTime::now()).get(12..16)?.parse().ok()?;
    let year = if 2000 + yy > now + 50 { 1900 + yy } else { 2000 + yy };
    httpdate::parse_http_date(&format!("{}, {dd} {mon} {year} {time}", day.get(..3)?)).ok()
}

pub(crate) fn warn_if_retiring(model: &str, headers: &reqwest::header::HeaderMap) -> bool {
    let Some(v) = headers.get("sunset").and_then(|v| v.to_str().ok()) else { return false };
    let Some(when) = http_date(v) else { return false };
    let day = utc_day(when);
    let seen = WARNED_RETIREMENTS.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    {
        let Ok(mut g) = seen.lock() else { return false };
        if g.len() >= WARNED_RETIREMENTS_MAX || !g.insert(format!("{model}|{day}")) {
            return false;
        }
    }
    let which = if model.is_empty() { "the model this client uses".to_string() } else { format!("model {model:?}") };
    eprintln!(
        "wontopos: {which} retires on {day} (UTC). From then on its calls answer 410, except \
         delete_store. list_models() lists the models you can move to."
    );
    true
}

pub(crate) fn new_idempotency_key() -> String {
    use std::hash::{BuildHasher, Hasher};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let half = |salt: u64| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(salt);
        h.write_u64(n);
        h.write_u128(now);
        h.finish()
    };
    format!("sdk-{:016x}{:016x}", half(0), half(1))
}

/// Quota from a response's `X-RateLimit-*` headers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RateLimit {
    pub limit: Option<u64>,
    pub remaining: Option<u64>,
    pub reset: Option<u64>,
}

pub(crate) fn parse_rate_limit(h: &reqwest::header::HeaderMap) -> Option<RateLimit> {
    let num = |name: &str| -> Option<u64> {
        let s = h.get(name)?.to_str().ok()?.trim_matches(|c: char| c == ' ' || c == '\t');
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse::<u64>().ok().filter(|n| *n < 1 << 53)
    };
    let rl = RateLimit { limit: num("x-ratelimit-limit"), remaining: num("x-ratelimit-remaining"), reset: num("x-ratelimit-reset") };
    (rl != RateLimit::default()).then_some(rl)
}

pub(crate) fn week_used_of(h: &reqwest::header::HeaderMap) -> Option<u8> {
    let s = h.get("x-wos-week-used")?.to_str().ok()?.trim_matches(|c: char| c == ' ' || c == '\t');
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u64>().ok().filter(|n| *n <= 100).map(|n| n as u8)
}

tokio::task_local! {
    pub(crate) static PAID_CALL: std::cell::Cell<bool>;
}

pub(crate) const PARTIAL_RESENDS: usize = 3;

pub(crate) const MAX_RETRY_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

pub(crate) fn fits_in(delay: std::time::Duration, deadline_at: Option<std::time::Instant>) -> bool {
    deadline_at.is_none_or(|at| delay < at.saturating_duration_since(std::time::Instant::now()))
}

pub(crate) fn ran_out_of_budget(e: &reqwest::Error, budget: std::time::Duration, per_attempt: std::time::Duration) -> bool {
    budget < per_attempt && e.is_timeout() && !e.is_connect()
}

pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub(crate) const STORE_TIMEOUT_SECS: u64 = 200;
pub(crate) const STORE_PATHS: [&str; 3] = ["/api/v1/memory/store", "/api/v1/memory/store-turn", "/api/v1/memory/bulk-store"];
pub(crate) const DEFAULT_RETRIES: u32 = 2;

pub(crate) fn build_http(timeout_secs: u64) -> HttpClient {
    HttpClient::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .connect_timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .user_agent(user_agent())
        .build()
        .expect("wontopos: failed to build HTTP client")
}

pub(crate) async fn read_capped(resp: reqwest::Response) -> Result<String, WosError> {
    Ok(answer_text(&read_capped_bytes(resp).await?))
}

pub(crate) fn answer_text(buf: &[u8]) -> String {
    String::from_utf8_lossy(buf.strip_prefix(b"\xef\xbb\xbf").unwrap_or(buf)).into_owned()
}

pub(crate) async fn read_capped_bytes(resp: reqwest::Response) -> Result<Vec<u8>, WosError> {
    let http = resp.status().as_u16();
    if let Some(cl) = resp.content_length() {
        if cl as usize > MAX_RESPONSE_BYTES {
            return Err(WosError::Api { status: http, message: format!("response too large ({cl} bytes) — refusing to buffer it") });
        }
    }
    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(network_error)?;
        if buf.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(WosError::Api { status: http, message: "response too large — refusing to buffer it".into() });
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

pub(crate) fn warn_if_plain_http(base_url: &str) {
    if plain_http_once("base_url", base_url) {
        eprintln!("wontopos: base_url uses plain HTTP on a non-local host, so the API key travels unencrypted. Use https://.");
    }
}

pub(crate) const WARNED_PLAIN_HTTP_MAX: usize = 1024;
pub(crate) static WARNED_PLAIN_HTTP: std::sync::OnceLock<std::sync::Mutex<std::collections::VecDeque<String>>> =
    std::sync::OnceLock::new();

pub(crate) fn plain_http_once(what: &str, base_url: &str) -> bool {
    if !plain_http_to_remote(base_url) {
        return false;
    }
    let key = format!("{what}\u{0}{}", js_trim(base_url).trim_end_matches('/'));
    let mut warned = lock(WARNED_PLAIN_HTTP.get_or_init(Default::default));
    if warned.contains(&key) {
        return false;
    }
    warned.push_back(key);
    while warned.len() > WARNED_PLAIN_HTTP_MAX {
        warned.pop_front();
    }
    true
}

pub(crate) const GIVE_THE_ORIGIN: &str = "the API origin without them, e.g. https://api.anthropic.com";

pub(crate) fn provider_url(raw: &str, wontopos_base: Option<&str>, keyed: bool, give: &str) -> Result<String, WosError> {
    if raw.contains(['?', '#']) {
        return Err(refused(format!("the model provider's base URL has a ? query or a # anchor: give {give}.")));
    }
    let base = provider_base(raw, wontopos_base)?;
    if keyed && plain_http_once("provider", &base) {
        eprintln!("wontopos: the model provider's base URL uses plain HTTP on a non-local host, so the model key travels unencrypted. Use https://.");
    }
    Ok(base)
}

pub(crate) fn plain_http_to_remote(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else { return false };
    if url.scheme() != "http" {
        return false;
    }
    let Some(host) = url.host_str() else { return false };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => !(ip.is_loopback() || ip.is_unspecified()),
        Err(_) => !host.eq_ignore_ascii_case("localhost"),
    }
}

pub(crate) fn redacted_base_url(base_url: &str) -> String {
    let is_scheme = |s: &str| {
        s.starts_with(|c: char| c.is_ascii_alphabetic())
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    };
    let usable = reqwest::Url::parse(base_url)
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some_and(|h| !h.is_empty()));
    if !usable {
        let Some(at) = base_url.rfind('@') else { return base_url.to_string() };
        let start = base_url[..at]
            .find("://")
            .filter(|&i| is_scheme(&base_url[..i]))
            .map_or(0, |i| i + 3);
        return format!("{}***@{}", &base_url[..start], &base_url[at + 1..]);
    }
    let lead = base_url.len() - base_url.trim_start_matches(|c: char| c <= ' ').len();
    let scheme = base_url[lead..].find(':').filter(|&i| is_scheme(&base_url[lead..lead + i])).map_or(lead, |i| lead + i + 1);
    let start = base_url.len() - base_url[scheme..].trim_start_matches(['/', '\\']).len();
    let end = base_url[start..].find(['/', '\\', '?', '#']).map_or(base_url.len(), |i| start + i);
    let Some(at) = base_url[start..end].rfind('@') else { return base_url.to_string() };
    format!("{}***@{}", &base_url[..start], &base_url[start + at + 1..])
}

pub(crate) fn far_future() -> std::time::Instant {
    std::time::Instant::now() + std::time::Duration::from_secs(60 * 60 * 24 * 365)
}

pub(crate) fn status_is_retryable(status: u16, idempotent: bool) -> bool {
    status == 429 || (matches!(status, 408 | 502 | 503 | 504) && idempotent)
}

pub(crate) fn status_is_ambiguous(status: u16) -> bool {
    matches!(status, 408 | 502 | 503 | 504)
}

pub(crate) fn retry_wait(attempt: u32, retry_after: Option<&str>) -> Option<std::time::Duration> {
    let Some(ra) = retry_after.map(str::trim).filter(|s| !s.is_empty()) else {
        return Some(backoff(attempt));
    };
    if ra.bytes().all(|b| b.is_ascii_digit()) {
        let secs = ra.parse::<u64>().unwrap_or(u64::MAX);
        return (secs <= MAX_RETRY_WAIT.as_secs()).then(|| std::time::Duration::from_secs(secs));
    }
    if let Some(when) = http_date(ra) {
        let delta = when.duration_since(std::time::SystemTime::now()).unwrap_or_default();
        return (delta <= MAX_RETRY_WAIT).then_some(delta);
    }
    Some(backoff(attempt))
}

pub(crate) fn write_lock_wait(text: &str) -> Option<std::time::Duration> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let err = v.get("error")?.as_object()?;
    if err.contains_key("conflicts_with") {
        return None;
    }
    let ms = err.get("retry_after_ms")?.as_f64()?;
    if !(ms.is_finite() && ms >= 0.0) {
        return None;
    }
    Some(std::time::Duration::from_millis(ms.ceil() as u64))
}

pub(crate) fn backoff(attempt: u32) -> std::time::Duration {
    use std::hash::{BuildHasher, Hasher};
    let base = 500u64.saturating_mul(1 << attempt.min(4)).min(8_000);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    std::time::Duration::from_millis(base + h.finish() % 250)
}

pub(crate) const MAX_ERR_MSG: usize = 4096;

pub(crate) fn cap_msg(s: String) -> String {
    if s.chars().count() <= MAX_ERR_MSG {
        return s;
    }
    let mut out: String = s.chars().take(MAX_ERR_MSG).collect();
    out.push_str("…(truncated)");
    out
}

pub(crate) fn strip_controls(s: &str) -> String {
    s.chars().filter(|c| *c >= ' ' && *c != '\u{7f}').collect()
}

pub(crate) fn parse_error(text: &str) -> (String, Option<String>) {
    let clean = |s: &str| cap_msg(strip_controls(s));
    let said = |v: Option<&serde_json::Value>| v.and_then(|s| s.as_str()).map(clean).filter(|s| !s.trim().is_empty());
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return (clean(text), None) };
    match v.get("error") {
        Some(serde_json::Value::String(s)) => (clean(s), None),
        Some(serde_json::Value::Object(obj)) => {
            let msg = said(obj.get("message")).or_else(|| said(obj.get("type"))).unwrap_or_else(|| clean(text));
            (msg, said(obj.get("request_id")).map(|id| clean_text(&id, 256)))
        }
        _ => (said(v.get("message")).unwrap_or_else(|| clean(text)), None),
    }
}

pub(crate) const READ_POSTS: &[&str] = &[
    "/api/v1/memory/search", "/api/v1/memory/recall", "/api/v1/memory/get", "/api/v1/memory/list", "/api/v1/memory/stats",
    "/api/v1/memory/history", "/api/v1/memory/lineage", "/api/v1/memory/by-speaker", "/api/v1/memory/images",
    "/api/v1/memory/image", "/api/v1/engram/run", "/api/v1/won/revisions", "/api/v1/memory/around", "/api/v1/memory/why",
    "/api/v1/memory/retrievals", "/api/v1/memory/sleep", "/api/v1/memory/work/state", "/api/v1/memory/sleep/verify",
    "/api/v1/memory/seen", "/api/v1/memory/channel/read", "/api/v1/memory/self", "/api/v1/memory/self/open",
    "/api/v1/memory/self/history", "/api/v1/memory/self/notes",
];

pub(crate) const RETRYABLE_BODY_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

pub(crate) struct Refusal {
    status: u16,
    message: String,
    wait: Option<std::time::Duration>,
    error: Option<serde_json::Map<String, serde_json::Value>>,
}

impl Refusal {
    pub(crate) async fn read(
        resp: reqwest::Response, idempotent: bool, attempt: u32, more: bool, deadline_at: Option<std::time::Instant>,
        attempt_left: std::time::Duration,
    ) -> Self {
        let status = resp.status().as_u16();
        let retry_after = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
        let planned = status_is_retryable(status, idempotent).then(|| retry_wait(attempt, retry_after.as_deref())).flatten();
        let text = if more && planned.is_some_and(|d| fits_in(d, deadline_at)) {
            let limit = RETRYABLE_BODY_WAIT.min(attempt_left);
            tokio::time::timeout(limit, read_capped(resp)).await.unwrap_or_else(|_| {
                let message = format!("HTTP {status} (the error body could not be read: not received within {limit:?})");
                Err(WosError::Api { status, message })
            })
        } else {
            read_capped(resp).await
        };
        let wait = match (&text, status) {
            (Ok(t), 409) => write_lock_wait(t).and_then(|asked| {
                let wait = asked.max(backoff(attempt));
                (asked <= MAX_RETRY_WAIT).then_some(wait)
            }),
            _ if status_is_retryable(status, idempotent) => planned,
            _ => None,
        };
        let error = match text.as_ref().ok().and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()) {
            Some(serde_json::Value::Object(mut o)) => match o.remove("error") {
                Some(serde_json::Value::Object(e)) => Some(e),
                _ => None,
            },
            _ => None,
        };
        let expired = error.as_ref().and_then(|e| e.get("code")).and_then(|c| c.as_str()) == Some("sleep_expired");
        let gone = |m: String| if status == 410 && !expired { format!("{m} {GONE_HINT}") } else { m };
        let message = match text {
            Ok(t) => {
                let (message, request_id) = parse_error(&t);
                let message = if message.trim().is_empty() { format!("HTTP {status}") } else { message };
                let message = gone(message);
                if let Some(id) = request_id { format!("{message} (request_id: {id})") } else { message }
            }
            Err(WosError::Api { message, .. }) => gone(message),
            Err(e) => gone(format!("the error body could not be read: {e}")),
        };
        Refusal { status, message, wait, error }
    }

    pub(crate) fn into_error(self, maybe_deleted: bool) -> WosError {
        let mut message = self.message;
        if maybe_deleted && self.status == 404 {
            message.push_str(" (an earlier attempt may already have deleted it)");
        }
        let field = |k: &str| self.error.as_ref().and_then(|e| e.get(k)).and_then(|c| c.as_str());
        let conflicts_with = if self.status == 409 { field("conflicts_with").map(strip_controls) } else { None };
        let billing = |k: &str| if self.status == 402 { field(k) } else { None };
        api_error(self.status, message, [field("code"), conflicts_with.as_deref(), billing("reason"), billing("resets_at")])
    }
}

impl Client {
    pub(crate) fn deadline_at(&self) -> Option<std::time::Instant> {
        self.deadline.map(|d| std::time::Instant::now().checked_add(d).unwrap_or_else(far_future))
    }

    pub(crate) fn attempt_budget(&self, deadline_at: Option<std::time::Instant>, per_attempt: std::time::Duration) -> Result<std::time::Duration, WosError> {
        let Some(at) = deadline_at else { return Ok(per_attempt) };
        let left = at.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(self.deadline_error());
        }
        Ok(per_attempt.min(left))
    }

    pub(crate) fn attempt_timeout(&self, path: &str) -> std::time::Duration {
        let path = path.split('?').next().unwrap_or(path);
        let secs = if STORE_PATHS.contains(&path) { self.store_timeout_secs } else { self.timeout_secs };
        std::time::Duration::from_secs(secs)
    }

    pub(crate) fn preflight(&self) -> Result<(), WosError> {
        if !self.model.is_empty() && !valid_model(&self.model) {
            return Err(WosError::Api {
                status: 400,
                message: format!("invalid model name: {:?} (letters, digits, '.', '_', '-' only)", self.model),
            });
        }
        self.check_api_key()?;
        if self.base_url.chars().any(|c| js_space(c) || matches!(c, '\0'..='\x1f' | '\x7f' | '\\')) {
            return Err(invalid_base_url("it contains whitespace, a backslash or a control character"));
        }
        if let Err(e) = reqwest::Url::parse(&self.base_url) {
            return Err(invalid_base_url(&e.to_string()));
        }
        if !self.base_url.split_once(':').is_some_and(|(_, rest)| rest.starts_with("//") && !rest.starts_with("///")) {
            return Err(invalid_base_url("its scheme is not followed by // and a host"));
        }
        if self.base_url.contains(['?', '#']) {
            return Err(invalid_base_url("it has a ? query or a # anchor"));
        }
        Ok(())
    }

    pub(crate) fn check_api_key(&self) -> Result<(), WosError> {
        if self.api_key.is_empty() {
            return Err(refused("api_key is required"));
        }
        if self.api_key.chars().any(js_space) {
            return Err(refused("api_key contains whitespace - check for a stray newline or paste error"));
        }
        if self.api_key.chars().any(|c| matches!(c, '\0'..='\x1f' | '\x7f')) {
            return Err(refused("api_key contains a control character - check for a paste error"));
        }
        if let Some(bad) = self.api_key.chars().find(|c| !c.is_ascii()) {
            return Err(refused(format!(
                "api_key contains a non-ASCII character ({bad:?}) - rich text turns '-' into an \
                 en dash; copy the key from a plain-text field"
            )));
        }
        if self.api_key.starts_with("sk-") {
            return Err(refused("api_key starts with sk-, so it is a model provider's key: give the Wontopos key here (it starts with wos-)."));
        }
        Ok(())
    }

    pub(crate) fn key_header(&self) -> reqwest::header::HeaderValue {
        let mut v = reqwest::header::HeaderValue::from_str(&self.api_key).unwrap_or_else(|_| reqwest::header::HeaderValue::from_static(""));
        v.set_sensitive(true);
        v
    }

    pub(crate) fn deadline_error(&self) -> WosError {
        WosError::Api { status: 0, message: format!("deadline of {:?} exhausted", self.deadline.unwrap_or_default()) }
    }

    pub(crate) fn record_billed_to(&self, headers: &reqwest::header::HeaderMap) {
        let balance = headers.get("x-wos-billed-to").and_then(|v| v.to_str().ok()).is_some_and(|v| v.trim().eq_ignore_ascii_case("balance"));
        self.billed_balance.store(balance, std::sync::atomic::Ordering::Relaxed);
        if balance {
            let _ = PAID_CALL.try_with(|paid| paid.set(true));
        }
    }

    pub(crate) fn record_week_used(&self, headers: &reqwest::header::HeaderMap, served: bool) {
        let week = week_used_of(headers);
        if week.is_some() || served {
            *lock(&self.week_used) = week;
        }
    }

    pub(crate) fn record_rate_limit(&self, headers: &reqwest::header::HeaderMap) {
        if let Some(rl) = parse_rate_limit(headers) {
            if let Ok(mut g) = self.rl.lock() {
                *g = Some(rl);
            }
        }
    }

    pub(crate) async fn request_bytes(&self, path: &str, body: &serde_json::Value) -> Result<(Vec<u8>, String), WosError> {
        let (status, headers, bytes) = self.exchange(reqwest::Method::POST, path, Some(body), None, None, false, None).await?;
        let ctype = headers.get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("application/octet-stream").to_string();
        if bytes.is_empty() {
            return Err(WosError::Api { status, message: "empty image body — the service returned no bytes".into() });
        }
        Ok((bytes, ctype))
    }

    pub(crate) async fn post(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value, WosError> {
        self.request(reqwest::Method::POST, path, Some(&body), None).await
    }

    pub(crate) async fn post_until_complete(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value, WosError> {
        let mut answer = self.post(path, body.clone()).await?;
        for _ in 0..PARTIAL_RESENDS {
            if answer.get("complete") != Some(&serde_json::Value::Bool(false)) {
                break;
            }
            answer = self.post(path, body.clone()).await?;
        }
        Ok(answer)
    }

    pub(crate) async fn post_idem(&self, path: &str, body: serde_json::Value, key: Option<&str>) -> Result<serde_json::Value, WosError> {
        let made = key.is_none().then(new_idempotency_key);
        self.send(reqwest::Method::POST, path, Some(&body), None, key.or(made.as_deref()), false, made.is_some()).await
    }

    pub(crate) async fn request(
        &self, method: reqwest::Method, path: &str, body: Option<&serde_json::Value>, query: Option<&[(&str, String)]>,
    ) -> Result<serde_json::Value, WosError> {
        self.request_with_key(method, path, body, query, None).await
    }

    pub(crate) async fn request_with_key(
        &self, method: reqwest::Method, path: &str, body: Option<&serde_json::Value>, query: Option<&[(&str, String)]>,
        idempotency_key: Option<&str>,
    ) -> Result<serde_json::Value, WosError> {
        let removes = method == reqwest::Method::DELETE;
        self.send(method, path, body, query, idempotency_key, removes, false).await
    }

    pub(crate) async fn write_or_refusal(
        &self, path: &str, body: serde_json::Value,
    ) -> Result<serde_json::Value, (WosError, Option<serde_json::Map<String, serde_json::Value>>)> {
        let said = std::sync::Mutex::new(None);
        let made = new_idempotency_key();
        self.send_full(reqwest::Method::POST, path, Some(&body), None, Some(&made), false, true, Some(&said))
            .await
            .map_err(|e| (e, lock(&said).take()))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn send(
        &self, method: reqwest::Method, path: &str, body: Option<&serde_json::Value>, query: Option<&[(&str, String)]>,
        idempotency_key: Option<&str>, removes: bool, made_key: bool,
    ) -> Result<serde_json::Value, WosError> {
        self.send_full(method, path, body, query, idempotency_key, removes, made_key, None).await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn exchange(
        &self, method: reqwest::Method, path: &str, body: Option<&serde_json::Value>, query: Option<&[(&str, String)]>,
        idempotency_key: Option<&str>, removes: bool, said: Option<&std::sync::Mutex<Option<serde_json::Map<String, serde_json::Value>>>>,
    ) -> Result<(u16, reqwest::header::HeaderMap, Vec<u8>), WosError> {
        if let Some(k) = idempotency_key.filter(|k| !valid_idempotency_key(k)) {
            return Err(refused(format!("invalid idempotency_key: {k:?} — 1-128 chars of [A-Za-z0-9._:-]")));
        }
        self.preflight()?;
        let key = self.key_header();
        let url = format!("{}{}", self.base_url, path);
        let attempts = self.retries.saturating_add(1);
        let deadline_at = self.deadline_at();
        let reads = method.is_idempotent()
            || (method == reqwest::Method::POST && READ_POSTS.contains(&path.split('?').next().unwrap_or(path)));
        let replay_safe = reads || idempotency_key.is_some();
        let mut maybe_applied = false;
        let per_attempt = self.attempt_timeout(path);
        let mut last_refusal: Option<Refusal> = None;
        let mut attempt: u32 = 0;
        loop {
            let start = std::time::Instant::now();
            let previous = last_refusal.take();
            let budget = match self.attempt_budget(deadline_at, per_attempt) {
                Ok(b) => b,
                Err(e) => return Err(previous.map_or(e, |r| r.into_error(removes && maybe_applied))),
            };
            let mut req = self.http.request(method.clone(), &url).timeout(budget).header("X-API-Key", key.clone());
            if !self.model.is_empty() {
                req = req.header("X-WOS-Model", &self.model);
            }
            if let Some(k) = idempotency_key {
                req = req.header("Idempotency-Key", k);
            }
            if let Some(b) = body {
                req = req.json(b);
            }
            if let Some(q) = query {
                req = req.query(q);
            }
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) if e.is_builder() => return Err(builder_error(e)),
                Err(e) => {
                    if ran_out_of_budget(&e, budget, per_attempt) {
                        return Err(previous.filter(|_| reads).map_or_else(|| self.deadline_error(), |r| r.into_error(removes)));
                    }
                    let safe = e.is_connect() || (replay_safe && !e.is_timeout());
                    if safe && attempt + 1 < attempts {
                        let delay = backoff(attempt);
                        if !fits_in(delay, deadline_at) {
                            return Err(previous.map_or_else(|| self.deadline_error(), |r| r.into_error(removes && maybe_applied)));
                        }
                        let what = if e.is_connect() { "connect error" } else { "transport error" };
                        log_debug!("{method} {path}: {what} — retrying in {}ms (attempt {}/{attempts})", delay.as_millis(), attempt + 1);
                        maybe_applied |= !e.is_connect();
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(network_error(e));
                }
            };
            let status = resp.status();
            self.record_week_used(resp.headers(), status.is_success());
            if status.is_redirection() {
                let message = "unexpected redirect — refused (the API key never follows a redirect). Check base_url: exact host, https://.";
                return Err(WosError::Api { status: status.as_u16(), message: message.into() });
            }
            self.record_rate_limit(resp.headers());
            warn_if_retiring(&self.model, resp.headers());
            if status.is_success() {
                self.record_billed_to(resp.headers());
            }
            if !status.is_success() {
                let left = budget.saturating_sub(start.elapsed());
                let refusal = Refusal::read(resp, replay_safe, attempt, attempt + 1 < attempts, deadline_at, left).await;
                if let Some(delay) = refusal.wait {
                    if attempt + 1 < attempts && fits_in(delay, deadline_at) {
                        log_debug!("{method} {path} -> {} — retrying in {}ms (attempt {}/{attempts})", status.as_u16(), delay.as_millis(), attempt + 1);
                        maybe_applied |= status_is_ambiguous(status.as_u16());
                        last_refusal = Some(refusal);
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                }
                log_debug!("{method} {path} -> {} in {}ms (attempt {}/{attempts})", status.as_u16(), start.elapsed().as_millis(), attempt + 1);
                if let Some(said) = said {
                    *lock(said) = refusal.error.clone();
                }
                return Err(refusal.into_error(removes && maybe_applied));
            }
            let headers = resp.headers().clone();
            let buf = match read_capped_bytes(resp).await {
                Ok(b) => b,
                Err(WosError::Network(ne)) if ran_out_of_budget(&ne, budget, per_attempt) => {
                    return Err(previous.filter(|_| reads).map_or_else(|| self.deadline_error(), |r| r.into_error(removes)));
                }
                Err(e) => {
                    let retry_body = matches!(&e, WosError::Network(ne) if replay_safe && !ne.is_timeout());
                    if retry_body && attempt + 1 < attempts {
                        let delay = backoff(attempt);
                        if !fits_in(delay, deadline_at) {
                            return Err(previous.map_or_else(|| self.deadline_error(), |r| r.into_error(removes && maybe_applied)));
                        }
                        log_debug!("{method} {path}: body dropped — retrying in {}ms (attempt {}/{attempts})", delay.as_millis(), attempt + 1);
                        maybe_applied = true;
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(e);
                }
            };
            log_debug!("{method} {path} -> {} in {}ms (attempt {}/{attempts})", status.as_u16(), start.elapsed().as_millis(), attempt + 1);
            return Ok((status.as_u16(), headers, buf));
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn send_full(
        &self, method: reqwest::Method, path: &str, body: Option<&serde_json::Value>, query: Option<&[(&str, String)]>,
        idempotency_key: Option<&str>, removes: bool, made_key: bool,
        said: Option<&std::sync::Mutex<Option<serde_json::Map<String, serde_json::Value>>>>,
    ) -> Result<serde_json::Value, WosError> {
        let (status, headers, buf) = self.exchange(method, path, body, query, idempotency_key, removes, said).await?;
        let text = answer_text(&buf);
        if text.trim().is_empty() {
            if NO_BODY_STATUS.contains(&status) {
                return Ok(serde_json::json!({}));
            }
            return Err(WosError::Api { status, message: "empty response body — expected a JSON object".into() });
        }
        let mut v = serde_json::from_str::<serde_json::Value>(&text)
            .map_err(|e| WosError::Api { status, message: format!("invalid JSON in response: {e}") })?;
        if !v.is_object() {
            return Err(WosError::Api { status, message: "expected a JSON object in the response".into() });
        }
        if !made_key && headers.get("idempotent-replayed").and_then(|v| v.to_str().ok()) == Some("true") {
            if let Some(o) = v.as_object_mut() {
                o.entry("replayed").or_insert(serde_json::Value::Bool(true));
            }
        }
        Ok(v)
    }
}
