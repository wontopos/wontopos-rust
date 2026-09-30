//! Wontopos — long-term memory for AI, in a few lines.
//!
//! ```no_run
//! use wontopos::Client;
//! # async fn run() -> Result<(), wontopos::WosError> {
//! let mem = Client::new("wos-...").with_user("alice");  // set the store once
//! mem.create_store(None).await?;                        // create it (uses the client's user)
//! mem.add("she prefers tea over coffee", None, serde_json::json!({})).await?;
//! let hits = mem.search("what does alice drink?", None, 10).await?;
//! # Ok(()) }
//! ```
//!
//! Pass `"alice"` for a specific store or `None` to use the client default (set via
//! [`Client::with_user`]). Stores are explicit: the store must exist first
//! ([`Client::create_store`]) or calls return 404. Every account starts with a
//! `default` store, so with `None` everywhere the zero-setup path just works.
//!
//! Recall quality does not depend on which language a memory was written in: a
//! memory stored in one language is found by a question asked in another. Storing
//! and searching call no LLM.
//!
//! Reliability: every call retries transient failures with exponential backoff
//! and jitter, honoring `Retry-After` up to 30s. 429, and a 409 saying another write
//! to the store was in flight, are retried on every call; 408/502/503/504 and network
//! errors only when a retry cannot apply a write twice (idempotent calls, or a failure
//! at connect time). Timeouts are never retried: the write may already have been
//! applied. A `Retry-After` above 30s, or a wait that does not fit in the
//! [`Client::with_deadline`] budget, returns that response's error at once.
//! Requests time out after 30s (connect 10s). Tune with
//! [`Client::with_retries`] (0 disables) and [`Client::with_timeout`].
//!
//! Runtime: every call is async and needs a Tokio 1.x runtime with IO and time
//! enabled (`#[tokio::main]` enables both). There is no blocking client.
//!
//! Debugging: set `WONTOPOS_LOG=debug` to log method/path/status/timing/retries
//! to stderr — never memory content, request bodies, or the API key.
//!
//! Security posture (not configurable off): TLS 1.2 is the floor and certificate
//! verification can never be disabled; redirects are refused so the API key can
//! never follow one to another host; responses over 64MB are refused; `{:?}` on
//! a client masks the key. Prefer [`Client::from_env`] over keys in source code.
//!
use reqwest::Client as HttpClient;
use serde::Deserialize;

const DEFAULT_BASE_URL: &str = "https://api.wontopos.com";
/// Runtime info helps support debug a report ("linux on arm...") — platform
/// only, never anything identifying.
fn user_agent() -> String {
    format!(
        "wontopos-rust/{} ({}-{})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// Wire-level debug logging: set `WONTOPOS_LOG=debug`. Logs method/path/status/
/// timing/retries to stderr — NEVER memory content, request bodies, or the key.
fn log_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("WONTOPOS_LOG").map(|v| v.eq_ignore_ascii_case("debug")).unwrap_or(false)
    })
}

macro_rules! log_debug {
    ($($arg:tt)*) => {
        if log_enabled() {
            eprintln!("wontopos: {}", format_args!($($arg)*));
        }
    };
}
/// Refuse to buffer absurd responses (real ones are a few KB) — protects the
/// process if a custom base_url points somewhere broken or hostile.
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
/// Statuses that legitimately carry NO body (RFC 9110). Everything else must
/// answer with a JSON object — see the empty-body check in `request`.
const NO_BODY_STATUS: [u16; 3] = [204, 205, 304];

/// A store id is 1-64 ASCII letters, digits, `.`, `_` and `-`, starting with a letter or
/// digit. Creating any other id (an email address, a name in another script) is refused
/// (400), so key stores on an id of your own. Store ids compare without regard to case:
/// `Alice` and `alice` name one store. Ids that differ only in `.`, `_` or `-` cannot
/// both exist: once `alice-smith` exists, creating `alice.smith` is refused (409) and
/// using it answers 404. With one store per end user, derive the ids so two users never
/// differ only in those three characters.
fn normalize_store_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' { c } else { '_' }
        })
        .collect()
}

/// Ids already warned about, newest last so the oldest is the one evicted.
/// Bounded — see the eviction note in `warn_if_store_id_collapses`.
const WARNED_STORE_IDS_MAX: usize = 1024;
static WARNED_STORE_IDS: std::sync::OnceLock<std::sync::Mutex<std::collections::VecDeque<String>>> =
    std::sync::OnceLock::new();

/// Refuse a metadata key that spells a store id or the idempotency key, compared in
/// any case with spaces, `_`, `-` and `.` ignored. Those values have their own
/// parameters.
fn check_metadata_keys(metadata: &serde_json::Value) -> Result<(), WosError> {
    if let Some(obj) = metadata.as_object() {
        for k in obj.keys() {
            let folded: String = k
                .chars()
                .filter(|c| !c.is_whitespace() && !matches!(c, '_' | '-' | '.'))
                .flat_map(char::to_lowercase)
                .collect();
            if matches!(folded.as_str(), "userid" | "storeid" | "idempotencykey") {
                return Err(WosError::Api {
                    status: 400,
                    message: format!(
                        "{k:?} is not a metadata field: pass the store as user_id and an \
                         idempotency key through the *_idempotent methods. Nothing was sent."
                    ),
                });
            }
        }
    }
    Ok(())
}

/// The store ids the API accepts: 1-64 ASCII letters, digits, `.`, `_` and `-`,
/// starting with a letter or digit.
fn valid_store_id_format(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

fn warn_if_store_id_collapses(id: &str) {
    if id.is_empty() {
        return;
    }
    let valid = valid_store_id_format(id);
    let normalized = normalize_store_id(id);
    if valid && normalized == id {
        return;
    }
    let seen = WARNED_STORE_IDS.get_or_init(|| std::sync::Mutex::new(std::collections::VecDeque::new()));
    // Losing the warning because the lock is poisoned is worse than repeating it.
    let first = match seen.lock() {
        Ok(mut g) => {
            if g.iter().any(|s| s == id) {
                false
            } else {
                // Bounded: one store per end user means one entry per user. Evict
                // oldest-first, so an id that first appears late still gets its warning.
                g.push_back(id.to_string());
                while g.len() > WARNED_STORE_IDS_MAX {
                    g.pop_front();
                }
                true
            }
        }
        Err(_) => true,
    };
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

/// The filter keys the API acts on. Anything else it DROPS silently, so a typo
/// widens the search instead of failing. We warn rather than reject: the API may
/// grow a key before this crate is updated, and blocking it would be worse.
const KNOWN_FILTER_KEYS: [&str; 6] = [
    "categories",
    "event_from",
    "event_to",
    "time_from",
    "time_to",
    "min_importance",
];

/// The metadata keys the service keeps. It drops every other key on the way in, so a
/// misspelled `speaker` stores an untagged memory.
const KNOWN_METADATA_KEYS: [&str; 4] = ["speaker", "event_date", "category", "conversation_id"];

/// Same cap, same reason, as the store-id warn set: an app forwarding user-supplied
/// keys would otherwise grow these sets forever. Losing an entry costs a repeated
/// warning, never correctness.
const WARNED_KEYS_MAX: usize = 1024;
type WarnSet = std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>>;
static WARNED_FILTER_KEYS: WarnSet = std::sync::OnceLock::new();
static WARNED_METADATA_KEYS: WarnSet = std::sync::OnceLock::new();

/// Record `key` in a bounded warn-once set. True the first time it is seen.
fn first_sighting(set: &WarnSet, key: &str) -> bool {
    let seen = set.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    // Losing the warning because the lock is poisoned is worse than repeating it.
    let Ok(mut g) = seen.lock() else { return true };
    let inserted = g.insert(key.to_string());
    while g.len() > WARNED_KEYS_MAX {
        let Some(victim) = g.iter().next().cloned() else { break };
        g.remove(&victim);
    }
    inserted
}

fn warn_on_unknown_filters(body: &serde_json::Value) {
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

fn warn_on_unknown_metadata(metadata: &serde_json::Value) {
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

/// Test hook — the warn-once sets are process-global by design.
#[doc(hidden)]
pub fn _reset_warning_state() {
    if let Some(m) = WARNED_STORE_IDS.get() {
        if let Ok(mut g) = m.lock() { g.clear(); }
    }
    for set in [&WARNED_FILTER_KEYS, &WARNED_METADATA_KEYS] {
        if let Some(m) = set.get() {
            if let Ok(mut g) = m.lock() { g.clear(); }
        }
    }
}

/// What the API accepts as an `Idempotency-Key`: 1-128 chars of `[A-Za-z0-9._:-]`.
/// Checked client-side so a bad key fails before the request rather than coming back
/// as a 400 mid-retry, which reads as "my write failed" when it never ran.
fn valid_idempotency_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 128
        && k.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}
const ENV_KEYS: [&str; 2] = ["WONTOPOS_API_KEY", "WOS_API_KEY"];

/// Error returned by the Wontopos API.
#[derive(Debug)]
pub enum WosError {
    /// Network / transport failure.
    Network(reqwest::Error),
    /// API responded with a non-2xx status. When the server sent a request id,
    /// `message` ends with `(request_id: ...)` — include it when contacting support.
    Api { status: u16, message: String },
}

impl std::fmt::Display for WosError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WosError::Network(e) => {
                // Never the URL: its query carries store ids.
                let text = e.to_string();
                let text = text.split(" for url (").next().unwrap_or("");
                match network_cause(e) {
                    Some(cause) => write!(f, "network error: {text} ({cause})"),
                    None => write!(f, "network error: {text}"),
                }
            }
            WosError::Api { status, message } => write!(f, "[{status}] {message}"),
        }
    }
}
impl std::error::Error for WosError {
    /// The transport error behind a [`WosError::Network`].
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WosError::Network(e) => Some(e),
            WosError::Api { .. } => None,
        }
    }
}
impl From<reqwest::Error> for WosError {
    fn from(e: reqwest::Error) -> Self {
        // An `error_for_status()` failure carries the HTTP status — surface it
        // as Api (kind() = Auth/NotFound/...), not as a bogus Connection error.
        match e.status() {
            Some(s) => WosError::Api { status: s.as_u16(), message: e.without_url().to_string() },
            None => network_error(e),
        }
    }
}

/// A transport failure as this crate reports it: without the request URL, whose
/// query can carry store ids.
fn network_error(e: reqwest::Error) -> WosError {
    WosError::Network(e.without_url())
}

/// A request that could not be built never left this process and never will on a
/// retry. Every header is checked before this point, so what is left is the URL.
fn builder_error(e: reqwest::Error) -> WosError {
    let why = std::error::Error::source(&e)
        .map(|s| s.to_string())
        .unwrap_or_else(|| "not a usable URL".into());
    invalid_base_url(&why)
}

fn invalid_base_url(why: &str) -> WosError {
    WosError::Api {
        status: 400,
        message: format!("invalid base_url ({why}). Pass the service root, e.g. {DEFAULT_BASE_URL}"),
    }
}

/// A short name for why a transport call failed, read from the error chain.
fn network_cause(e: &reqwest::Error) -> Option<&'static str> {
    if e.is_timeout() {
        return Some("timed out");
    }
    let mut next: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    let mut tcp_stage = false;
    while let Some(s) = next {
        if let Some(io) = s.downcast_ref::<std::io::Error>() {
            match io.kind() {
                std::io::ErrorKind::ConnectionRefused => return Some("connection refused"),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted => {
                    return Some("connection reset")
                }
                std::io::ErrorKind::TimedOut => return Some("timed out"),
                _ => {}
            }
        }
        let text = s.to_string().to_ascii_lowercase();
        if text.contains("dns error") || text.contains("failed to lookup address") {
            return Some("dns");
        }
        if text.contains("certificate") || text.contains("tls") || text.contains("ssl") || text.contains("handshake") {
            return Some("tls");
        }
        if text.contains("connection closed") || text.contains("end of file") {
            return Some("connection closed");
        }
        tcp_stage |= text.starts_with("tcp ");
        next = s.source();
    }
    if e.is_connect() {
        // A connect failure outside the TCP and DNS steps is the TLS handshake.
        return Some(if tcp_stage { "connect failed" } else { "tls" });
    }
    if e.is_body() || e.is_decode() {
        return Some("response body interrupted");
    }
    None
}

/// Coarse classification of a failure — match on this instead of raw status codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// No response reached us (DNS/TLS/timeout/connection).
    Connection,
    /// 400, 413 or 422: the request was refused as sent (malformed, too large, or an
    /// idempotency key reused with a different body).
    BadRequest,
    /// 401 — missing or invalid API key.
    Auth,
    /// 402 — no card on file or depleted balance.
    PaymentRequired,
    /// 403 — not allowed.
    PermissionDenied,
    /// 404 — the store or resource doesn't exist.
    NotFound,
    /// 409: another write to this store was in flight (retried automatically; nothing
    /// was stored), or the store id collides with an existing one (not retried).
    Conflict,
    /// 429 — rate limited.
    RateLimited,
    /// 5xx — server failure.
    ///
    /// `502` / `503` / `504` are transient: this client already retries them where a
    /// retry cannot apply a write twice. `501` is NOT: the selected model does not
    /// implement that endpoint, so retrying can never succeed. Pick a model that
    /// supports it ([`Client::list_models`]) instead.
    Server,
    /// Any other status.
    Other,
}

impl WosError {
    /// The HTTP status, or `None` for a transport/connection failure.
    pub fn status(&self) -> Option<u16> {
        match self {
            WosError::Api { status, .. } => Some(*status),
            WosError::Network(_) => None,
        }
    }
    /// Classify the failure so callers can branch without matching raw status codes.
    pub fn kind(&self) -> ErrorKind {
        match self {
            WosError::Network(_) => ErrorKind::Connection,
            WosError::Api { status, .. } => match status {
                // An exhausted deadline, and only that: no response arrived. The 64MB
                // cap carries the response's own status and the page-walk ceiling
                // carries 200, so neither lands here. Every caller mistake is 400.
                0 => ErrorKind::Connection,
                400 | 413 | 422 => ErrorKind::BadRequest,
                401 => ErrorKind::Auth,
                402 => ErrorKind::PaymentRequired,
                403 => ErrorKind::PermissionDenied,
                404 => ErrorKind::NotFound,
                409 => ErrorKind::Conflict,
                429 => ErrorKind::RateLimited,
                500..=599 => ErrorKind::Server,
                _ => ErrorKind::Other,
            },
        }
    }
    /// True for a 429 rate-limit response.
    pub fn is_rate_limited(&self) -> bool {
        self.kind() == ErrorKind::RateLimited
    }
    /// True for a 401 auth failure.
    pub fn is_auth(&self) -> bool {
        self.kind() == ErrorKind::Auth
    }
}

/// Quota from a response's `X-RateLimit-*` headers (`None` fields when a header is absent).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RateLimit {
    pub limit: Option<u64>,
    pub remaining: Option<u64>,
    pub reset: Option<u64>,
}

fn parse_rate_limit(h: &reqwest::header::HeaderMap) -> Option<RateLimit> {
    let num = |name: &str| -> Option<u64> {
        h.get(name).and_then(|v| v.to_str().ok()).and_then(|s| s.parse::<u64>().ok())
    };
    let rl = RateLimit {
        limit: num("x-ratelimit-limit"),
        remaining: num("x-ratelimit-remaining"),
        reset: num("x-ratelimit-reset"),
    };
    if rl == RateLimit::default() {
        None
    } else {
        Some(rl)
    }
}

/// Deserialize a field the server should always send, falling back to the type's
/// default when it is `null` or will not convert, so one odd field does not cost the
/// caller the whole record.
fn null_to_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let v = serde_json::Value::deserialize(d)?;
    if v.is_null() {
        return Ok(T::default());
    }
    Ok(serde_json::from_value(v).unwrap_or_default())
}

/// One retrieved memory. Known fields are typed; everything else (e.g. `speaker`:
/// `"me"` for the assistant's own words, or a person's name) lands in `extra`.
#[derive(Debug, Deserialize)]
pub struct Memory {
    #[serde(default, deserialize_with = "null_to_default")]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "null_to_default")]
    pub content: String,
    /// Cognitive category, e.g. "general".
    #[serde(default, deserialize_with = "null_to_default")]
    pub category: String,
    /// Raw closeness to the query — higher is closer. NOT the ranking key: results
    /// already arrive best-first, and what produces that order is internal and not
    /// returned, so re-sorting by `similarity` overrides the ranking and makes results
    /// worse. Take the list in the order given. There is no `score` field.
    #[serde(default, deserialize_with = "null_to_default")]
    pub similarity: f64,
    /// Importance weight the service assigns to the memory.
    #[serde(default, deserialize_with = "null_to_default")]
    pub importance: f64,
    /// Month bucket, e.g. "2026-07". Absent when temporal fields are stripped.
    #[serde(default, deserialize_with = "null_to_default")]
    pub time_bucket: Option<String>,
    /// True if a later memory has superseded this one.
    #[serde(default, deserialize_with = "null_to_default")]
    pub is_superseded: bool,
    /// Id of the memory that superseded this one, if any.
    #[serde(default, deserialize_with = "null_to_default")]
    pub superseded_by: Option<String>,
    /// When the memory was stored (RFC3339). Absent when temporal fields are stripped.
    #[serde(default, deserialize_with = "null_to_default")]
    pub created_at: Option<String>,
    /// When the content actually happened (RFC3339), if known.
    #[serde(default, deserialize_with = "null_to_default")]
    pub event_date: Option<String>,
    /// WHO said it: `"me"` for the agent's own words, or a registered person's name.
    /// `None` when the memory carries no speaker tag.
    #[serde(default, deserialize_with = "null_to_default")]
    pub speaker: Option<String>,
    /// The memory's time written in the requested delivery form — `"a couple weeks
    /// ago"` (memoir) or `"2 weeks ago (Jun 09)"` (archive). Present only when the
    /// call asked for a form on a model whose `capabilities` include `forms`.
    #[serde(default, deserialize_with = "null_to_default")]
    pub time: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// One-call LLM context: short-term turns + long-term matches + surrounding context.
///
/// Fields this struct does not name land in `extra`. It is `#[non_exhaustive]`:
/// destructure with a trailing `..` and build one with `serde_json::from_value`.
#[derive(Debug, Deserialize)]
#[non_exhaustive]
pub struct RecallResponse {
    #[serde(default)]
    pub short_term: serde_json::Value,
    #[serde(default)]
    pub long_term: serde_json::Value,
    #[serde(default)]
    pub context: serde_json::Value,
    /// Every other field of the reply, as it arrived.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Per-call search options with a fixed shape — see [`Client::search_opts`].
///
/// `Default` is "ask once, one image at most", which is what the API does when neither
/// field is sent, so `..Default::default()` never turns anything on behind your back.
#[derive(Debug, Clone, Default)]
pub struct SearchOpts {
    /// Re-ask passes, 0–3. After the first answer the service asks again up to this
    /// many times, and each extra pass reaches memories the earlier ones did not. No
    /// LLM runs at any value. It stops early when a pass finds nothing new; the
    /// response's `verify_used` says how many actually ran.
    ///
    /// Each pass can add up to `limit` more memories, and delivered memories are
    /// billed, so it costs more. Helps most on questions needing several distinct
    /// memories from far apart in the history; does little on a single-fact lookup.
    ///
    /// Needs a model whose [`Client::list_models`] `capabilities` include `re_ask`.
    /// Any other refuses the call (403).
    pub verify: Option<u8>,
    /// How many image memories the answer may carry, 0–5. `None` lets the service use
    /// one; `Some(0)` asks for none. Out of range is refused before sending, not
    /// clamped: quietly cutting 6 to 5 would leave you believing you got six.
    ///
    /// Needs a model whose [`Client::list_models`] `capabilities` include `images`.
    /// Any other refuses the call (403) rather than answering with no images.
    pub max_images: Option<usize>,
}

/// Per-call options for [`Client::recall_opts`].
///
/// `Default` is "ten memories, ten pieces of context", which is what the service does when
/// neither field is sent, so `..Default::default()` never changes the answer behind your back.
#[derive(Debug, Clone, Default)]
pub struct RecallOpts {
    /// Long-term memories to recall, 5–20 (default 10). Out of range is refused, not
    /// clamped — asking for 20 and silently getting 10 reads as "that is all there is".
    ///
    /// A model that recalls a fixed ten refuses the call (403) rather than answering
    /// with a number you did not ask for.
    pub limit: Option<usize>,
    /// How much surrounding context is attached around the best match, 0–20 (default 10).
    /// `Some(0)` attaches none. Same model rule as `limit`.
    pub context_limit: Option<usize>,
}

/// [`Client::search_self`] result: general memories and the assistant's own, kept apart.
#[derive(Debug)]
pub struct SelfSearch {
    /// What others said, and general memories.
    pub memories: Vec<Memory>,
    /// The assistant's own words (stored with speaker "me"); empty on a model whose
    /// [`Client::list_models`] `capabilities` do not include `self_memories`.
    pub self_memories: Vec<Memory>,
}

/// Everything one search answered with, not just the merged memories.
///
/// [`Client::search`] returns every memory as one `Vec`. This keeps the fields apart
/// and adds `verify_used`, the report on the `verify` option.
///
/// Search `filters` apply to `memories`; `self_memories` are not filtered.
#[derive(Debug, Default)]
pub struct SearchFull {
    /// What others said, and general memories.
    pub memories: Vec<Memory>,
    /// The assistant's own words (speaker "me"); empty on a model whose
    /// `capabilities` do not include `self_memories`. Not narrowed by `filters`.
    pub self_memories: Vec<Memory>,
    /// Image memories the answer carried (one by default on a model whose
    /// `capabilities` include `images`, up to `max_images`); empty when there were none.
    pub images: Vec<Memory>,
    /// Re-ask passes actually performed. `None` unless `verify` was sent; lower than
    /// requested means the store had nothing further to add.
    pub verify_used: Option<u8>,
    /// The response as it arrived, for anything this struct does not name yet.
    pub raw: serde_json::Value,
}

/// The model every call uses unless the caller names another; pin another with
/// `Client::new(key).with_model("tablet-1")`. What each model can do is listed in
/// [`Client::list_models`] `capabilities`.
///
/// Every model on the shared pool lists, fetches and deletes the same memories, but a
/// search may not find memories stored through a different model. Store and search
/// with the same one.
const DEFAULT_MODEL: &str = "tablet-2";

/// A page walk stops after this many pages: at 100 per page that is two million
/// memories, past any real store. Reaching it is an error, never a quiet end, because a
/// truncated `Vec` looks exactly like a complete one.
const MAX_PAGES: u32 = 20_000;

/// The count shared by `search` and `recall`. 5 to 20 inclusive.
pub const SEARCH_LIMIT_MIN: usize = 5;
/// The count shared by `search` and `recall`. 5 to 20 inclusive.
pub const SEARCH_LIMIT_MAX: usize = 20;
/// `recall`'s surrounding-context count. 0 to 20 inclusive; 0 attaches none.
pub const CONTEXT_LIMIT_MIN: usize = 0;
/// `recall`'s surrounding-context count. 0 to 20 inclusive; 0 attaches none.
pub const CONTEXT_LIMIT_MAX: usize = 20;

/// One page of images, a speaker's words, or revisions: 5 to 20 inclusive.
const PAGE_LIMIT_MIN: usize = 5;
const PAGE_LIMIT_MAX: usize = 20;
/// One page of `list_memories`: 1 to 500 inclusive. The service reads anything
/// above 500 as 500, so it is refused here instead.
const LIST_LIMIT_MIN: usize = 1;
const LIST_LIMIT_MAX: usize = 500;
/// Image memories one search may carry: 0 to 5 inclusive.
const MAX_IMAGES_MAX: usize = 5;

/// The longest wait this client sleeps before a retry. A service asking for more gets
/// its answer handed back at once instead of a sleep the caller did not plan for.
const MAX_RETRY_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Whether sleeping `delay` still leaves time for another attempt inside the budget.
fn fits_in(delay: std::time::Duration, deadline_at: Option<std::time::Instant>) -> bool {
    match deadline_at {
        Some(at) => delay < at.saturating_duration_since(std::time::Instant::now()),
        None => true,
    }
}

/// Whether a transport failure is the attempt's own timeout firing at a budget that
/// was cut to what was left of the deadline: the deadline ran out mid-attempt.
fn ran_out_of_budget(e: &reqwest::Error, budget: std::time::Duration, per_attempt: std::time::Duration) -> bool {
    budget < per_attempt && e.is_timeout() && !e.is_connect()
}

/// `recall`'s `context_limit`, 0 to 20, refused rather than adjusted. `Some(0)` is a
/// real answer ("attach none"), not a missing value, so it passes.
fn check_context_limit(n: usize) -> Result<(), WosError> {
    // A range, not two comparisons: `n < CONTEXT_LIMIT_MIN` can never be true for a
    // usize, and a guard half of which is dead is a guard nobody can read.
    if !(CONTEXT_LIMIT_MIN..=CONTEXT_LIMIT_MAX).contains(&n) {
        return Err(WosError::Api {
            status: 400,
            message: format!(
                "context_limit must be between {CONTEXT_LIMIT_MIN} and {CONTEXT_LIMIT_MAX}, got {n}."
            ),
        });
    }
    Ok(())
}

fn check_search_limit(limit: usize) -> Result<(), WosError> {
    if !(SEARCH_LIMIT_MIN..=SEARCH_LIMIT_MAX).contains(&limit) {
        return Err(WosError::Api {
            status: 400,
            message: format!(
                "limit must be between {SEARCH_LIMIT_MIN} and {SEARCH_LIMIT_MAX}, got {limit}. \
                 Out of range is refused rather than adjusted, so a short answer always \
                 means the store was short."
            ),
        });
    }
    Ok(())
}

/// A page size for images, a speaker's words or revisions: 5 to 20, refused rather
/// than adjusted.
fn check_page_limit(name: &str, n: usize) -> Result<(), WosError> {
    if !(PAGE_LIMIT_MIN..=PAGE_LIMIT_MAX).contains(&n) {
        return Err(WosError::Api {
            status: 400,
            message: format!("{name} must be between {PAGE_LIMIT_MIN} and {PAGE_LIMIT_MAX}, got {n}."),
        });
    }
    Ok(())
}

fn check_list_limit(n: usize) -> Result<(), WosError> {
    if !(LIST_LIMIT_MIN..=LIST_LIMIT_MAX).contains(&n) {
        return Err(WosError::Api {
            status: 400,
            message: format!("limit must be between {LIST_LIMIT_MIN} and {LIST_LIMIT_MAX}, got {n}."),
        });
    }
    Ok(())
}

/// `max_images` in a search body: a whole number from 0 to 5. JSON `true`, `2.5` or
/// `"3"` is refused here rather than read by the service as something else. `null`
/// reads as absent (the default of one).
fn check_max_images(body: &serde_json::Value) -> Result<(), WosError> {
    let Some(v) = body.get("max_images").filter(|v| !v.is_null()) else { return Ok(()) };
    match v.as_u64() {
        Some(n) if n <= MAX_IMAGES_MAX as u64 => Ok(()),
        _ => Err(WosError::Api {
            status: 400,
            message: format!("max_images must be a whole number between 0 and {MAX_IMAGES_MAX}, got {v}."),
        }),
    }
}
const DEFAULT_USER: &str = "default";
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_RETRIES: u32 = 2;

/// `wos-abc...wxyz` — enough to tell keys apart, never enough to use.
fn mask_key(key: &str) -> String {
    // Index by CHARACTER, not byte: byte-slicing a key with a multibyte char at
    // the boundary panics ("byte index is not a char boundary"), and Debug on the
    // client reaches here with arbitrary input.
    let chars: Vec<char> = key.chars().collect();
    if chars.len() > 12 {
        let first: String = chars[..4].iter().collect();
        let last: String = chars[chars.len() - 4..].iter().collect();
        format!("{first}...{last}")
    } else {
        "***".to_string()
    }
}

fn build_http(timeout_secs: u64) -> HttpClient {
    HttpClient::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .connect_timeout(std::time::Duration::from_secs(10))
        // Never follow a redirect: the API key would be forwarded to wherever a
        // 3xx points. The API never legitimately redirects.
        .redirect(reqwest::redirect::Policy::none())
        // TLS 1.2 floor; certificate verification is never disabled.
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .user_agent(user_agent())
        .build()
        .expect("wontopos: failed to build HTTP client")
}

/// Model names travel in a header — only header-safe characters.
fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}


/// Read the body with a hard size cap, so a broken/hostile endpoint can't make
/// the process buffer gigabytes.
async fn read_capped(resp: reqwest::Response) -> Result<String, WosError> {
    Ok(String::from_utf8_lossy(&read_capped_bytes(resp).await?).into_owned())
}

/// The same ceiling, for a body that is not text. One implementation for the JSON and
/// the image paths, so the two cannot drift apart.
async fn read_capped_bytes(resp: reqwest::Response) -> Result<Vec<u8>, WosError> {
    // The response's own status, not 0: a body did arrive. 0 means nothing came back
    // and reads as `ErrorKind::Connection`, which callers retry.
    let http = resp.status().as_u16();
    if let Some(cl) = resp.content_length() {
        if cl as usize > MAX_RESPONSE_BYTES {
            return Err(WosError::Api {
                status: http,
                message: format!("response too large ({cl} bytes) — refusing to buffer it"),
            });
        }
    }
    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(network_error)?;
        if buf.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(WosError::Api {
                status: http,
                message: "response too large — refusing to buffer it".into(),
            });
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// An API key on plain HTTP travels readable by anyone on the path. Loopback is
/// fine (local dev, or a proxy on the same box); anything else gets a warning,
/// not an error, so private-network gateways keep working.
fn warn_if_plain_http(base_url: &str) {
    if plain_http_to_remote(base_url) {
        eprintln!("wontopos: base_url uses plain HTTP on a non-local host, so the API key travels unencrypted. Use https://.");
    }
}

/// True when `base_url` would carry the key over plain HTTP to a host other than this
/// machine. Read with the parser the transport uses, so userinfo, `\`, `?` or `#`
/// cannot make a remote host look like loopback. A URL it cannot parse is left to the
/// request, which refuses it.
fn plain_http_to_remote(base_url: &str) -> bool {
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

/// `base_url` for display, with any user name and password replaced by `***`.
/// Everything between the scheme and the last '@' is hidden, so a password the URL
/// parser does not read as one (a '/', '?', '#' or '@' in it, or no scheme) is too.
fn redacted_base_url(base_url: &str) -> String {
    let Some(at) = base_url.rfind('@') else { return base_url.to_string() };
    let is_scheme = |s: &str| {
        s.starts_with(|c: char| c.is_ascii_alphabetic())
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    };
    let start = base_url[..at]
        .find("://")
        .filter(|&i| is_scheme(&base_url[..i]))
        .map_or(0, |i| i + 3);
    format!("{}***@{}", &base_url[..start], &base_url[at + 1..])
}

/// An instant far enough out that nothing waits for it — what a deadline past the
/// clock's range means in practice.
fn far_future() -> std::time::Instant {
    std::time::Instant::now() + std::time::Duration::from_secs(60 * 60 * 24 * 365)
}

/// Whether a status may be sent again, given whether the method can be replayed.
/// 429 is refused before the request is processed, so nothing was written and any
/// method may retry. The rest are ambiguous for a write: 408, 502, 503 and 504 can
/// arrive after the write was applied, so they retry only when the method is
/// idempotent. A 409 is decided from its body; see `write_lock_wait`.
fn status_is_retryable(status: u16, idempotent: bool) -> bool {
    status == 429 || (matches!(status, 408 | 502 | 503 | 504) && idempotent)
}

/// A status that leaves open whether the request was applied.
fn status_is_ambiguous(status: u16) -> bool {
    matches!(status, 408 | 502 | 503 | 504)
}

/// The wait before retry `attempt` (0-based). `Retry-After` counts only as
/// delta-seconds (digits) or an HTTP-date; anything else in it falls back to the
/// backoff. `None` when it asks for more than 30s: the caller then gets the response
/// instead of a sleep.
fn retry_wait(attempt: u32, retry_after: Option<&str>) -> Option<std::time::Duration> {
    let Some(ra) = retry_after.map(str::trim).filter(|s| !s.is_empty()) else {
        return Some(backoff(attempt));
    };
    if ra.bytes().all(|b| b.is_ascii_digit()) {
        // Too many digits for u64 is still a number of seconds, and above the cap.
        let secs = ra.parse::<u64>().unwrap_or(u64::MAX);
        return (secs <= MAX_RETRY_WAIT.as_secs()).then(|| std::time::Duration::from_secs(secs));
    }
    // HTTP-date form (RFC 9110), e.g. "Wed, 21 Oct 2015 07:28:00 GMT". A past date
    // means retry now.
    if let Ok(when) = httpdate::parse_http_date(ra) {
        let delta = when.duration_since(std::time::SystemTime::now()).unwrap_or_default();
        return (delta <= MAX_RETRY_WAIT).then_some(delta);
    }
    Some(backoff(attempt))
}

/// The wait a 409 asks for when another write to the store was in flight: the body
/// carries `error.retry_after_ms` and no `error.conflicts_with`. `None` for a store-id
/// collision, which no retry can fix.
fn write_lock_wait(text: &str) -> Option<std::time::Duration> {
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

/// Exponential backoff with jitter for retry `attempt` (0-based): 0.5s doubling to 8s.
fn backoff(attempt: u32) -> std::time::Duration {
    use std::hash::{BuildHasher, Hasher};
    let base = 500u64.saturating_mul(1 << attempt.min(4)).min(8_000);
    // Jitter without a rand dependency: each RandomState has fresh random keys, so
    // the hash differs per call even where the clock is coarse.
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    std::time::Duration::from_millis(base + h.finish() % 250)
}

/// Cap a server-controlled error message so a hostile body can't become a huge
/// error string / log line (chars, not bytes — never split a UTF-8 boundary).
const MAX_ERR_MSG: usize = 4096;

fn cap_msg(s: String) -> String {
    if s.chars().count() <= MAX_ERR_MSG {
        return s;
    }
    let mut out: String = s.chars().take(MAX_ERR_MSG).collect();
    out.push_str("…(truncated)");
    out
}

/// Put an image's base64 into the shape the API expects.
///
/// Two things get stripped, and both of them otherwise fail late and opaquely:
///
/// A `data:image/...;base64,` prefix, because that is what browsers and file pickers
/// hand you, and passing it through fails with "not a readable image" — an error that
/// points at the image rather than at the extra 23 characters in front of it.
///
/// Every line break, because `base64` and `openssl base64` wrap at 76 columns and
/// pasting a file made that way carries the newlines into the payload. The base64
/// alphabet has no whitespace in it.
fn normalize_image_b64(s: &str) -> String {
    let s = s.trim();
    let body = match s.strip_prefix("data:") {
        Some(rest) => match rest.find(',') {
            Some(i) => &rest[i + 1..],
            None => s,
        },
        None => s,
    };
    body.split_whitespace().collect()
}

/// Normalise an `image` object in place: `{"data": ..., "reference"?, "taken_at"?}`.
///
/// Sizes are left to the service, which refuses what it cannot take: a request body
/// over 10MB, an edge under 700px. A long edge over 1568px is downscaled, not refused.
fn normalize_image_field(image: &mut serde_json::Value) -> Result<(), WosError> {
    let bad = |m: &str| WosError::Api { status: 400, message: m.to_string() };
    let obj = image
        .as_object_mut()
        .ok_or_else(|| bad("image must be an object — {\"data\": \"<base64>\"}"))?;
    let raw = obj
        .get("data")
        .and_then(|d| d.as_str())
        .ok_or_else(|| bad("image.data is required — base64 of the image (a data: URL is fine)"))?;
    let data = normalize_image_b64(raw);
    if data.is_empty() {
        return Err(bad("image.data is empty after stripping its data: prefix"));
    }
    obj.insert("data".into(), serde_json::Value::String(data));
    Ok(())
}

/// Server text goes into error messages, and from there into logs and terminals.
/// Control characters (C0 and DEL) are dropped so it cannot forge log lines or send
/// escape sequences.
fn strip_controls(s: &str) -> String {
    s.chars().filter(|c| *c >= ' ' && *c != '\u{7f}').collect()
}

/// Server may return either the envelope
/// `{"type":"error","error":{"type":...,"message":...,"request_id":...}}`
/// or simple `{"error":"reason"}`. Falls back to the raw text. A `message` that is
/// empty or not a string falls back to `type`, then to the raw text; the request id is
/// kept either way. The message may come back empty (an empty body).
fn parse_error(text: &str) -> (String, Option<String>) {
    let clean = |s: &str| cap_msg(strip_controls(s));
    // A string that is blank once cleaned says nothing.
    let said = |v: Option<&serde_json::Value>| {
        v.and_then(|s| s.as_str()).map(clean).filter(|s| !s.trim().is_empty())
    };
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        match v.get("error") {
            Some(serde_json::Value::String(s)) => return (clean(s), None),
            Some(serde_json::Value::Object(obj)) => {
                let msg = said(obj.get("message"))
                    .or_else(|| said(obj.get("type")))
                    .unwrap_or_else(|| clean(text));
                return (msg, said(obj.get("request_id")));
            }
            _ => {}
        }
    }
    (clean(text), None)
}

/// How long a retryable answer's error body may take to arrive.
const RETRYABLE_BODY_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// A non-2xx answer: the error it becomes, and whether and when it may be sent again.
struct Refusal {
    status: u16,
    message: String,
    /// The wait before another attempt, when the status allows one.
    wait: Option<std::time::Duration>,
}

impl Refusal {
    /// Read a non-2xx response. A body that cannot be read still yields an error with
    /// this status, saying why: the status is the answer, so a lost body does not make
    /// the call one to send again.
    ///
    /// When another attempt is expected to follow whatever the body says, the body gets
    /// at most `RETRYABLE_BODY_WAIT` of `attempt_left`; if it has not arrived by then,
    /// the message says so.
    async fn read(
        resp: reqwest::Response,
        idempotent: bool,
        attempt: u32,
        more: bool,
        deadline_at: Option<std::time::Instant>,
        attempt_left: std::time::Duration,
    ) -> Self {
        let status = resp.status().as_u16();
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let planned = if status_is_retryable(status, idempotent) {
            retry_wait(attempt, retry_after.as_deref())
        } else {
            None
        };
        let text = if more && planned.is_some_and(|d| fits_in(d, deadline_at)) {
            let limit = RETRYABLE_BODY_WAIT.min(attempt_left);
            tokio::time::timeout(limit, read_capped(resp)).await.unwrap_or_else(|_| {
                Err(WosError::Api {
                    status,
                    message: format!("HTTP {status} (the error body could not be read: not received within {limit:?})"),
                })
            })
        } else {
            read_capped(resp).await
        };
        let wait = match (&text, status) {
            (Ok(t), 409) => write_lock_wait(t).and_then(|asked| {
                // Never shorter than the backoff, never longer than the cap.
                let wait = asked.max(backoff(attempt));
                (asked <= MAX_RETRY_WAIT).then_some(wait)
            }),
            _ if status_is_retryable(status, idempotent) => planned,
            _ => None,
        };
        let message = match text {
            Ok(t) => {
                let (message, request_id) = parse_error(&t);
                let message = if message.trim().is_empty() { format!("HTTP {status}") } else { message };
                match request_id {
                    Some(id) => format!("{message} (request_id: {id})"),
                    None => message,
                }
            }
            Err(WosError::Api { message, .. }) => message,
            Err(e) => format!("the error body could not be read: {e}"),
        };
        Refusal { status, message, wait }
    }

    /// The error to return. `maybe_deleted` marks a DELETE whose earlier attempt may
    /// have been applied, so a 404 now can mean it already happened.
    fn into_error(self, maybe_deleted: bool) -> WosError {
        let mut message = self.message;
        if maybe_deleted && self.status == 404 {
            message.push_str(" (an earlier attempt may already have deleted it)");
        }
        WosError::Api { status: self.status, message }
    }
}

/// Wontopos memory client. Memories are isolated per `user_id`.
///
/// The API key picks *which memory* (your account); the model picks *which engine*
/// reads it. Set a default with [`Client::with_model`], or
/// override for a single call by chaining it:
///
/// ```no_run
/// # use wontopos::Client;
/// # async fn run() -> Result<(), wontopos::WosError> {
/// let mem = Client::new("wos-...");                          // tablet-2
/// mem.recall("...", "alice").await?;                        // tablet-2
/// mem.with_model("tablet-1").recall("...", "alice").await?; // this call only
/// # Ok(()) }
/// ```
pub struct Client {
    api_key: String,
    base_url: String,
    model: String,
    /// The store every call uses unless one passes `Some(user_id)`.
    default_user: String,
    timeout_secs: u64,
    /// A TOTAL budget for one call, across every attempt; `timeout_secs` bounds one
    /// attempt. `None` means no overall budget.
    deadline: Option<std::time::Duration>,
    retries: u32,
    http: HttpClient,
    /// Quota from the most recent response's `X-RateLimit-*` headers. Interior
    /// mutability so `&self` request methods can refresh it.
    rl: std::sync::Mutex<Option<RateLimit>>,
}

/// Back-compat alias.
pub type WME = Client;

/// Every memory a search returned — `memories`, then `self_memories`, then `images` —
/// as one `Vec`, de-duplicated by id. Each keeps its `speaker`, and a photo carries
/// `image_ref` in `extra`, so the caller can still tell them apart.
fn merge_results(v: &serde_json::Value) -> Vec<Memory> {
    let mut out = memories_from(v.get("memories"));
    let mut seen: std::collections::HashSet<String> =
        out.iter().filter_map(|m| m.id.clone()).collect();
    for m in memories_from(v.get("self_memories")).into_iter().chain(memories_from(v.get("images"))) {
        if let Some(id) = &m.id {
            if !seen.insert(id.clone()) {
                continue;
            }
        }
        out.push(m);
    }
    out
}

/// Parse a memory array element-wise, KEEPING every valid record.
///
/// A missing field or a `null` scalar inside one memory is tolerated (see
/// `null_to_default`). An element that is not an object (`null`, a number, a string)
/// is skipped, so one bad element never costs the good memories beside it. A missing
/// value or non-array yields an empty vec.
fn memories_from(v: Option<&serde_json::Value>) -> Vec<Memory> {
    match v.and_then(|x| x.as_array()) {
        Some(arr) => arr
            .iter()
            .filter_map(|el| serde_json::from_value::<Memory>(el.clone()).ok())
            .collect(),
        None => Vec::new(),
    }
}

/// The objects in the array at `v[key]`. Any element that is not an object is
/// skipped; a missing value or non-array yields an empty vec.
fn records(v: &serde_json::Value, key: &str) -> Vec<serde_json::Value> {
    v.get(key)
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter(|x| x.is_object()).cloned().collect())
        .unwrap_or_default()
}

/// The error a page walk returns when it cannot reach the end of the store. `why`
/// says what stopped it and that the store did not end.
fn truncated_walk(why: &str) -> WosError {
    // 200, not 0: every page answered. This is not a failed connection.
    WosError::Api {
        status: 200,
        message: format!("{why}. This is a truncated answer, not the whole store."),
    }
}

/// A walk that reached `MAX_PAGES` pages.
fn page_ceiling() -> WosError {
    truncated_walk(&format!("stopped after {MAX_PAGES} pages — the store did not end"))
}

/// A walk the service answered with a cursor it had already given.
fn repeated_cursor() -> WosError {
    truncated_walk("the service handed back a cursor it had already given, so the store did not end")
}

/// `get` answers `{"memory": {...}}` on some models and the row itself on others.
fn memory_from_get(v: serde_json::Value) -> serde_json::Value {
    if let Some(m) = v.get("memory").filter(|m| m.is_object()) {
        return m.clone();
    }
    if v.get("id").is_some_and(|id| id.is_string()) {
        v
    } else {
        serde_json::json!({})
    }
}

// Never show the key or base_url credentials — debug output ends up in logs.
impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &redacted_base_url(&self.base_url))
            .field("model", &self.model)
            .field("user", &self.default_user)
            .field("api_key", &mask_key(&self.api_key))
            .finish()
    }
}

impl Client {
    /// Create a client against the hosted service.
    pub fn new(api_key: &str) -> Self {
        Self::with_base_url(api_key, DEFAULT_BASE_URL)
    }

    /// A client whose key comes from `WONTOPOS_API_KEY` (or `WOS_API_KEY`) —
    /// keeps keys out of source code.
    pub fn from_env() -> Result<Self, WosError> {
        for name in ENV_KEYS {
            if let Ok(key) = std::env::var(name) {
                if !key.trim().is_empty() {
                    return Ok(Self::new(&key));
                }
            }
        }
        Err(WosError::Api {
            status: 400,
            message: format!("set {} (or {}) in the environment", ENV_KEYS[0], ENV_KEYS[1]),
        })
    }

    /// Create a client against a custom base URL (a dedicated region, a proxy, a test server).
    /// The key is trimmed — a stray newline from a file or env var otherwise
    /// turns into a mystery 401.
    pub fn with_base_url(api_key: &str, base_url: &str) -> Self {
        warn_if_plain_http(base_url);
        Self {
            api_key: api_key.trim().to_string(),
            base_url: base_url.trim().trim_end_matches('/').to_string(),
            model: DEFAULT_MODEL.to_string(),
            default_user: DEFAULT_USER.to_string(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            retries: DEFAULT_RETRIES,
            http: build_http(DEFAULT_TIMEOUT_SECS),
            deadline: None,
            rl: std::sync::Mutex::new(None),
        }
    }

    fn clone_with(&self, model: Option<&str>, user: Option<&str>) -> Self {
        Self {
            api_key: self.api_key.clone(),
            base_url: self.base_url.clone(),
            model: model.unwrap_or(&self.model).to_string(),
            default_user: user.unwrap_or(&self.default_user).to_string(),
            timeout_secs: self.timeout_secs,
            deadline: self.deadline,
            retries: self.retries,
            http: self.http.clone(),
            // A derived client has not made a call yet, so it has no snapshot.
            rl: std::sync::Mutex::new(None),
        }
    }

    /// Quota from the MOST RECENT call: `{ limit, remaining, reset }` (or `None` before
    /// the first call). Read it to self-throttle — the client already retries 429s, but
    /// this lets you slow down before hitting the wall.
    pub fn rate_limit(&self) -> Option<RateLimit> {
        self.rl.lock().ok().and_then(|g| *g)
    }

    /// Return a client that uses `model` (sent as `X-WOS-Model`). Cheap clone —
    /// use it to set the default (`Client::new(k).with_model("tablet-1")`) or to
    /// override one call (`client.with_model("tablet-1").recall(...)`).
    pub fn with_model(&self, model: &str) -> Self {
        self.clone_with(Some(model), None)
    }

    /// Return a client bound to `user_id` as its default store. Then every call can
    /// omit the store by passing `None`, e.g. `client.with_user("alice").search("q", None, 10)`.
    pub fn with_user(&self, user_id: &str) -> Self {
        self.clone_with(None, Some(user_id))
    }

    /// A client with a TOTAL budget per call, across every retry (everything else
    /// kept) — `mem.with_deadline(Duration::from_secs(5))` inside a handler that has
    /// five seconds.
    ///
    /// [`Client::with_timeout`] bounds ONE attempt. A zero duration is treated as no
    /// budget, the same way a zero timeout falls back to the default — a budget of
    /// zero would fail every call before it started.
    pub fn with_deadline(&self, deadline: std::time::Duration) -> Self {
        let mut c = self.clone_with(None, None);
        c.deadline = if deadline.is_zero() { None } else { Some(deadline) };
        c
    }

    /// Where this call's budget runs out. Computed ONCE per call, never per attempt —
    /// a budget recomputed each attempt is the per-attempt timeout under another name.
    fn deadline_at(&self) -> Option<std::time::Instant> {
        // `Instant + Duration` panics on overflow, and `with_deadline(Duration::MAX)` is a
        // natural spelling of "no budget". Saturate: a budget past the clock's range
        // never runs out.
        self.deadline
            .map(|d| std::time::Instant::now().checked_add(d).unwrap_or_else(far_future))
    }

    /// How long THIS attempt may take: the per-attempt timeout, or what is left of the
    /// budget, whichever is smaller. Errors when the budget is already gone, so no
    /// socket is opened that there is no time to use.
    fn attempt_budget(
        &self,
        deadline_at: Option<std::time::Instant>,
    ) -> Result<std::time::Duration, WosError> {
        let per_attempt = std::time::Duration::from_secs(self.timeout_secs);
        let Some(at) = deadline_at else { return Ok(per_attempt) };
        let left = at.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(self.deadline_error());
        }
        Ok(per_attempt.min(left))
    }

    /// Return a client with a different per-request timeout (each retry attempt).
    /// `0` falls back to the default (30s) — a zero timeout would fail every request.
    pub fn with_timeout(&self, timeout_secs: u64) -> Self {
        let timeout_secs = if timeout_secs == 0 { DEFAULT_TIMEOUT_SECS } else { timeout_secs };
        let mut c = self.clone_with(None, None);
        c.timeout_secs = timeout_secs;
        c.http = build_http(timeout_secs);
        c
    }

    /// Return a client that retries transient failures `retries` times before giving
    /// up: 429, a 409 for a write in flight, and 408/502/503/504 and connect errors
    /// where a retry cannot apply a write twice. 0 disables retries (default 2).
    pub fn with_retries(&self, retries: u32) -> Self {
        let mut c = self.clone_with(None, None);
        c.retries = retries;
        c
    }

    /// Resolve a call's store: the explicit `Some(user_id)`, else the client default.
    fn uid(&self, user_id: Option<&str>) -> Result<String, WosError> {
        // An omitted id uses the client default — the documented shortcut. A PASSED but
        // blank id is a bug at the call site: the caller computed a tenant id and got
        // nothing back. Falling back would silently write one end user's memories into
        // another store, so it is refused.
        if matches!(user_id, Some(u) if u.trim().is_empty()) {
            return Err(WosError::Api {
                status: 400,
                message: "user_id was passed but is blank. Omit it to use the client's default \
                          store, or pass a real store id — a blank id would silently write into \
                          a different store."
                    .into(),
            });
        }
        // Non-blank by the guard above. Sent as written, not trimmed: trimming would be a
        // second silent change of destination for ids that already work.
        let sid = match user_id {
            Some(u) => u.to_string(),
            None => self.default_user.clone(),
        };
        // The guard above only sees a PASSED id. `with_user("")` cannot return an error,
        // so the RESOLVED id is checked here too.
        if sid.trim().is_empty() {
            return Err(WosError::Api {
                status: 400,
                message: "this client's default store is blank — with_user(\"\") leaves it that way. \
                          Pass a real store id, or build the client without with_user to use `default`."
                    .into(),
            });
        }
        warn_if_store_id_collapses(&sid);
        Ok(sid)
    }

    /// Available models: `[{ "id", "name", "available", "memory", "capabilities" }, ...]`.
    /// Needs no API key. Elements that are not objects are skipped.
    ///
    /// `capabilities` says what the model can do (`forms`, `images`, `re_ask`,
    /// `self_memories`, `engrams`, `speaker_names`); check it before relying on one.
    /// `memory` is `"shared"` or `"isolated"`. Every model on the shared pool lists,
    /// fetches and deletes the same memories, but a search may not find memories stored
    /// through a different model, so store and search with the same one.
    pub async fn list_models(&self) -> Result<Vec<serde_json::Value>, WosError> {
        let v = self.request(reqwest::Method::GET, "/api/v1/models", None, None).await?;
        Ok(records(&v, "models"))
    }

    /// The engrams (and delivery forms) the selected model can actually run.
    ///
    /// Ask rather than hard-code: a name copied from the docs freezes a caller to the
    /// catalogue as it was that day, and anything added later stays invisible. The
    /// service is the authority, and the answer depends on the model (its
    /// [`Client::list_models`] `capabilities` list `engrams` and `forms`), so use
    /// [`Client::with_model`] to ask about another one.
    ///
    /// Returns the whole object: `{ engrams: [...], forms: [...], note? }`. `note`
    /// explains an empty `engrams` on a model without engram support.
    pub async fn list_engrams(&self) -> Result<serde_json::Value, WosError> {
        self.request(reqwest::Method::GET, "/api/v1/engram", None, None).await
    }

    // ----- write -----
    // `user_id` takes `impl Into<Option<&str>>` everywhere below: pass `"alice"` for
    // a specific store, or `None` to use the client default (see `with_user`).

    /// Store one memory. `metadata` may be `json!({})` — or carry a speaker:
    /// `json!({"speaker": "me"})` for the assistant's own words, or a registered
    /// person's name (see [`Client::add_speaker`]).
    ///
    /// The service keeps four metadata keys: `speaker`, `event_date`, `category` and
    /// `conversation_id`. Every other key is dropped, and this client warns once per
    /// unknown key on stderr. `event_date` takes RFC3339 or a plain date (YYYY-MM-DD);
    /// a value that is not a date is refused (400) naming the field.
    pub async fn add(&self, content: &str, user_id: impl Into<Option<&str>>, metadata: serde_json::Value) -> Result<serde_json::Value, WosError> {
        self.store_one(content, user_id, metadata, serde_json::json!({}), None).await
    }

    /// Alias of `add` — store one memory.
    pub async fn store(&self, content: &str, user_id: impl Into<Option<&str>>, metadata: serde_json::Value) -> Result<serde_json::Value, WosError> {
        self.add(content, user_id, metadata).await
    }

    /// Like [`Client::add`], but merges extra body fields — e.g.
    /// `json!({"image": {"data": "<base64>"}})` to store an image with the memory.
    /// Reserved fields (user_id, content, metadata) are set last so they win.
    ///
    /// **This is where images go; there is no `add_image`.** Images need a model whose
    /// [`Client::list_models`] `capabilities` include `images`.
    ///
    /// `content` is required even with an image: it is the caption, and an empty one is
    /// refused (400). Inside `image`, `taken_at` (RFC3339 or a plain date, YYYY-MM-DD,
    /// usually from EXIF) fills `event_date` when that is empty, so the memory sorts by
    /// when the image was TAKEN rather than by when it was uploaded; a value that is not
    /// a date is refused (400) naming the field. `image.data` is normalised on the way
    /// through: a `data:...;base64,` prefix is dropped and all whitespace, including the
    /// newlines a wrapped base64 file carries, is removed. Nothing else is changed —
    /// URL-safe base64 is not converted, so send the standard alphabet.
    ///
    /// `image.reference` is a string of your own (stored, never fetched). When it is
    /// sent, the service keeps no image bytes: [`Client::get_image`] answers 404 for
    /// that memory, and you fetch the picture from your reference.
    ///
    /// Limits: the request body is at most 10MB, both edges must be 700px or more (a
    /// smaller image is refused, 400), and a long edge over 1568px is downscaled to 1568.
    /// Downscaling re-encodes: lossless formats are written as WebP, so a PNG comes back
    /// from [`Client::get_image`] as `image/webp`; JPEG stays JPEG. Under 1568px the
    /// bytes are untouched. Keep the full-resolution file yourself.
    pub async fn add_with(&self, content: &str, user_id: impl Into<Option<&str>>, metadata: serde_json::Value, extra: serde_json::Value) -> Result<serde_json::Value, WosError> {
        self.store_one(content, user_id, metadata, extra, None).await
    }

    /// [`Client::add_with`] and [`Client::add_idempotent`] in one call: carry an image
    /// AND make re-running the write safe.
    pub async fn add_with_idempotent(
        &self,
        content: &str,
        user_id: impl Into<Option<&str>>,
        metadata: serde_json::Value,
        extra: serde_json::Value,
        idempotency_key: &str,
    ) -> Result<serde_json::Value, WosError> {
        self.store_one(content, user_id, metadata, extra, Some(idempotency_key)).await
    }

    /// The body builder behind [`Client::add_with`], [`Client::add_with_idempotent`]
    /// and [`Client::add_idempotent`], which passes no `extra`. `extra` goes in first
    /// and reserved fields after, so forwarded JSON cannot point the write at another
    /// store.
    async fn store_one(
        &self,
        content: &str,
        user_id: impl Into<Option<&str>>,
        metadata: serde_json::Value,
        extra: serde_json::Value,
        idempotency_key: Option<&str>,
    ) -> Result<serde_json::Value, WosError> {
        check_metadata_keys(&metadata)?;
        let user_id = self.uid(user_id.into())?;
        warn_on_unknown_metadata(&metadata);
        let mut obj = serde_json::Map::new();
        if let Some(e) = extra.as_object() {
            for (k, v) in e {
                obj.insert(k.clone(), v.clone());
            }
        }
        if let Some(image) = obj.get_mut("image") {
            normalize_image_field(image)?;
        }
        obj.insert("user_id".into(), serde_json::json!(user_id));
        obj.insert("content".into(), serde_json::json!(content));
        obj.insert("metadata".into(), metadata);
        self.post_idem("/api/v1/memory/store", serde_json::Value::Object(obj), idempotency_key)
            .await
    }

    /// Store a conversation turn (user + assistant). Payload first, user_id last — same shape as `add`/`search`.
    pub async fn add_turn(&self, user_msg: &str, assistant_msg: &str, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/memory/store-turn", serde_json::json!({"user_id": user_id, "user_msg": user_msg, "assistant_msg": assistant_msg})).await
    }

    /// Bulk-ingest a large blob of text in one call.
    ///
    /// An empty `category` is sent as it is, and the service decides what to do with
    /// it.
    ///
    /// To date the ingested memories, use [`Client::add_bulk_with`] — `add_bulk`
    /// cannot take a `timestamp` and every memory would carry the upload time.
    pub async fn add_bulk(&self, content: &str, user_id: impl Into<Option<&str>>, category: &str) -> Result<serde_json::Value, WosError> {
        self.add_bulk_with(content, user_id, category, serde_json::json!({})).await
    }

    /// [`Client::add_bulk`] plus extra body fields — `json!({"timestamp": "2024-03-01T10:00:00Z"})`
    /// to date what is being backfilled.
    ///
    /// Backfilling is what bulk ingest is FOR, and dating it is what makes the result
    /// usable: without a timestamp every memory carries the upload time, so the store
    /// sorts wrong and an `event_from`/`event_to` search misses it.
    ///
    /// `timestamp` must be RFC3339 (`2024-03-01T10:00:00Z`). A plain date or any other
    /// string is ignored without an error, and the memories are filed at upload time.
    ///
    /// A backfill usually wants a key as well — see
    /// [`Client::add_bulk_with_idempotent`], which takes both.
    pub async fn add_bulk_with(
        &self,
        content: &str,
        user_id: impl Into<Option<&str>>,
        category: &str,
        extra: serde_json::Value,
    ) -> Result<serde_json::Value, WosError> {
        self.bulk(content, user_id, category, extra, None).await
    }

    /// [`Client::add_bulk_with`] and [`Client::add_bulk_idempotent`] in one call: date
    /// the backfill AND make re-running it safe.
    ///
    /// A backfill is the call most worth a key, because a run that dies halfway and is
    /// started again would otherwise ingest the whole blob a second time — and it is
    /// also the call that has to carry a timestamp, or every memory is dated the day
    /// the import ran.
    pub async fn add_bulk_with_idempotent(
        &self,
        content: &str,
        user_id: impl Into<Option<&str>>,
        category: &str,
        extra: serde_json::Value,
        idempotency_key: &str,
    ) -> Result<serde_json::Value, WosError> {
        self.bulk(content, user_id, category, extra, Some(idempotency_key)).await
    }

    /// The body builder behind all four `add_bulk*`. Written once because the ORDER
    /// matters: `extra` first, reserved fields after, so a caller forwarding untrusted
    /// JSON cannot point the write at another store. The single-memory route has the
    /// same rule in [`Client::store_one`].
    async fn bulk(
        &self,
        content: &str,
        user_id: impl Into<Option<&str>>,
        category: &str,
        extra: serde_json::Value,
        idempotency_key: Option<&str>,
    ) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let mut obj = serde_json::Map::new();
        if let Some(e) = extra.as_object() {
            for (k, v) in e {
                obj.insert(k.clone(), v.clone());
            }
        }
        // Reserved fields last, so they win over anything `extra` tried to set.
        obj.insert("user_id".into(), serde_json::json!(user_id));
        obj.insert("content".into(), serde_json::json!(content));
        obj.insert("category".into(), serde_json::json!(category));
        self.post_idem("/api/v1/memory/bulk-store", serde_json::Value::Object(obj), idempotency_key)
            .await
    }

    /// Supersede an old memory with new content. Payload first, user_id last — same shape as `add`/`search`.
    pub async fn update(&self, old_memory_id: &str, new_content: &str, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/memory/supersede", serde_json::json!({"user_id": user_id, "old_memory_id": old_memory_id, "new_content": new_content})).await
    }

    // ----- idempotent writes -----
    //
    // An `idempotency_key` makes repeating THAT EXACT write safe: the API replays the
    // first response instead of storing again (10 min), and answers 422 if the same key
    // arrives with a different body. Use it when the retry is YOURS — a job that died and
    // was re-run, a queue that redelivers. This client retries a write only where
    // nothing was stored: a 429, or a 409 saying another write to the store was in
    // flight. It never retries a write on 408 / 502 / 503 / 504 or a dropped body,
    // where the first attempt may already have been applied — without a key the client
    // cannot know whether it was.
    //
    // The key covers a retry sent after the first attempt finished. A retry that
    // overlaps a first attempt still running can run twice.
    //
    // The key must be UNIQUE PER LOGICAL WRITE — derive it from the thing being stored
    // (`format!("import:{}", row.id)`), never a constant, or the second write replays the
    // first and is silently lost. That is also why these are separate methods rather than
    // a `with_idempotency_key()` clone: a clone invites reuse across different writes,
    // which is exactly the mistake that loses data.
    //
    // The window is best-effort and can be shorter than 10 minutes; it is not a durable
    // de-duplication record.
    // Format: 1-128 chars of `[A-Za-z0-9._:-]`, rejected locally before the request.

    /// [`Client::add`] with an idempotency key.
    pub async fn add_idempotent(&self, content: &str, user_id: impl Into<Option<&str>>, metadata: serde_json::Value, idempotency_key: &str) -> Result<serde_json::Value, WosError> {
        self.store_one(content, user_id, metadata, serde_json::json!({}), Some(idempotency_key)).await
    }

    /// [`Client::add_turn`] with an idempotency key.
    pub async fn add_turn_idempotent(&self, user_msg: &str, assistant_msg: &str, user_id: impl Into<Option<&str>>, idempotency_key: &str) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post_idem("/api/v1/memory/store-turn", serde_json::json!({"user_id": user_id, "user_msg": user_msg, "assistant_msg": assistant_msg}), Some(idempotency_key)).await
    }

    /// [`Client::add_bulk`] with an idempotency key. The call most worth one: a backfill
    /// that dies halfway and is re-run would otherwise ingest the whole blob a second time.
    pub async fn add_bulk_idempotent(&self, content: &str, user_id: impl Into<Option<&str>>, category: &str, idempotency_key: &str) -> Result<serde_json::Value, WosError> {
        // An empty category is sent as it is — see `add_bulk`.
        self.bulk(content, user_id, category, serde_json::json!({}), Some(idempotency_key)).await
    }

    /// [`Client::update`] with an idempotency key.
    pub async fn update_idempotent(&self, old_memory_id: &str, new_content: &str, user_id: impl Into<Option<&str>>, idempotency_key: &str) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post_idem("/api/v1/memory/supersede", serde_json::json!({"user_id": user_id, "old_memory_id": old_memory_id, "new_content": new_content}), Some(idempotency_key)).await
    }

    // ----- read -----

    /// Search stored memories. Returns them most relevant first.
    ///
    /// `limit` is 5..=20, and out of range is refused rather than clamped: asking for
    /// 50 and silently receiving 20 reads as "that is all there is".
    ///
    /// `limit` bounds `memories`, not the returned `Vec`. The assistant's own words
    /// (on a model whose [`Client::list_models`] `capabilities` include
    /// `self_memories`) and image memories (with `images`, one unless `max_images` says
    /// otherwise) come back in it as well, so it can hold more than `limit`. Size a
    /// prompt window on what you get back, not on `limit`. [`Client::search_full`]
    /// hands the fields back apart.
    pub async fn search(&self, query: &str, user_id: impl Into<Option<&str>>, limit: usize) -> Result<Vec<Memory>, WosError> {
        let user_id = self.uid(user_id.into())?;
        check_search_limit(limit)?;
        let v = self.post("/api/v1/memory/search", serde_json::json!({"user_id": user_id, "query": query, "max_results": limit})).await?;
        Ok(merge_results(&v))
    }

    /// Like [`Client::search`], but merges extra request fields into the body —
    /// e.g. `json!({"cache_control": {"ttl": "5m"}})` for recall caching,
    /// `json!({"speaker": "Bob"})` to recall one person's words only, or
    /// `json!({"tz": 9})` / `json!({"form": "memoir"})`.
    ///
    /// Recall caching is not free to switch on: a hit inside the TTL bills at 0.1x,
    /// but the FIRST call writes the cache and bills the query tokens at 2x (`5m`)
    /// or 3x (`1h`). It pays for a query you repeat or extend and costs more for one
    /// you issue once, so do not set it on every search.
    ///
    /// It also carries `filters`, which narrows the search to part of a store. The
    /// filter chooses what is searched, not what is kept afterwards — so a narrow
    /// filter still returns your full `limit` when that many matches sit inside it:
    ///
    /// ```no_run
    /// # use wontopos::Client; use serde_json::json;
    /// # async fn f(mem: Client) -> Result<(), wontopos::WosError> {
    /// mem.search_with("what did we decide", "alice", 10, json!({"filters": {
    ///     "categories": ["work"],
    ///     "event_from": "2026-01-01",   // WHEN IT HAPPENED (metadata.event_date),
    ///     "event_to": "2026-06-30",     // not when it was written
    /// }})).await?;
    /// # Ok(()) }
    /// ```
    ///
    /// Filters apply to `memories`. The assistant's own words (`self_memories`, merged
    /// into the returned `Vec`) are not filtered; [`Client::search_full_with`] keeps
    /// them apart.
    ///
    /// Accepted filter keys: `categories`, `event_from`, `event_to`, `time_from`,
    /// `time_to`, `min_importance`. Unlisted keys are dropped by the API rather than
    /// rejected — a typo silently widens the search, so spell them exactly (this client
    /// warns once per unknown key on stderr).
    ///
    /// The four date ends take RFC3339 or a plain date (YYYY-MM-DD). A plain end date
    /// (`time_to`, `event_to`) covers that whole day in UTC. A value that is not a date
    /// is refused (400) naming the field.
    pub async fn search_with(&self, query: &str, user_id: impl Into<Option<&str>>, limit: usize, extra: serde_json::Value) -> Result<Vec<Memory>, WosError> {
        let user_id = self.uid(user_id.into())?;
        // `extra` goes in first; the reserved fields are set AFTER so they win —
        // an app forwarding untrusted input as `extra` can't override the store
        // (user_id), query, or limit.
        let mut obj = serde_json::Map::new();
        if let Some(extra_obj) = extra.as_object() {
            for (k, v) in extra_obj {
                obj.insert(k.clone(), v.clone());
            }
        }
        obj.insert("user_id".into(), serde_json::json!(user_id));
        obj.insert("query".into(), serde_json::json!(query));
        check_search_limit(limit)?;
        obj.insert("max_results".into(), serde_json::json!(limit));
        let body = serde_json::Value::Object(obj);
        check_max_images(&body)?;
        warn_on_unknown_filters(&body);
        let v = self.post("/api/v1/memory/search", body).await?;
        Ok(merge_results(&v))
    }

    /// Search with the options that have a fixed shape, spelled as types.
    ///
    /// [`Client::search_with`] takes arbitrary JSON and stays the escape hatch for
    /// anything the API grows before this crate does. That flexibility is exactly why it
    /// cannot help with `json!({"verfy": 3})` — a typo inside a JSON literal is still a
    /// perfectly good JSON literal, so it travels to the API, is ignored as an unknown
    /// field, and comes back 200 having changed nothing, while the caller believes they
    /// turned on re-asking. These fields exist so the compiler reads them.
    ///
    /// ```no_run
    /// # async fn f(mem: &wontopos::Client) -> Result<(), wontopos::WosError> {
    /// use wontopos::SearchOpts;
    /// let hits = mem
    ///     .search_opts("what did we decide about the deadline?", None, 10,
    ///                  &SearchOpts { verify: Some(2), ..Default::default() })
    ///     .await?;
    /// # let _ = hits;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn search_opts(
        &self,
        query: &str,
        user_id: impl Into<Option<&str>>,
        limit: usize,
        opts: &SearchOpts,
    ) -> Result<Vec<Memory>, WosError> {
        let mut extra = serde_json::Map::new();
        if let Some(v) = opts.verify {
            extra.insert("verify".into(), serde_json::json!(v));
        }
        if let Some(n) = opts.max_images {
            extra.insert("max_images".into(), serde_json::json!(n));
        }
        // Through `search_with` on purpose: the rule that user_id/query/max_results beat
        // anything the caller supplies lives there, and a second copy of it here is a
        // second place for it to stop being true.
        self.search_with(query, user_id, limit, serde_json::Value::Object(extra)).await
    }

    /// Search, and keep every field the answer came with.
    ///
    /// Same request as [`Client::search_opts`] — reach for this one when the options
    /// make the merged `Vec` an incomplete answer:
    ///
    /// ```no_run
    /// # use wontopos::{Client, SearchOpts};
    /// # async fn run() -> Result<(), wontopos::WosError> {
    /// let mem = Client::new("wos-...");
    /// let r = mem.search_full("the day we moved", "alice", 10,
    ///                         &SearchOpts { max_images: Some(3), verify: Some(2), ..Default::default() }).await?;
    /// r.images.len();   // the photos, apart from the text memories
    /// r.verify_used;    // re-ask passes that actually ran
    /// # Ok(()) }
    /// ```
    pub async fn search_full(
        &self,
        query: &str,
        user_id: impl Into<Option<&str>>,
        limit: usize,
        opts: &SearchOpts,
    ) -> Result<SearchFull, WosError> {
        self.search_full_with(query, user_id, limit, opts, serde_json::json!({})).await
    }

    /// [`Client::search_full`] plus extra body fields: `filters`, `speaker`,
    /// `cache_control`, `form`, `tz`, anything [`Client::search_with`] carries. The
    /// answer keeps `images`, `self_memories` and `verify_used` apart.
    ///
    /// `extra` goes in first, `opts` over it, and the reserved fields (user_id, query,
    /// max_results) last, so forwarded JSON cannot point the search at another store.
    /// Unknown filter keys are warned about once, as in [`Client::search_with`], and
    /// `filters` apply to `memories`, not to `self_memories`.
    ///
    /// ```no_run
    /// # use wontopos::{Client, SearchOpts};
    /// # use serde_json::json;
    /// # async fn run(mem: Client) -> Result<(), wontopos::WosError> {
    /// let r = mem.search_full_with("photos from the trip", "alice", 10,
    ///     &SearchOpts { max_images: Some(3), ..Default::default() },
    ///     json!({"filters": {"event_from": "2026-06-01", "event_to": "2026-06-30"}})).await?;
    /// r.images.len();
    /// # Ok(()) }
    /// ```
    pub async fn search_full_with(
        &self,
        query: &str,
        user_id: impl Into<Option<&str>>,
        limit: usize,
        opts: &SearchOpts,
        extra: serde_json::Value,
    ) -> Result<SearchFull, WosError> {
        let user_id = self.uid(user_id.into())?;
        let mut obj = serde_json::Map::new();
        if let Some(e) = extra.as_object() {
            for (k, v) in e {
                obj.insert(k.clone(), v.clone());
            }
        }
        if let Some(v) = opts.verify {
            obj.insert("verify".into(), serde_json::json!(v));
        }
        if let Some(n) = opts.max_images {
            obj.insert("max_images".into(), serde_json::json!(n));
        }
        // Reserved fields last, so they win over anything above.
        obj.insert("user_id".into(), serde_json::json!(user_id));
        obj.insert("query".into(), serde_json::json!(query));
        check_search_limit(limit)?;
        obj.insert("max_results".into(), serde_json::json!(limit));
        let body = serde_json::Value::Object(obj);
        check_max_images(&body)?;
        warn_on_unknown_filters(&body);
        let v = self.post("/api/v1/memory/search", body).await?;
        Ok(SearchFull {
            memories: memories_from(v.get("memories")),
            self_memories: memories_from(v.get("self_memories")),
            images: memories_from(v.get("images")),
            verify_used: v.get("verify_used").and_then(|n| n.as_u64()).map(|n| u8::try_from(n).unwrap_or(u8::MAX)),
            raw: v,
        })
    }

    /// Search, with the assistant's own words kept apart: both fields from ONE call.
    ///
    /// Returns `{ memories, self_memories }` — `memories` is what others said and
    /// general memories, `self_memories` is the assistant's OWN words (stored with
    /// speaker "me"), kept apart so whoever reads them never confuses who said what.
    /// `self_memories` is empty unless the model's [`Client::list_models`]
    /// `capabilities` include `self_memories`. Image memories are not included;
    /// [`Client::search`] and [`Client::search_full`] carry them.
    pub async fn search_self(&self, query: &str, user_id: impl Into<Option<&str>>, limit: usize) -> Result<SelfSearch, WosError> {
        let user_id = self.uid(user_id.into())?;
        check_search_limit(limit)?;
        let v = self
            .post("/api/v1/memory/search", serde_json::json!({"user_id": user_id, "query": query, "max_results": limit}))
            .await?;
        // Parse each field the same way `search` does: corrupt elements are skipped and
        // the good memories survive. Missing OR explicit null → empty (a non-self
        // model sends self_memories: null; a broken proxy may null either).
        Ok(SelfSearch {
            memories: memories_from(v.get("memories")),
            self_memories: memories_from(v.get("self_memories")),
        })
    }

    /// One-call LLM context: short-term + long-term + surrounding context.
    pub async fn recall(&self, query: &str, user_id: impl Into<Option<&str>>) -> Result<RecallResponse, WosError> {
        let user_id = self.uid(user_id.into())?;
        let v = self.post("/api/v1/memory/recall", serde_json::json!({"user_id": user_id, "query": query})).await?;
        serde_json::from_value(v).map_err(|e| WosError::Api {
            status: 200,
            message: format!("invalid recall response: {e}"),
        })
    }

    /// [`Client::recall`] with the two options that have a fixed shape.
    ///
    /// [`Client::recall_with`] takes arbitrary JSON and stays the escape hatch; that is also
    /// why it cannot catch `json!({"context_limt": 5})`. A typo inside a JSON literal is a
    /// perfectly good JSON literal, so it travels, is ignored as an unknown field, and comes
    /// back 200 with the default. These fields exist so the compiler reads them instead.
    ///
    /// ```no_run
    /// # async fn f(mem: &wontopos::Client) -> Result<(), wontopos::WosError> {
    /// use wontopos::RecallOpts;
    /// let ctx = mem
    ///     .recall_opts("what should I know before I reply?", None,
    ///                  &RecallOpts { limit: Some(20), ..Default::default() })
    ///     .await?;
    /// # let _ = ctx;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn recall_opts(
        &self,
        query: &str,
        user_id: impl Into<Option<&str>>,
        opts: &RecallOpts,
    ) -> Result<RecallResponse, WosError> {
        // Checked here, where the body is built, so an out-of-range count is refused
        // before a socket is opened.
        let mut extra = serde_json::Map::new();
        if let Some(n) = opts.limit {
            check_search_limit(n)?;
            extra.insert("limit".into(), serde_json::json!(n));
        }
        if let Some(n) = opts.context_limit {
            check_context_limit(n)?;
            extra.insert("context_limit".into(), serde_json::json!(n));
        }
        // Through `recall_with` on purpose: the rule that user_id and query beat anything
        // the caller supplies lives there, and a second copy is a second place to break it.
        self.recall_with(query, user_id, serde_json::Value::Object(extra)).await
    }

    /// Run a built-in engram ("deep_recall", "timeline", "gather", "equilibrium",
    /// "tone_stabilizer"; the service is the authority — an unknown name comes back
    /// with the list it accepts). Returns the merged result.
    pub async fn engram(&self, name: &str, query: &str, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/engram/run", serde_json::json!({"name": name, "user_id": user_id, "query": query})).await
    }

    /// Like [`Client::recall`], but merges extra body fields — e.g.
    /// `json!({"form": "memoir", "tz": 9})` to render each long-term memory's time in a
    /// delivery form (on a model whose [`Client::list_models`] `capabilities` include
    /// `forms`). Reserved fields (user_id, query) are set last so they win.
    pub async fn recall_with(&self, query: &str, user_id: impl Into<Option<&str>>, extra: serde_json::Value) -> Result<RecallResponse, WosError> {
        let user_id = self.uid(user_id.into())?;
        let mut obj = serde_json::Map::new();
        if let Some(e) = extra.as_object() {
            for (k, v) in e {
                obj.insert(k.clone(), v.clone());
            }
        }
        obj.insert("user_id".into(), serde_json::json!(user_id));
        obj.insert("query".into(), serde_json::json!(query));
        let v = self.post("/api/v1/memory/recall", serde_json::Value::Object(obj)).await?;
        serde_json::from_value(v).map_err(|e| WosError::Api {
            status: 200,
            message: format!("invalid recall response: {e}"),
        })
    }

    /// Like [`Client::engram`], but merges extra body fields — e.g. `json!({"form": "archive", "tz": 9})`.
    pub async fn engram_with(&self, name: &str, query: &str, user_id: impl Into<Option<&str>>, extra: serde_json::Value) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let mut obj = serde_json::Map::new();
        if let Some(e) = extra.as_object() {
            for (k, v) in e {
                obj.insert(k.clone(), v.clone());
            }
        }
        obj.insert("name".into(), serde_json::json!(name));
        obj.insert("user_id".into(), serde_json::json!(user_id));
        obj.insert("query".into(), serde_json::json!(query));
        self.post("/api/v1/engram/run", serde_json::Value::Object(obj)).await
    }

    /// Like [`Client::search_self`], but merges extra request fields — e.g.
    /// `json!({"form": "memoir", "tz": 9})` or `json!({"speaker": "Bob"})`. `filters`
    /// apply to `memories`; `self_memories` are not filtered.
    pub async fn search_self_with(&self, query: &str, user_id: impl Into<Option<&str>>, limit: usize, extra: serde_json::Value) -> Result<SelfSearch, WosError> {
        let user_id = self.uid(user_id.into())?;
        let mut obj = serde_json::Map::new();
        if let Some(e) = extra.as_object() {
            for (k, v) in e {
                obj.insert(k.clone(), v.clone());
            }
        }
        obj.insert("user_id".into(), serde_json::json!(user_id));
        obj.insert("query".into(), serde_json::json!(query));
        check_search_limit(limit)?;
        obj.insert("max_results".into(), serde_json::json!(limit));
        let body = serde_json::Value::Object(obj);
        check_max_images(&body)?;
        warn_on_unknown_filters(&body);
        let v = self.post("/api/v1/memory/search", body).await?;
        // Element-wise per field: corrupt elements skipped, good memories survive.
        Ok(SelfSearch {
            memories: memories_from(v.get("memories")),
            self_memories: memories_from(v.get("self_memories")),
        })
    }

    /// Recent conversation turns (short-term memory). Elements that are not objects
    /// are skipped.
    pub async fn history(&self, user_id: impl Into<Option<&str>>) -> Result<Vec<serde_json::Value>, WosError> {
        let user_id = self.uid(user_id.into())?;
        let v = self.post("/api/v1/memory/history", serde_json::json!({"user_id": user_id})).await?;
        Ok(records(&v, "turns"))
    }

    /// Memory counts for a store.
    pub async fn stats(&self, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/memory/stats", serde_json::json!({"user_id": user_id})).await
    }

    /// Fetch ONE memory by id — the text you stored, and its metadata.
    /// The id is what `add`/`store` or `list_memories` returned. Same visibility
    /// as `list_memories`: an id from another store, an id `list_memories` does not
    /// return, or an invalidated memory is a 404 (`ErrorKind::NotFound`).
    pub async fn get(&self, user_id: impl Into<Option<&str>>, memory_id: &str) -> Result<serde_json::Value, WosError> {
        if memory_id.trim().is_empty() {
            return Err(WosError::Api {
                status: 400,
                message: "memory_id is required — the id that add/store or list_memories returned.".into(),
            });
        }
        let user_id = self.uid(user_id.into())?;
        let v = self
            .post(
                "/api/v1/memory/get",
                // Validated by trimming, so the trimmed form is sent.
                serde_json::json!({"user_id": user_id, "memory_id": memory_id.trim()}),
            )
            .await?;
        Ok(memory_from_get(v))
    }

    /// List a store's stored memories — the text you stored, plus its metadata.
    /// Paginated: pass the returned `next_cursor` back as `cursor` for the next page,
    /// and only a cursor the service returned. A null `next_cursor` means the last
    /// page, but the last page can also carry one; the call after it then returns an
    /// empty page. Use it to browse or export a store. `limit` and `cursor` accept
    /// `None` (defaults: 100, first page). `limit` is 1 to 500; out of range is refused
    /// before sending. Returns `{ "memories": [...], "count", "next_cursor" }`.
    pub async fn list_memories(
        &self,
        user_id: impl Into<Option<&str>>,
        limit: impl Into<Option<usize>>,
        cursor: impl Into<Option<&str>>,
    ) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let limit = limit.into().unwrap_or(100);
        check_list_limit(limit)?;
        let mut body = serde_json::json!({ "user_id": user_id, "limit": limit });
        if let Some(c) = cursor.into() {
            body["cursor"] = serde_json::json!(c);
        }
        self.post("/api/v1/memory/list", body).await
    }

    // ----- images -----
    //
    // Images need a model whose `list_models()` capabilities include `images`. Storing
    // one is [`Client::add_with`]: an image is an option on the ordinary store call.

    /// Fetch the bytes of an image memory → `(bytes, content_type)`.
    ///
    /// This is the picture the SERVICE holds, not necessarily your upload — in size or
    /// in format. An image whose long edge was over 1568px was downscaled to 1568 on the
    /// way in and re-encoded (lossless formats as WebP, so a PNG comes back as
    /// `image/webp`; JPEG stays JPEG), and that smaller picture is what is stored and
    /// comes back here. Keep your own copy if you need the full-resolution file.
    ///
    /// The type is sniffed from the BYTES, not from whatever the upload was named, so
    /// take the file extension from the returned type rather than from what you sent.
    ///
    /// Answers `NotFound` when the memory has no image, when it was stored with
    /// `image.reference` (the service then keeps no bytes; fetch it from your own
    /// reference), or when this service keeps no image bytes at all. It says "no"
    /// rather than handing back something empty, so "a memory with no image" never
    /// looks like "an image we lost".
    pub async fn get_image(
        &self,
        user_id: impl Into<Option<&str>>,
        memory_id: &str,
    ) -> Result<(Vec<u8>, String), WosError> {
        // Same guard `get` and `delete` carry. A blank id here is not a wipe, but it
        // is a request the caller cannot have meant, and answering it from the server
        // costs a round trip to learn that.
        let memory_id = memory_id.trim();
        if memory_id.is_empty() {
            return Err(WosError::Api {
                status: 400,
                message: "memory_id is required — the id that add/store or list_images returned.".into(),
            });
        }
        let user_id = self.uid(user_id.into())?;
        let body = serde_json::json!({ "user_id": user_id, "memory_id": memory_id });
        self.request_bytes("/api/v1/memory/image", &body).await
    }

    /// Remove the IMAGE from a memory, keeping its text.
    ///
    /// Pass `preview = true` to see the outcome first: it reports `memory_kept` and
    /// changes nothing. Without a preview, a 404 after an attempt that may have been
    /// applied ends with "(an earlier attempt may already have deleted it)".
    pub async fn forget_image(
        &self,
        user_id: impl Into<Option<&str>>,
        memory_id: &str,
        preview: bool,
    ) -> Result<serde_json::Value, WosError> {
        let memory_id = memory_id.trim();
        if memory_id.is_empty() {
            return Err(WosError::Api {
                status: 400,
                message: "memory_id is required — the id of the memory whose image you want removed.".into(),
            });
        }
        let user_id = self.uid(user_id.into())?;
        let mut body = serde_json::json!({ "user_id": user_id, "memory_id": memory_id });
        if preview {
            body["preview"] = serde_json::json!(true);
        }
        // A preview deletes nothing, so a 404 there never says it may already be gone.
        self.send(reqwest::Method::DELETE, "/api/v1/memory/image", Some(&body), None, None, !preview).await
    }

    /// One page of image memories, newest first, plus the store's TOTAL `count`.
    ///
    /// `count` is the total, not the size of the page. `limit` is 5 to 20; out of range
    /// is refused before sending. Paging is by cursor: hand `next_before` and
    /// `next_skip_ids` back as `before` / `skip_ids`. Both are needed because several
    /// images can share a timestamp, and a timestamp alone would either repeat them or
    /// skip them.
    pub async fn list_images(
        &self,
        user_id: impl Into<Option<&str>>,
        limit: impl Into<Option<usize>>,
        before: impl Into<Option<&str>>,
        skip_ids: Option<&[String]>,
    ) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let mut body = serde_json::json!({ "user_id": user_id });
        if let Some(l) = limit.into() {
            check_page_limit("limit", l)?;
            body["limit"] = serde_json::json!(l);
        }
        if let Some(b) = before.into() {
            body["before"] = serde_json::json!(b);
        }
        if let Some(s) = skip_ids {
            body["skip_ids"] = serde_json::json!(s);
        }
        self.post("/api/v1/memory/images", body).await
    }

    /// EVERY image memory in a store, paging under the hood. [`Client::list_images`] is
    /// the one-page primitive; this is the "give me all of them" convenience.
    ///
    /// Rust has no async generator in the stable language, so this collects and
    /// returns. Reach for `list_images` when a store is big enough that holding every
    /// image row at once matters. `page_size` is 5 to 20; out of range is refused before
    /// sending.
    ///
    /// This walks ROWS, not pixels — the bytes come from [`Client::get_image`] one at a
    /// time. A thousand images here is a thousand small JSON records, not a thousand JPEGs.
    /// Elements that are not objects are skipped. A cursor that comes back after a page
    /// with rows in it, or the page ceiling, is an error rather than a short list.
    pub async fn list_all_images(
        &self,
        user_id: impl Into<Option<&str>>,
        page_size: impl Into<Option<usize>>,
    ) -> Result<Vec<serde_json::Value>, WosError> {
        let user_id = self.uid(user_id.into())?;
        let page_size = page_size.into();
        if let Some(n) = page_size {
            check_page_limit("page_size", n)?;
        }
        let mut out: Vec<serde_json::Value> = Vec::new();
        let mut before: Option<String> = None;
        let mut skip_ids: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Rust has no `for … else`, so the legitimate exits say so.
        let mut ended = false;
        for _ in 0..MAX_PAGES {
            let mut body = serde_json::json!({ "user_id": user_id });
            if let Some(l) = page_size {
                body["limit"] = serde_json::json!(l);
            }
            if let Some(b) = &before {
                body["before"] = serde_json::json!(b);
            }
            if !skip_ids.is_empty() {
                body["skip_ids"] = serde_json::json!(skip_ids);
            }
            let page = self.post("/api/v1/memory/images", body).await?;
            let rows = records(&page, "images");
            let had_rows = !rows.is_empty();
            out.extend(rows);
            if !page.get("has_more").and_then(|v| v.as_bool()).unwrap_or(false) {
                ended = true;
                break;
            }
            let Some(next_before) = page.get("next_before").and_then(|v| v.as_str()).filter(|s| !s.is_empty())
            else {
                ended = true;
                break;
            };
            let next_skip: Vec<String> = page
                .get("next_skip_ids")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default();
            // The image cursor is the PAIR `before` and `skip_ids`: several images can
            // share a timestamp, so `before` alone does not move while a group that
            // shares one is walked. Progress is judged on the pair.
            let key = format!("{next_before}|{}", next_skip.join(","));
            if !seen.insert(key) {
                if had_rows {
                    return Err(repeated_cursor());
                }
                ended = true;
                break;
            }
            before = Some(next_before.to_string());
            skip_ids = next_skip;
        }
        if !ended {
            return Err(page_ceiling());
        }
        Ok(out)
    }

    /// Every image memory in a store, as a list. The image-side pair of
    /// [`Client::export_memories`].
    ///
    /// It collects rather than streams, and so does every other name for it here:
    /// Rust has no async generator in the stable language, so there is no
    /// page-at-a-time form in this client. A store too large to hold at once has to
    /// be walked with [`Client::list_images`], handing back BOTH cursors the previous
    /// page returned — `before` and `skip_ids`. `before` alone repeats or skips images
    /// that share a timestamp.
    pub async fn export_images(
        &self,
        user_id: impl Into<Option<&str>>,
        page_size: impl Into<Option<usize>>,
    ) -> Result<Vec<serde_json::Value>, WosError> {
        self.list_all_images(user_id, page_size).await
    }

    /// Kept for callers written against it. Despite the name it returns a fully
    /// buffered `Vec`. Use [`Client::export_images`].
    pub async fn iter_images(
        &self,
        user_id: impl Into<Option<&str>>,
        page_size: impl Into<Option<usize>>,
    ) -> Result<Vec<serde_json::Value>, WosError> {
        self.export_images(user_id, page_size).await
    }

    // ----- how much has this memory been edited -----

    /// How much of this store has been altered since it was written.
    ///
    /// Aimed at the MODEL rather than at the developer: an assistant leaning on its own
    /// memory should be able to ask how far that memory has been edited underneath it.
    /// Returns `revised` / `unrevised` / `total`, plus a plain-language `counts` and
    /// `excludes`. `total` counts the memories you stored.
    ///
    /// COUNTS ONLY, and the answer is the same size for a store of a hundred memories
    /// and a store of a hundred million. That is the point: a model asks this
    /// mid-conversation, and an answer that grew with the store would be unusable for
    /// exactly the customers who most need to ask. To see WHICH memories, call
    /// [`Client::revisions_page`] — a separate method, so a page can never arrive
    /// because of a default nobody chose.
    ///
    /// Counts memories a transform touched (supersede, update, retract, image removed).
    /// Deletions are NOT counted — a deleted memory leaves nothing to count.
    /// Served from `/api/v1/won/*`, the surface for calls a model makes ABOUT its
    /// memory rather than calls an application makes WITH it.
    pub async fn revisions(
        &self,
        user_id: impl Into<Option<&str>>,
    ) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/won/revisions", serde_json::json!({ "user_id": user_id })).await
    }

    /// What this key has spent, and what is left — the numbers behind "keep going?".
    ///
    /// Free: no charge and no balance gate, because an account at zero still has to be
    /// able to find out why. Rate-limited instead.
    ///
    /// Scoped to THIS key: its own lifetime spend, plus its workspace and stores over
    /// the window. Never another key's. `balance_cents` is account-wide, since that is
    /// what gates the next call whichever key makes it.
    ///
    /// `stores` is busiest first and at most 50 rows — a longer list is cut, so the rows
    /// need not sum to `workspace`. A row named `other` is an overflow bucket, not a
    /// store: passing it as a store id finds nothing.
    ///
    /// Served from `/api/v1/won/*`, like [`Client::revisions`]: calls a model makes
    /// ABOUT its memory rather than calls an application makes WITH it.
    pub async fn usage(&self, days: u32) -> Result<serde_json::Value, WosError> {
        if !(1..=365).contains(&days) {
            return Err(WosError::Api {
                status: 400,
                message: format!("days must be between 1 and 365, got {days}."),
            });
        }
        self.request(
            reqwest::Method::GET,
            &format!("/api/v1/won/usage?days={days}"),
            None,
            None,
        )
        .await
    }

    /// ONE PAGE of the memories behind a [`Client::revisions`] number — at most 20.
    ///
    /// `include` picks which side: `"revised"` or `"unrevised"`. One side per call; there
    /// is no way to ask for both lists in a single response, which is what keeps this
    /// usable on a store with a hundred million memories. An unrecognised value is
    /// rejected by the service rather than silently ignored.
    ///
    /// Paging is by cursor, like [`Client::list_images`]: hand `next_before` and
    /// `next_skip_ids` back as `before` / `skip_ids`. Both are needed because several
    /// memories can share a timestamp, and a timestamp alone would repeat or skip them.
    /// `limit` is 5 to 20; out of range is refused before sending.
    ///
    /// A separate method, so the counts call stays the cheap question it is.
    ///
    /// Pages are ordered by when each memory was STORED, not by when it was edited.
    /// The response says which in `ordered_by`.
    ///
    /// ```no_run
    /// # async fn f(mem: &wontopos::Client) -> Result<(), wontopos::WosError> {
    /// let n = mem.revisions(None).await?;
    /// println!("{} of {} edited", n["revised"], n["total"]);
    ///
    /// let mut before: Option<String> = None;
    /// let mut skip: Vec<String> = Vec::new();
    /// loop {
    ///     let p = mem
    ///         .revisions_page(None, "revised", None, before.as_deref(), Some(&skip))
    ///         .await?;
    ///     for m in p["memories"].as_array().unwrap_or(&vec![]) {
    ///         println!("{}", m["content"]);
    ///     }
    ///     // Both, not just `has_more`: a page that says there is more but carries no
    ///     // cursor would set `before` back to None, which reads as "no cursor" —
    ///     // page one, forever, without a single error.
    ///     let next = p["next_before"].as_str().map(str::to_string);
    ///     if !p["has_more"].as_bool().unwrap_or(false) || next.is_none() {
    ///         break;
    ///     }
    ///     before = next;
    ///     skip = p["next_skip_ids"]
    ///         .as_array()
    ///         .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
    ///         .unwrap_or_default();
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn revisions_page(
        &self,
        user_id: impl Into<Option<&str>>,
        include: &str,
        limit: impl Into<Option<usize>>,
        before: impl Into<Option<&str>>,
        skip_ids: Option<&[String]>,
    ) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let mut body = serde_json::json!({ "user_id": user_id, "include": include });
        if let Some(l) = limit.into() {
            check_page_limit("limit", l)?;
            body["limit"] = serde_json::json!(l);
        }
        if let Some(b) = before.into() {
            body["before"] = serde_json::json!(b);
        }
        if let Some(s) = skip_ids {
            body["skip_ids"] = serde_json::json!(s);
        }
        self.post("/api/v1/won/revisions", body).await
    }

    /// The full chain of edits behind one memory, oldest first. `is_current` marks the
    /// version in force. `revisions` says how much a store moved; this says what happened
    /// to one fact.
    pub async fn lineage(
        &self,
        user_id: impl Into<Option<&str>>,
        memory_id: &str,
    ) -> Result<serde_json::Value, WosError> {
        if memory_id.trim().is_empty() {
            return Err(WosError::Api {
                status: 400,
                message: "memory_id is required — the id that add/store or list_memories returned.".into(),
            });
        }
        let user_id = self.uid(user_id.into())?;
        self.post(
            "/api/v1/memory/lineage",
            serde_json::json!({ "user_id": user_id, "memory_id": memory_id.trim() }),
        )
        .await
    }

    /// What one person said, newest first.
    ///
    /// `speaker` is the tag written at store time — `"me"` for the assistant's own words,
    /// otherwise a person's name. Same cursor paging as `list_images`; `limit` is 5 to
    /// 20, and out of range is refused before sending.
    ///
    /// `points_to_delete` is the count to show before anyone confirms a delete of this
    /// speaker's memories.
    pub async fn by_speaker(
        &self,
        speaker: &str,
        user_id: impl Into<Option<&str>>,
        limit: impl Into<Option<usize>>,
        before: impl Into<Option<&str>>,
        skip_ids: Option<&[String]>,
    ) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        if speaker.trim().is_empty() {
            return Err(WosError::Api {
                status: 400,
                message: "speaker is required — \"me\" for the assistant, or a person's name".into(),
            });
        }
        let mut body = serde_json::json!({ "user_id": user_id, "speaker": speaker.trim() });
        if let Some(l) = limit.into() {
            check_page_limit("limit", l)?;
            body["limit"] = serde_json::json!(l);
        }
        if let Some(b) = before.into() {
            body["before"] = serde_json::json!(b);
        }
        if let Some(s) = skip_ids {
            body["skip_ids"] = serde_json::json!(s);
        }
        self.post("/api/v1/memory/by-speaker", body).await
    }

    /// Fetch EVERY memory in a store, paging under the hood — the text you stored, and
    /// its metadata. Returns them all. `list_memories` is the one-page primitive; this is
    /// the "give me the whole store" convenience (browse / export).
    ///
    /// Elements that are not objects are skipped. A cursor that comes back after a page
    /// with rows in it, or the page ceiling, is an error rather than a short list.
    pub async fn list_all_memories(
        &self,
        user_id: impl Into<Option<&str>>,
    ) -> Result<Vec<serde_json::Value>, WosError> {
        let user_id = self.uid(user_id.into())?;
        let mut out: Vec<serde_json::Value> = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        // The ceiling bounds a server that mints a FRESH cursor every page. Rust has no
        // `for … else`, so the legitimate exits say so.
        let mut ended = false;
        for _ in 0..MAX_PAGES {
            let mut body = serde_json::json!({ "user_id": user_id, "limit": 100 });
            if let Some(c) = &cursor {
                body["cursor"] = serde_json::json!(c);
            }
            let page = self.post("/api/v1/memory/list", body).await?;
            let rows = records(&page, "memories");
            let had_rows = !rows.is_empty();
            out.extend(rows);
            let next = page
                .get("next_cursor")
                .and_then(|c| c.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            match next {
                None => {
                    ended = true;
                    break;
                }
                Some(c) if seen.insert(c.clone()) => cursor = Some(c),
                // A cursor seen before, after an empty page: nothing is left.
                Some(_) if !had_rows => {
                    ended = true;
                    break;
                }
                Some(_) => {
                    return Err(repeated_cursor())
                }
            }
        }
        if !ended {
            return Err(page_ceiling());
        }
        Ok(out)
    }

    /// Every memory in a store; the same as [`Client::list_all_memories`].
    pub async fn export_memories(
        &self,
        user_id: impl Into<Option<&str>>,
    ) -> Result<Vec<serde_json::Value>, WosError> {
        self.list_all_memories(user_id).await
    }

    /// Check connectivity AND that the API key works. `Ok(true)` on success, else an
    /// error (`err.kind()`: `Auth` = bad key, `PaymentRequired` = valid key but no
    /// card / depleted balance, `Connection` = unreachable). Makes one metered request.
    pub async fn ping(&self) -> Result<bool, WosError> {
        self.request(reqwest::Method::GET, "/api/v1/memory/collections", None, None).await?;
        Ok(true)
    }

    // ----- stores -----

    /// Create a store — the `user_id` you read and write under. Stores are
    /// explicit: a store must exist before you `add` to or `search` it, otherwise
    /// those calls return 404. Idempotent. Every account starts with a `default`
    /// store. Returns `{ "user_id", "status" }` (`status` is `"created"`/`"exists"`), plus
    /// `canonical_id` and `note` when the id is filed under a normalized form. An id that
    /// collides with an existing store once punctuation is folded is refused (409,
    /// `ErrorKind::Conflict`) and never retried.
    pub async fn create_store(&self, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/memory/collection", serde_json::json!({ "user_id": user_id })).await
    }

    /// List your stores: `[{ "user_id", "created_at", "canonical_id"? }, ...]`
    /// (`default` first). Each `user_id` is the id the store was created with;
    /// `canonical_id` appears when the normalized form differs. Elements that are not
    /// objects are skipped.
    pub async fn list_stores(&self) -> Result<Vec<serde_json::Value>, WosError> {
        let v = self.request(reqwest::Method::GET, "/api/v1/memory/collections", None, None).await?;
        Ok(records(&v, "collections"))
    }

    /// Delete a store and ALL its memories. Returns `{ "user_id", "status" }`.
    /// A 404 after an attempt that may have been applied ends with "(an earlier
    /// attempt may already have deleted it)".
    pub async fn delete_store(&self, user_id: &str) -> Result<serde_json::Value, WosError> {
        warn_if_store_id_collapses(user_id); // destructive: warn here too
        if user_id.trim().is_empty() {
            return Err(WosError::Api {
                status: 400,
                message: "user_id is required (non-blank) — delete_store never falls back to the default store.".into(),
            });
        }
        let body = serde_json::json!({ "user_id": user_id });
        self.request(reqwest::Method::DELETE, "/api/v1/memory/collection", Some(&body), None).await
    }

    // ----- speakers (who said it) -----

    /// Register a person for this store. Speakers are explicit: register once,
    /// then store with `json!({"speaker": name})`. `"me"` (the assistant itself)
    /// never needs registration. A store registers up to 50 people to start.
    pub async fn add_speaker(&self, speaker: &str, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/memory/speakers", serde_json::json!({"user_id": user_id, "speaker": speaker})).await
    }

    /// The store's registered people, each with its memory count.
    pub async fn list_speakers(&self, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let query = [("user_id", user_id)];
        self.request(reqwest::Method::GET, "/api/v1/memory/speakers", None, Some(&query)).await
    }

    /// Unregister a person. Their memories stay; the name tag goes. A 404 after an
    /// attempt that may have been applied ends with "(an earlier attempt may already
    /// have deleted it)".
    pub async fn remove_speaker(&self, speaker: &str, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let body = serde_json::json!({"user_id": user_id, "speaker": speaker});
        self.request(reqwest::Method::DELETE, "/api/v1/memory/speakers", Some(&body), None).await
    }

    // ----- delete -----

    /// Delete a single memory by id. `user_id` may be `None` (uses the default store).
    pub async fn delete(&self, user_id: impl Into<Option<&str>>, memory_id: &str) -> Result<serde_json::Value, WosError> {
        // Guard: without a memory_id the API's forget endpoint means "delete the
        // whole store". An empty id slipping in here must never become a wipe.
        // Trimmed first: "   " would otherwise pass, and a server that trims it reads
        // the whole-store form.
        let memory_id = memory_id.trim();
        if memory_id.is_empty() {
            return Err(WosError::Api {
                status: 400,
                message: "memory_id is required (non-blank). To delete every memory in a store, call delete_all(user_id) explicitly.".into(),
            });
        }
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/memory/forget", serde_json::json!({"user_id": user_id, "memory_id": memory_id})).await
    }

    /// Delete ALL memories for a store (GDPR erase). `user_id` is required on purpose -
    /// this is destructive, so it never falls back to the default store.
    pub async fn delete_all(&self, user_id: &str) -> Result<serde_json::Value, WosError> {
        // Destructive calls take the id directly rather than through uid(), so they
        // warn here.
        warn_if_store_id_collapses(user_id);
        if user_id.trim().is_empty() {
            return Err(WosError::Api {
                status: 400,
                message: "user_id is required (non-blank) for delete_all — a blank/whitespace id would wipe the default store.".into(),
            });
        }
        self.post("/api/v1/memory/forget", serde_json::json!({"user_id": user_id})).await
    }

    // ----- internal -----

    /// The checks every request makes before it opens a socket.
    fn preflight(&self) -> Result<(), WosError> {
        if !valid_model(&self.model) {
            return Err(WosError::Api {
                status: 400,
                message: format!(
                    "invalid model name: {:?} (letters, digits, '.', '_', '-' only)",
                    self.model
                ),
            });
        }
        self.check_api_key()?;
        // Inside the URL, the parser drops tabs and newlines and reads `\` as `/`, so the
        // host a request reaches is not the one the string shows.
        if self.base_url.chars().any(|c| c == '\\' || c.is_whitespace() || c.is_control()) {
            return Err(invalid_base_url("it contains whitespace, a backslash or a control character"));
        }
        // `http://` with no host would otherwise become `http:/api/...`, which the
        // URL parser reads as host `api`.
        if let Err(e) = reqwest::Url::parse(&self.base_url) {
            return Err(invalid_base_url(&e.to_string()));
        }
        Ok(())
    }

    /// The key is trimmed at construction, so anything left that is not a plain
    /// ASCII token is a paste error, refused here rather than sent and answered 401.
    /// Empty is checked here because `new` and `with_base_url` are infallible.
    fn check_api_key(&self) -> Result<(), WosError> {
        // `is_whitespace` is false for "", so emptiness has to be its own check.
        if self.api_key.is_empty() {
            return Err(WosError::Api { status: 400, message: "api_key is required".into() });
        }
        // Control characters slip past `is_whitespace` (NUL, 0x01, DEL) and would be
        // carried into the header or rejected deep in the HTTP stack instead of here.
        if self.api_key.chars().any(|c| c.is_control()) {
            return Err(WosError::Api {
                status: 400,
                message: "api_key contains a control character - check for a paste error".into(),
            });
        }
        // Keys are ASCII by construction. A key pasted from a rich-text doc, Slack or a
        // PDF has had its hyphen turned into an en dash, which then fails at the network
        // as the same mystery 401.
        if let Some(bad) = self.api_key.chars().find(|c| !c.is_ascii()) {
            return Err(WosError::Api {
                status: 400,
                message: format!(
                    "api_key contains a non-ASCII character ({bad:?}) - rich text turns '-' into an \
                     en dash; copy the key from a plain-text field"
                ),
            });
        }
        if self.api_key.chars().any(|c| c.is_whitespace()) {
            return Err(WosError::Api {
                status: 400,
                message: "api_key contains whitespace - check for a stray newline or paste error".into(),
            });
        }
        Ok(())
    }

    /// The error for a budget that ran out with no response to report.
    fn deadline_error(&self) -> WosError {
        WosError::Api {
            status: 0,
            message: format!("deadline of {:?} exhausted", self.deadline.unwrap_or_default()),
        }
    }

    /// Keep the quota from a response's headers; `rate_limit()` reports the latest.
    fn record_rate_limit(&self, headers: &reqwest::header::HeaderMap) {
        if let Some(rl) = parse_rate_limit(headers) {
            if let Ok(mut g) = self.rl.lock() {
                *g = Some(rl);
            }
        }
    }

    /// One request that answers with BYTES rather than JSON.
    ///
    /// Only `/memory/image` does this, and it is why it cannot go through `request`:
    /// that path parses the body as JSON and errors on anything else, so a JPEG would
    /// surface as a parse failure on a call that actually succeeded.
    ///
    /// Errors still arrive as JSON, so a non-2xx is decoded the same way as everywhere
    /// else and keeps `NotFound` / auth failures behaving identically.
    ///
    /// Retries 429, a 409 for a write in flight, and connect-level failures, like
    /// every other call. None of those carries an image, and a connect failure never
    /// reached the server.
    async fn request_bytes(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<(Vec<u8>, String), WosError> {
        self.preflight()?;
        let url = format!("{}{}", self.base_url, path);
        let attempts = self.retries.saturating_add(1);
        let deadline_at = self.deadline_at();
        let per_attempt = std::time::Duration::from_secs(self.timeout_secs);
        // The answer that led to this attempt, reported if the budget runs out first.
        let mut last_refusal: Option<Refusal> = None;
        let mut attempt: u32 = 0;
        let (resp, budget, previous) = loop {
            let start = std::time::Instant::now();
            let previous = last_refusal.take();
            let budget = match self.attempt_budget(deadline_at) {
                Ok(b) => b,
                Err(e) => return Err(previous.map_or(e, |r| r.into_error(false))),
            };
            let sent = self
                .http
                .post(&url)
                .header("X-API-Key", &self.api_key)
                .header("X-WOS-Model", &self.model)
                .json(body)
                .timeout(budget)
                .send()
                .await;
            let r = match sent {
                Ok(r) => r,
                Err(e) if e.is_builder() => return Err(builder_error(e)),
                Err(e) => {
                    // The deadline ran out mid-attempt: report the answer before it, or
                    // that the deadline ran out.
                    if ran_out_of_budget(&e, budget, per_attempt) {
                        return Err(previous.map_or_else(|| self.deadline_error(), |r| r.into_error(false)));
                    }
                    // A connect-level failure never reached the server, so it can be sent
                    // again. That includes a CONNECT timeout, which reports both
                    // predicates; a timeout after the request went out is not retried.
                    if e.is_connect() && attempt + 1 < attempts {
                        let delay = backoff(attempt);
                        if !fits_in(delay, deadline_at) {
                            return Err(previous.map_or_else(|| self.deadline_error(), |r| r.into_error(false)));
                        }
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(network_error(e));
                }
            };
            let status = r.status();
            if status.is_redirection() {
                return Err(WosError::Api {
                    status: status.as_u16(),
                    message: "unexpected redirect — refused (the API key never follows a redirect). Check base_url: exact host, https://.".into(),
                });
            }
            self.record_rate_limit(r.headers());
            if !status.is_success() {
                let left = budget.saturating_sub(start.elapsed());
                let refusal = Refusal::read(r, false, attempt, attempt + 1 < attempts, deadline_at, left).await;
                if let Some(delay) = refusal.wait {
                    if attempt + 1 < attempts && fits_in(delay, deadline_at) {
                        last_refusal = Some(refusal);
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                }
                return Err(refusal.into_error(false));
            }
            break (r, budget, previous);
        };
        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        let status = resp.status().as_u16();
        let bytes = match read_capped_bytes(resp).await {
            Ok(b) => b,
            Err(WosError::Network(ne)) if ran_out_of_budget(&ne, budget, per_attempt) => {
                return Err(previous.map_or_else(|| self.deadline_error(), |r| r.into_error(false)));
            }
            Err(e) => return Err(e),
        };
        if bytes.is_empty() {
            // An empty 200 would otherwise read as "here is your image" and write a
            // zero-byte file — indistinguishable from an image we lost.
            return Err(WosError::Api {
                status,
                message: "empty image body — the service returned no bytes".into(),
            });
        }
        Ok((bytes, ctype))
    }

    async fn post(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value, WosError> {
        self.request(reqwest::Method::POST, path, Some(&body), None).await
    }

    async fn post_idem(&self, path: &str, body: serde_json::Value, key: Option<&str>) -> Result<serde_json::Value, WosError> {
        self.request_with_key(reqwest::Method::POST, path, Some(&body), None, key).await
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
        query: Option<&[(&str, String)]>,
    ) -> Result<serde_json::Value, WosError> {
        self.request_with_key(method, path, body, query, None).await
    }

    /// `request`, plus an optional `Idempotency-Key`. The key rides on EVERY attempt of
    /// this call — that is the point: if a retry ever does happen, the API replays the
    /// first response instead of applying the write again.
    async fn request_with_key(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
        query: Option<&[(&str, String)]>,
        idempotency_key: Option<&str>,
    ) -> Result<serde_json::Value, WosError> {
        let removes = method == reqwest::Method::DELETE;
        self.send(method, path, body, query, idempotency_key, removes).await
    }

    /// The request loop. `removes` marks a call that deletes something, so a 404 after
    /// an attempt that may have been applied can say it may already be gone.
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
        query: Option<&[(&str, String)]>,
        idempotency_key: Option<&str>,
        removes: bool,
    ) -> Result<serde_json::Value, WosError> {
        if let Some(k) = idempotency_key {
            if !valid_idempotency_key(k) {
                return Err(WosError::Api {
                    status: 400,
                    message: format!(
                        "invalid idempotency_key: {k:?} — 1-128 chars of [A-Za-z0-9._:-]"
                    ),
                });
            }
        }
        self.preflight()?;
        let url = format!("{}{}", self.base_url, path);
        // Saturating: `with_retries(u32::MAX)` means "retry as hard as you can", and a
        // wrapped count would mean "do not retry".
        let attempts = self.retries.saturating_add(1);
        let deadline_at = self.deadline_at();
        let idempotent = method.is_idempotent();
        // Set once an attempt ended without saying whether it was applied.
        let mut maybe_applied = false;
        let per_attempt = std::time::Duration::from_secs(self.timeout_secs);
        // The answer that led to this attempt, reported if the budget runs out first.
        let mut last_refusal: Option<Refusal> = None;
        let mut attempt: u32 = 0;
        loop {
            let start = std::time::Instant::now();
            let previous = last_refusal.take();
            let budget = match self.attempt_budget(deadline_at) {
                Ok(b) => b,
                Err(e) => return Err(previous.map_or(e, |r| r.into_error(removes && maybe_applied))),
            };
            let mut req = self
                .http
                .request(method.clone(), &url)
                .timeout(budget)
                .header("X-API-Key", &self.api_key)
                .header("X-WOS-Model", &self.model);
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
                    // The deadline ran out mid-attempt. A read reports the answer before
                    // it; otherwise the caller hears that the deadline ran out.
                    if ran_out_of_budget(&e, budget, per_attempt) {
                        return Err(match previous.filter(|_| idempotent) {
                            Some(r) => r.into_error(removes),
                            None => self.deadline_error(),
                        });
                    }
                    // Retry only when a retry cannot apply a write twice:
                    //   · a connect-level failure — the request never reached the server
                    //   · an idempotent method — re-running it changes nothing
                    // Timeouts are excluded from both: the write may already have been
                    // applied.
                    let safe = e.is_connect() || (idempotent && !e.is_timeout());
                    if safe && attempt + 1 < attempts {
                        let delay = backoff(attempt);
                        if !fits_in(delay, deadline_at) {
                            return Err(previous.map_or_else(
                                || self.deadline_error(),
                                |r| r.into_error(removes && maybe_applied),
                            ));
                        }
                        log_debug!(
                            "{method} {path}: {} — retrying in {}ms (attempt {}/{attempts})",
                            if e.is_connect() { "connect error" } else { "transport error" },
                            delay.as_millis(),
                            attempt + 1
                        );
                        maybe_applied |= !e.is_connect();
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(network_error(e));
                }
            };
            let status = resp.status();
            if status.is_redirection() {
                return Err(WosError::Api {
                    status: status.as_u16(),
                    message: "unexpected redirect — refused (the API key never follows a redirect). Check base_url: exact host, https://.".into(),
                });
            }
            self.record_rate_limit(resp.headers());
            if !status.is_success() {
                let left = budget.saturating_sub(start.elapsed());
                let refusal = Refusal::read(resp, idempotent, attempt, attempt + 1 < attempts, deadline_at, left).await;
                if let Some(delay) = refusal.wait {
                    if attempt + 1 < attempts && fits_in(delay, deadline_at) {
                        log_debug!(
                            "{method} {path} -> {} — retrying in {}ms (attempt {}/{attempts})",
                            status.as_u16(),
                            delay.as_millis(),
                            attempt + 1
                        );
                        maybe_applied |= status_is_ambiguous(status.as_u16());
                        last_refusal = Some(refusal);
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                }
                log_debug!(
                    "{method} {path} -> {} in {}ms (attempt {}/{attempts})",
                    status.as_u16(),
                    start.elapsed().as_millis(),
                    attempt + 1
                );
                return Err(refusal.into_error(removes && maybe_applied));
            }
            // Surface whether the write was stored or replayed. The server says so with
            // `Idempotent-Replayed: true`. Read it before `read_capped`, which consumes
            // the response.
            let replayed = resp
                .headers()
                .get("idempotent-replayed")
                .and_then(|v| v.to_str().ok())
                .map(|v| v == "true")
                .unwrap_or(false);
            // A drop WHILE READING the body is not a connect error — `send()` already
            // returned, so it surfaces here and nowhere else. Same rule as above: an
            // idempotent read can be re-run safely, a write cannot (it may already have
            // been applied). A timeout stays unretried either way.
            let text = match read_capped(resp).await {
                Ok(t) => t,
                Err(WosError::Network(ne)) if ran_out_of_budget(&ne, budget, per_attempt) => {
                    return Err(match previous.filter(|_| idempotent) {
                        Some(r) => r.into_error(removes),
                        None => self.deadline_error(),
                    });
                }
                Err(e) => {
                    let retry_body = match &e {
                        WosError::Network(ne) => idempotent && !ne.is_timeout(),
                        _ => false, // the size cap and friends are not transport failures
                    };
                    if retry_body && attempt + 1 < attempts {
                        let delay = backoff(attempt);
                        if !fits_in(delay, deadline_at) {
                            return Err(previous.map_or_else(
                                || self.deadline_error(),
                                |r| r.into_error(removes && maybe_applied),
                            ));
                        }
                        log_debug!(
                            "{method} {path}: body dropped — retrying in {}ms (attempt {}/{attempts})",
                            delay.as_millis(),
                            attempt + 1
                        );
                        maybe_applied = true;
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(e);
                }
            };
            log_debug!(
                "{method} {path} -> {} in {}ms (attempt {}/{attempts})",
                status.as_u16(),
                start.elapsed().as_millis(),
                attempt + 1
            );
            // An empty body is only legal when the STATUS says there is no body, where
            // it reads as `{}`. An empty body on any other 2xx means a response went
            // missing on the way, and passing that off as success hides it.
            if text.trim().is_empty() {
                if NO_BODY_STATUS.contains(&status.as_u16()) {
                    return Ok(serde_json::json!({}));
                }
                return Err(WosError::Api {
                    status: status.as_u16(),
                    message: "empty response body — expected a JSON object".into(),
                });
            }
            // A 200 with a corrupt/non-JSON body is a real failure — surface it,
            // don't silently return null (which reads as "no data").
            let v = serde_json::from_str::<serde_json::Value>(&text).map_err(|e| WosError::Api {
                status: status.as_u16(),
                message: format!("invalid JSON in response: {e}"),
            })?;
            // Every endpoint returns a JSON OBJECT, so a body that parses to null / a
            // number / an array is a broken server, not an empty result to swallow.
            if !v.is_object() {
                return Err(WosError::Api {
                    status: status.as_u16(),
                    message: "expected a JSON object in the response".into(),
                });
            }
            let mut v = v;
            // Only when the body does not already carry it: the service's own value
            // is the true one.
            if replayed {
                if let Some(o) = v.as_object_mut() {
                    o.entry("replayed").or_insert(serde_json::Value::Bool(true));
                }
            }
            return Ok(v);
        }
    }
}

// ─── Offline tests: a scripted localhost HTTP responder, no network. ────────
#[cfg(test)]
mod store_resolution_tests {
    use super::*;

    // These assert the DESTINATION of a call: a blank store id must never be sent on or
    // quietly replaced by the client's bound store.

    #[test]
    fn a_passed_blank_store_id_is_rejected_rather_than_redirected() {
        let c = Client::new("k").with_user("alice");
        for blank in ["", " ", "\t", "\n", "   "] {
            let e = c.uid(Some(blank)).expect_err("a blank id must not resolve to a store");
            match e {
                WosError::Api { status, ref message } => {
                    assert_eq!(status, 400, "client-side rejection uses 400, got {status}");
                    // Assert this guard's own wording, not a phrase the collapse warning
                    // also emits — otherwise the test passes on the wrong message.
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
        // The documented shortcut. Rejecting this too would break every bound client.
        let c = Client::new("k").with_user("alice");
        assert_eq!(c.uid(None).unwrap(), "alice");
    }

    #[test]
    fn a_real_store_id_is_sent_exactly_as_written() {
        // Not trimmed: trimming would be a second silent change of destination.
        let c = Client::new("k").with_user("alice");
        assert_eq!(c.uid(Some("bob")).unwrap(), "bob");
        assert_eq!(c.uid(Some(" bob ")).unwrap(), " bob ");
    }
}

#[cfg(test)]
mod tests {
    /// `Instant + Duration` panics on overflow; a deadline past the clock's range must
    /// still resolve to a usable budget.
    #[test]
    fn a_deadline_past_the_clocks_range_does_not_panic() {
        for d in [
            std::time::Duration::MAX,
            std::time::Duration::from_secs(u64::MAX / 2),
            std::time::Duration::from_secs(u64::MAX),
        ] {
            let c = Client::new("wos-live-kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk").with_deadline(d);
            let at = c.deadline_at().expect("a non-zero deadline is Some");
            assert!(at > std::time::Instant::now(), "{d:?} must resolve to a future instant");
            assert!(c.attempt_budget(Some(at)).is_ok(), "{d:?} must leave a usable budget");
        }
        // ZERO still means "no budget", and an ordinary one still bounds the call.
        assert!(Client::new("wos-live-kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk")
            .with_deadline(std::time::Duration::ZERO)
            .deadline_at()
            .is_none());
    }

    /// `retries + 1` must not wrap to 0: "retry as hard as you can" must not become
    /// "do not retry".
    #[test]
    fn the_largest_retry_count_still_retries() {
        let c = Client::new("wos-live-kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk").with_retries(u32::MAX);
        assert_eq!(c.retries.saturating_add(1), u32::MAX, "attempts must not wrap to 0");
    }

    /// with_user("") cannot leave a blank default that every later call resolves to.
    #[test]
    fn with_user_blank_cannot_silently_become_the_default_store() {
        let bound = Client::new("wos-live-kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk").with_user("tenant-a");
        assert_eq!(bound.uid(None).unwrap(), "tenant-a");
        assert_eq!(bound.uid(Some("tenant-b")).unwrap(), "tenant-b");

        for blank in ["", " ", "\t", "\n"] {
            let c = Client::new("wos-live-kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk").with_user(blank);
            let err = c.uid(None).unwrap_err();
            assert!(
                format!("{err}").contains("default store is blank"),
                "with_user({blank:?}) must not resolve to a store: {err}"
            );
            // The per-call form stays guarded.
            assert!(c.uid(Some(blank)).is_err(), "uid(Some({blank:?}))");
        }

        // The zero-setup path still works.
        assert_eq!(
            Client::new("wos-live-kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk").uid(None).unwrap(),
            "default"
        );
    }

    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Serve `responses` one connection each (Connection: close), counting hits.
    /// `mock_server`, but it also hands back the raw requests it received — needed to
    /// assert on HEADERS (the idempotency key), which the hit counter can't show.
    pub(super) fn mock_server_recording(responses: Vec<String>) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let s = seen.clone();
        std::thread::spawn(move || {
            for resp in responses {
                let Ok((mut sock, _)) = listener.accept() else { return };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 2048];
                loop {
                    match sock.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                s.lock().unwrap().push(String::from_utf8_lossy(&buf).to_string());
                let _ = sock.write_all(resp.as_bytes());
            }
        });
        (format!("http://{}", addr), seen)
    }

    pub(super) fn mock_server(responses: Vec<String>) -> (String, Arc<AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        std::thread::spawn(move || {
            for resp in responses {
                let Ok((mut sock, _)) = listener.accept() else { return };
                // Read until the header terminator (requests here are small).
                let mut buf = Vec::new();
                let mut chunk = [0u8; 2048];
                loop {
                    match sock.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                h.fetch_add(1, Ordering::SeqCst);
                let _ = sock.write_all(resp.as_bytes());
            }
        });
        (format!("http://{}", addr), hits)
    }

    pub(super) fn http(status: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        )
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retries_429_then_succeeds() {
        let (base, hits) = mock_server(vec![
            http("429 Too Many Requests", "Retry-After: 0\r\n", "{\"error\":\"rate limited\"}"),
            http("200 OK", "", "{\"memories\":[]}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let out = mem.search("q", "alice", 10).await.unwrap();
        assert!(out.is_empty());
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    /// `get_image` has its own request path; the key checks run on it too.
    #[tokio::test(flavor = "current_thread")]
    async fn get_image_refuses_a_bad_key_before_the_network() {
        let (base, hits) = mock_server(vec![]);
        for bad in ["", "wos-live-\u{0}abc", "wos-live-\u{1}abc", "wos live"] {
            let mem = Client::with_base_url(bad, &base);
            match mem.get_image(None, "11111111-1111-1111-1111-111111111111").await {
                Err(WosError::Api { status, .. }) => assert_eq!(status, 400, "key {bad:?}"),
                other => panic!("key {bad:?} should be refused, got {other:?}"),
            }
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing may reach the network");
    }

    /// The count is refused out of range, not adjusted: both ends, every search method.
    #[tokio::test(flavor = "current_thread")]
    async fn search_count_out_of_range_is_refused_not_adjusted() {
        let (base, hits) = mock_server(vec![]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for bad in [0usize, 1, 4, 21, 50, 100] {
            for r in [
                mem.search("q", "alice", bad).await.map(|_| ()),
                mem.search_full("q", "alice", bad, &SearchOpts::default()).await.map(|_| ()),
                mem.search_self("q", "alice", bad).await.map(|_| ()),
            ] {
                match r {
                    Err(WosError::Api { status, message }) => {
                        // 400, not 0: a caller's own mistake.
                        assert_eq!(status, 400, "a caller's mistake reads as a bad request");
                        assert!(message.contains("between 5 and 20"), "got {message}");
                    }
                    other => panic!("limit {bad} should be refused, got {other:?}"),
                }
            }
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing may reach the network");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn no_retry_on_400_and_parses_envelope() {
        let (base, hits) = mock_server(vec![http(
            "400 Bad Request",
            "",
            "{\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"boom\",\"request_id\":\"req_123\"}}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.search("q", "alice", 10).await {
            Err(WosError::Api { status, message }) => {
                assert_eq!(status, 400);
                assert!(message.contains("boom"));
                assert!(message.contains("req_123"));
            }
            other => panic!("expected Api error, got {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn refuses_redirects() {
        let (base, _) = mock_server(vec![http("302 Found", "Location: http://evil.example/\r\n", "")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.stats("alice").await {
            Err(WosError::Api { status, message }) => {
                assert_eq!(status, 302);
                assert!(message.contains("redirect"));
            }
            other => panic!("expected redirect refusal, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn delete_requires_memory_id() {
        // No server: the guard must trip before any request is sent.
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:9");
        match mem.delete("alice", "").await {
            Err(WosError::Api { status, message }) => {
                assert_eq!(status, 400);
                assert!(message.contains("delete_all"));
            }
            other => panic!("expected guard error, got {other:?}"),
        }
        match mem.delete_all("").await {
            Err(WosError::Api { status, .. }) => assert_eq!(status, 400),
            other => panic!("expected guard error, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn image_calls_require_a_memory_id() {
        // A blank id is refused locally, before any request.
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:9");
        for r in [
            mem.get_image("alice", "").await.err(),
            mem.get_image("alice", "   ").await.err(),
            mem.forget_image("alice", "", false).await.err(),
            mem.forget_image("alice", "  ", true).await.err(),
            mem.lineage("alice", "").await.err(),
            mem.lineage("alice", "   ").await.err(),
        ] {
            match r {
                Some(WosError::Api { status, message }) => {
                    assert_eq!(status, 400);
                    assert!(message.contains("memory_id is required"), "got {message}");
                }
                other => panic!("expected guard error, got {other:?}"),
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_call_that_names_no_model_carries_the_default() {
        // Against the constant, not a literal: a spelled-out model name here keeps
        // passing the day the default changes, which is the day it matters.
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.stats("alice").await.unwrap();
        let reqs = seen.lock().unwrap();
        assert!(
            reqs[0].to_lowercase().contains(&format!("x-wos-model: {DEFAULT_MODEL}").to_lowercase()),
            "got {}",
            reqs[0]
        );
    }

    /// Serve `n` connections that send headers promising a body, then drop mid-body,
    /// and answer every connection after that with `tail`. Returns the hit count.
    pub(super) fn dropping_server(drops: usize, tail: String) -> (String, Arc<AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        std::thread::spawn(move || {
            let mut served = 0usize;
            while let Ok((mut sock, _)) = listener.accept() {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 2048];
                loop {
                    match sock.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                h.fetch_add(1, Ordering::SeqCst);
                if served < drops {
                    // Promise 512 bytes, send 5, hang up. Headers arrived, so this is
                    // NOT a connect failure — it is a drop while reading the body.
                    let _ = sock.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 512\r\n\r\n{\"co",
                    );
                } else {
                    let _ = sock.write_all(tail.as_bytes());
                }
                served += 1;
                let _ = sock.shutdown(std::net::Shutdown::Both);
            }
        });
        (format!("http://{}", addr), hits)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_is_retried_when_the_body_drops_mid_stream() {
        // A GET whose body drops mid-stream is retried, though the drop is not a
        // connect error.
        let (base, hits) = dropping_server(1, http("200 OK", "", r#"{"collections":[],"count":0}"#));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_retries(2);
        let got = mem.list_stores().await;
        assert!(got.is_ok(), "a dropped GET body should be retried, got {got:?}");
        assert_eq!(hits.load(Ordering::SeqCst), 2, "one drop + one good response");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_write_is_never_retried_when_the_body_drops_mid_stream() {
        // The other half, and the reason the rule is not simply "retry everything":
        // headers came back, so the write may already have been applied.
        let (base, hits) = dropping_server(1, http("200 OK", "", r#"{"id":"m1","status":"stored"}"#));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_retries(2);
        let got = mem.add("hello", "alice", serde_json::json!({})).await;
        assert!(got.is_err(), "a dropped POST body must NOT be retried");
        assert_eq!(hits.load(Ordering::SeqCst), 1, "the write must be attempted exactly once");
    }

    #[test]
    fn debug_never_shows_the_key() {
        let mem = Client::new("wos-live-supersecretkeyvalue1234");
        let dbg = format!("{mem:?}");
        assert!(!dbg.contains("supersecretkeyvalue"));
        assert!(dbg.contains("wos-"));
        assert!(dbg.contains("1234"));
    }

    #[test]
    fn backoff_honors_retry_after_and_caps() {
        assert_eq!(retry_wait(0, Some("2")).unwrap().as_millis(), 2000);
        assert_eq!(retry_wait(0, Some("30")).unwrap().as_millis(), 30_000);
        // Above the cap the client does not wait at all; the caller gets the 429.
        assert_eq!(retry_wait(0, Some("999")), None);
        let b = retry_wait(3, None).unwrap().as_millis();
        assert!((4000..=4250).contains(&b), "got {b}");
        let cap = retry_wait(10, None).unwrap().as_millis();
        assert!((8000..=8250).contains(&cap), "got {cap}");
    }

    #[test]
    fn only_delta_seconds_or_an_http_date_count_as_retry_after() {
        for junk in ["-5", "5, 10", "0x2", "garbage", "", " ", "1.5", "+5", "inf", "NaN", "1e3", "٣"] {
            let d = retry_wait(0, Some(junk)).expect("junk falls back to the backoff").as_millis();
            assert!((500..750).contains(&d), "{junk:?} waited {d}ms instead of the backoff");
        }
        assert_eq!(retry_wait(0, Some("0")).unwrap().as_millis(), 0);
        assert_eq!(retry_wait(0, Some("99999999999999999999999")), None, "huge is still above the cap");
    }

    // ----- security round 2 -----

    #[test]
    fn key_is_trimmed() {
        let mem = Client::new(" wos-test-xxxxxxxxxx\n");
        let dbg = format!("{mem:?}");
        assert!(dbg.contains("wos-"));
        assert!(!dbg.contains('\n'));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn invalid_model_name_errors_before_sending() {
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:9").with_model("bad model");
        match mem.stats("alice").await {
            Err(WosError::Api { status, message }) => {
                assert_eq!(status, 400);
                assert!(message.contains("invalid model name"));
            }
            other => panic!("expected model validation error, got {other:?}"),
        }
    }

    #[test]
    fn from_env_reads_the_key() {
        std::env::set_var("WONTOPOS_API_KEY", "wos-test-envkey12345");
        let mem = Client::from_env().unwrap();
        assert!(format!("{mem:?}").contains("wos-"));
        std::env::remove_var("WONTOPOS_API_KEY");
        std::env::remove_var("WOS_API_KEY");
        assert!(Client::from_env().is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn refuses_oversized_responses() {
        // Content-Length lies huge; the cap must trip before any body read.
        let (base, _) = mock_server(vec![format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{{}}",
            65 * 1024 * 1024
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.stats("alice").await {
            Err(WosError::Api { message, .. }) => assert!(message.contains("too large"), "got {message}"),
            other => panic!("expected size-cap error, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn delete_all_rejects_blank_and_whitespace() {
        // Blank OR whitespace would resolve to the "default" store server-side and
        // wipe it — the guard must reject before any request (no base_url needed).
        let mem = Client::new("wos-test-xxxxxxxxxx");
        for uid in ["", " ", "\t", "\n", "   "] {
            assert!(mem.delete_all(uid).await.is_err(), "uid {uid:?} must be rejected");
        }
    }

    #[test]
    fn backoff_honors_retry_after_seconds_and_http_date() {
        assert_eq!(retry_wait(0, Some("2")).unwrap().as_millis(), 2000);
        // HTTP-date in the past → retry immediately (0), not exponential backoff.
        assert_eq!(retry_wait(3, Some("Wed, 21 Oct 2015 07:28:00 GMT")).unwrap().as_millis(), 0);
        // A future HTTP-date → a positive wait inside the cap.
        let future = httpdate::fmt_http_date(std::time::SystemTime::now() + std::time::Duration::from_secs(5));
        let d = retry_wait(0, Some(&future)).unwrap().as_millis();
        assert!(d > 0 && d <= 5_000, "got {d}");
        // An hour ahead is above the cap.
        let later = httpdate::fmt_http_date(std::time::SystemTime::now() + std::time::Duration::from_secs(3600));
        assert_eq!(retry_wait(0, Some(&later)), None);
    }


    #[tokio::test(flavor = "current_thread")]
    async fn non_object_2xx_body_is_error() {
        // A 2xx that parses to a non-object (null / number / string / array) is a
        // broken server, not an empty result to swallow.
        for body in ["null", "12345", "\"hi\"", "[1,2,3]"] {
            let (base, _) = mock_server(vec![http("200 OK", "", body)]);
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
            match mem.stats("u").await {
                Err(WosError::Api { message, .. }) => assert!(message.contains("object"), "got {message}"),
                other => panic!("expected object error for {body}, got {other:?}"),
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn null_scalar_fields_default_instead_of_failing_the_batch() {
        // A memory element with an explicit `null` on a non-Option scalar
        // (content / category / similarity / importance / is_superseded) must NOT
        // fail the whole search: the good elements survive and each null becomes
        // the field's default.
        let body = "{\"memories\":[\
            {\"id\":\"1\",\"content\":\"good\",\"similarity\":0.9},\
            {\"id\":\"2\",\"content\":null,\"category\":null,\"similarity\":null,\"importance\":null,\"is_superseded\":null}\
        ]}";
        let (base, _) = mock_server(vec![http("200 OK", "", body)]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let out = mem.search("q", "u", 5).await.expect("null scalars must not fail the batch");
        assert_eq!(out.len(), 2, "both memories survive the null element");
        assert_eq!(out[0].content, "good");
        assert_eq!(out[1].content, ""); // null -> default
        assert_eq!(out[1].similarity, 0.0);
        assert!(!out[1].is_superseded);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_self_returns_both_fields_null_self_is_empty() {
        // search_self returns both fields from one call; missing/null self_memories → [].
        let (base, _) = mock_server(vec![
            http("200 OK", "", "{\"memories\":[{\"id\":\"m1\"}],\"self_memories\":[{\"id\":\"s1\"}]}"),
            http("200 OK", "", "{\"memories\":[{\"id\":\"m2\"}],\"self_memories\":null}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let r = mem.search_self("q", "alice", 5).await.unwrap();
        assert_eq!(r.memories.len(), 1);
        assert_eq!(r.self_memories.len(), 1);
        assert_eq!(r.self_memories[0].id.as_deref(), Some("s1"));
        let r2 = mem.search_self("q", "u", 5).await.unwrap(); // non-self model: null → []
        assert_eq!(r2.self_memories.len(), 0);
        assert_eq!(r2.memories[0].id.as_deref(), Some("m2"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_returns_both_fields() {
        // Some models answer with the assistant's own words in `self_memories`, not
        // repeated in `memories`. Both fields come back, de-duplicated.
        let (base, _) = mock_server(vec![http(
            "200 OK",
            "",
            "{\"memories\":[{\"id\":\"m1\",\"content\":\"partner said this\"}],\
              \"self_memories\":[{\"id\":\"s1\",\"content\":\"I said this\",\"speaker\":\"me\"},\
                                 {\"id\":\"m1\",\"content\":\"dup\"}]}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let out = mem.search("q", "u", 5).await.unwrap();
        let ids: Vec<&str> = out.iter().filter_map(|m| m.id.as_deref()).collect();
        assert_eq!(ids, vec!["m1", "s1"], "both fields, id-deduplicated");
        assert_eq!(out[1].speaker.as_deref(), Some("me"), "self_memories keeps its speaker");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bad_elements_are_skipped_not_fatal() {
        // One malformed element must not fail the whole call: skip it and keep the
        // good records.
        let (base, _) = mock_server(vec![
            http("200 OK", "", "{\"memories\":[null,1,\"x\",[],{\"id\":\"ok\",\"content\":\"c\"}]}"),
            http(
                "200 OK",
                "",
                "{\"memories\":[null,{\"id\":\"m1\"}],\"self_memories\":[2,{\"id\":\"s1\"}]}",
            ),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let out = mem.search("q", "u", 5).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id.as_deref(), Some("ok"));
        let r = mem.search_self("q", "u", 5).await.unwrap();
        assert_eq!(r.memories.len(), 1);
        assert_eq!(r.memories[0].id.as_deref(), Some("m1"));
        assert_eq!(r.self_memories.len(), 1);
        assert_eq!(r.self_memories[0].id.as_deref(), Some("s1"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_coerces_null_or_nonarray_memories_to_empty() {
        // A server sending `memories: null` or a truthy non-array (`"oops"` / a number)
        // must yield [], not an "invalid type" error, on search and search_with alike.
        for body in ["{\"memories\":null}", "{\"memories\":\"oops\"}", "{\"memories\":42}", "{}"] {
            let (base, _) = mock_server(vec![http("200 OK", "", body), http("200 OK", "", body)]);
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
            let out = mem.search("q", "u", 5).await.expect("null/non-array memories must coerce to []");
            assert!(out.is_empty(), "search: expected [] for {body}");
            let out2 = mem.search_with("q", "u", 5, serde_json::json!({"speaker": "me"})).await.expect("search_with too");
            assert!(out2.is_empty(), "search_with: expected [] for {body}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn recall_with_and_engram_with_send_form() {
        // recall_with / engram_with merge form+tz into the body (the docs' recall/engram forms).
        let (base, _) = mock_server(vec![http("200 OK", "", "{\"short_term\":{},\"long_term\":{},\"context\":\"\"}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let r = mem.recall_with("q", "u", serde_json::json!({"form": "archive", "tz": 9})).await;
        assert!(r.is_ok(), "recall_with should send form and parse the response");
    }

    #[test]
    fn error_message_is_capped() {
        let long = "Z".repeat(100_000);
        let (msg, _) = parse_error(&format!("{{\"error\":\"{long}\"}}"));
        assert!(msg.chars().count() <= MAX_ERR_MSG + 20);
        assert!(msg.contains("truncated"));
        // A multibyte tail must not panic the char-based truncation.
        // A 3-byte UTF-8 char in one UTF-16 unit: the shape a char-based cap misjudges.
        let multi = "\u{4E00}".repeat(100_000);
        let (m2, _) = parse_error(&format!("{{\"error\":\"{multi}\"}}"));
        assert!(m2.chars().count() <= MAX_ERR_MSG + 20);
    }

    #[test]
    fn user_agent_carries_platform_info() {
        let ua = user_agent();
        assert!(ua.starts_with("wontopos-rust/"), "got {ua}");
        assert!(ua.contains(std::env::consts::OS) && ua.contains(std::env::consts::ARCH), "got {ua}");
    }

    #[test]
    fn mask_key_handles_unicode_without_panic() {
        // A key with multibyte chars must not panic on byte-boundary slicing.
        let masked = mask_key("wos-caf\u{e9}-\u{4E00}\u{4E8C}-se\u{f1}or-key");
        assert!(masked.contains("..."));
        assert_eq!(mask_key("sh\u{f6}rt"), "***"); // 12 chars or fewer -> fully hidden
    }

    #[tokio::test(flavor = "current_thread")]
    async fn post_write_not_retried_on_502() {
        // A 502 on a write (POST) must NOT retry — the write may already have been applied.
        let (base, hits) = mock_server(vec![http("502 Bad Gateway", "", "{\"error\":\"bad gateway\"}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let r = mem.add("hi", "u", serde_json::json!({})).await;
        assert!(matches!(r, Err(WosError::Api { status: 502, .. })));
        assert_eq!(hits.load(Ordering::SeqCst), 1); // no retry
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_retried_on_503() {
        // 503 on an idempotent GET (/models) is safe to retry.
        let (base, hits) = mock_server(vec![
            http("503 Service Unavailable", "Retry-After: 0\r\n", "{}"),
            http("200 OK", "", "{\"models\":[]}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.list_models().await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 2); // retried
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bad_json_on_200_is_an_error_not_empty() {
        // A 200 with a corrupt body must surface an error, not read as "no data".
        let (base, _) = mock_server(vec![http("200 OK", "", "not valid json{")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.search("q", "u", 5).await {
            Err(WosError::Api { message, .. }) => assert!(message.contains("invalid JSON"), "got {message}"),
            other => panic!("expected invalid-JSON error, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn whitespace_in_key_errors_before_sending() {
        // The key is trimmed at construction; REMAINING (inner) whitespace is a
        // paste error that would read back as a mystery 401 — fail it clearly,
        // before any request.
        let mem = Client::with_base_url("wos-test xxxxxxxxxx", "http://127.0.0.1:9");
        match mem.stats("alice").await {
            Err(WosError::Api { status, message }) => {
                assert_eq!(status, 400);
                assert!(message.contains("whitespace"), "got {message}");
            }
            other => panic!("expected whitespace-key error, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn with_timeout_zero_falls_back_to_default() {
        // A zero timeout would fail every request instantly — it must fall back
        // to the default and still complete a normal call.
        let (base, _) = mock_server(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_timeout(0);
        mem.stats("alice").await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_unwraps_memory_and_guards_empty_id() {
        let (base, _) = mock_server(vec![http(
            "200 OK",
            "",
            "{\"user_id\":\"u\",\"memory\":{\"id\":\"9b2d\",\"content\":\"tea\",\"is_superseded\":false}}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let m = mem.get("u", "9b2d").await.unwrap();
        assert_eq!(m["content"], "tea");
        // Empty id must trip the guard before any request (no server needed).
        let off = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:9");
        match off.get("u", " ").await {
            Err(WosError::Api { status: 400, message }) => assert!(message.contains("memory_id")),
            other => panic!("expected guard error, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_takes_a_flat_row() {
        let (base, _) = mock_server(vec![
            http("200 OK", "", "{\"id\":\"9b2d\",\"content\":\"tea\"}"),
            http("200 OK", "", "{\"user_id\":\"u\",\"memory\":null}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let m = mem.get("u", "9b2d").await.unwrap();
        assert_eq!(m["content"], "tea");
        assert_eq!(mem.get("u", "9b2d").await.unwrap(), serde_json::json!({}));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_returns_image_rows_after_the_text_each_id_once() {
        let payload = "{\"memories\":[{\"id\":\"m1\"},{\"id\":\"dup\"}],\"self_memories\":[{\"id\":\"s1\"}],\
                       \"images\":[{\"id\":\"dup\"},{\"id\":\"i1\",\"content\":\"\"}]}";
        let (base, _) = mock_server(vec![http("200 OK", "", payload), http("200 OK", "", payload)]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let ids: Vec<_> = mem.search("q", "alice", 10).await.unwrap().into_iter().filter_map(|m| m.id).collect();
        assert_eq!(ids, ["m1", "dup", "s1", "i1"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_all_memories_refuses_a_repeated_cursor() {
        // A cursor that comes back after a page with rows in it means the walk cannot
        // finish: stop after two requests and say the answer is truncated.
        let (base, hits) = mock_server(vec![
            http("200 OK", "", "{\"memories\":[{\"id\":\"1\"}],\"next_cursor\":\"C\"}"),
            http("200 OK", "", "{\"memories\":[{\"id\":\"2\"}],\"next_cursor\":\"C\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.list_all_memories("u").await {
            Err(WosError::Api { status: 200, message }) => {
                assert!(message.contains("truncated"), "got {message}");
                assert!(!message.contains('\u{2014}'), "got {message}");
            }
            other => panic!("a repeated cursor must not read as a complete store, got {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_all_memories_refuses_a_cursor_cycle() {
        let (base, hits) = mock_server(vec![
            http("200 OK", "", "{\"memories\":[{\"id\":\"1\"}],\"next_cursor\":\"A\"}"),
            http("200 OK", "", "{\"memories\":[{\"id\":\"2\"}],\"next_cursor\":\"B\"}"),
            http("200 OK", "", "{\"memories\":[{\"id\":\"3\"}],\"next_cursor\":\"A\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem.list_all_memories("u").await.unwrap_err();
        assert!(format!("{e}").contains("truncated"), "got {e}");
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_all_memories_ends_on_an_empty_page_that_repeats_its_cursor() {
        // An empty page has nothing left to lose, so a repeated cursor there is the end.
        let (base, hits) = mock_server(vec![
            http("200 OK", "", "{\"memories\":[{\"id\":\"1\"}],\"next_cursor\":\"C\"}"),
            http("200 OK", "", "{\"memories\":[],\"next_cursor\":\"C\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let all = mem.list_all_memories("u").await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    /// The image cursor is not `before` alone, it is **the pair `before` and `skip_ids`**.
    ///
    /// Several images can share a timestamp, so `before` does not move for as long as it
    /// takes to walk that group. Judging progress by `before` alone reads "stuck" on the
    /// second page and stops, and the remaining images **simply never arrive, with no
    /// error** — the kind you conclude were never there.
    #[tokio::test(flavor = "current_thread")]
    async fn all_images_keeps_walking_when_images_share_a_timestamp() {
        let ts = "2026-08-19T00:00:00Z";
        let (base, hits) = mock_server(vec![
            // Same `before`, growing `skip_ids` — three images share one timestamp.
            http("200 OK", "", &format!("{{\"images\":[{{\"id\":\"a\"}}],\"has_more\":true,\"next_before\":\"{ts}\",\"next_skip_ids\":[\"a\"]}}")),
            http("200 OK", "", &format!("{{\"images\":[{{\"id\":\"b\"}}],\"has_more\":true,\"next_before\":\"{ts}\",\"next_skip_ids\":[\"a\",\"b\"]}}")),
            http("200 OK", "", &format!("{{\"images\":[{{\"id\":\"c\"}}],\"has_more\":true,\"next_before\":\"{ts}\",\"next_skip_ids\":[\"a\",\"b\",\"c\"]}}")),
            http("200 OK", "", "{\"images\":[{\"id\":\"d\"}],\"has_more\":false}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let all = mem.list_all_images("u", None).await.unwrap();
        let ids: Vec<&str> = all.iter().filter_map(|m| m["id"].as_str()).collect();
        assert_eq!(ids, ["a", "b", "c", "d"], "stopped partway through one timestamp's images");
        assert_eq!(hits.load(Ordering::SeqCst), 4);
    }

    /// A server that keeps returning the same pair must not hang the walk, and must not
    /// pass for the end of the store either.
    #[tokio::test(flavor = "current_thread")]
    async fn all_images_refuses_a_repeated_cursor_pair() {
        let page = "{\"images\":[{\"id\":\"x\"}],\"has_more\":true,\"next_before\":\"T\",\"next_skip_ids\":[\"x\"]}";
        let (base, hits) = mock_server(vec![http("200 OK", "", page), http("200 OK", "", page)]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.list_all_images("u", None).await {
            Err(WosError::Api { status: 200, message }) => {
                assert!(message.contains("truncated"), "got {message}");
                assert!(!message.contains('\u{2014}'), "got {message}");
            }
            other => panic!("a repeated cursor pair must not read as every image, got {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    // A `204 No Content` is a success (`{}`), not a JSON parse error.
    #[tokio::test(flavor = "current_thread")]
    async fn no_content_is_a_success_not_a_parse_error() {
        let (base, _) = mock_server(vec![http("204 No Content", "", "")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert_eq!(mem.delete("alice", "m1").await.unwrap(), serde_json::json!({}));
    }

    // `Idempotency-Key` is accepted on the write routes. Pin that the header actually
    // rides on the write rather than being dropped on the way out.
    #[tokio::test(flavor = "current_thread")]
    async fn idempotency_key_rides_on_the_write() {
        let (base, seen) = mock_server_recording(vec![
            http("200 OK", "", "{\"id\":\"m1\",\"status\":\"stored\"}"),
            http("200 OK", "", "{}"),
            http("200 OK", "", "{}"),
            http("200 OK", "", "{}"),
            http("200 OK", "", "{}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = serde_json::json!({});
        mem.add_idempotent("x", "alice", e.clone(), "import:row-42").await.unwrap();
        mem.add_turn_idempotent("hi", "yo", "alice", "k2").await.unwrap();
        mem.add_bulk_idempotent("blob", "alice", "general", "k3").await.unwrap();
        mem.update_idempotent("m1", "new", "alice", "k4").await.unwrap();
        mem.add("x", "alice", e).await.unwrap(); // no key → no header
        let reqs = seen.lock().unwrap();
        let keys: Vec<bool> = reqs.iter().map(|r| r.to_lowercase().contains("idempotency-key:")).collect();
        assert_eq!(keys, vec![true, true, true, true, false], "reqs: {reqs:?}");
        assert!(reqs[0].contains("import:row-42"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn malformed_idempotency_key_fails_before_the_request() {
        // A server-side 400 on a retry path reads as "my write failed" when it never ran.
        let off = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:9");
        for bad in ["caf\u{e9} key", &"a".repeat(129), "", "has space"] {
            match off.add_idempotent("x", "alice", serde_json::json!({}), bad).await {
                Err(WosError::Api { status: 400, message }) => {
                    assert!(message.contains("invalid idempotency_key"), "got {message}")
                }
                other => panic!("expected a local reject for {bad:?}, got {other:?}"),
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn revisions_goes_to_the_won_surface() {
        // Calls a model makes ABOUT its memory live under /api/v1/won/*.
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"revised\":3,\"total\":40}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.revisions("alice").await.unwrap();
        let reqs = seen.lock().unwrap();
        assert!(reqs[0].contains("POST /api/v1/won/revisions"), "got {}", reqs[0]);
        assert!(!reqs[0].contains("/api/v1/memory/revisions"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_full_keeps_what_search_merges_away() {
        // `search` answers with one merged Vec; search_full keeps the photos and the
        // count of re-ask passes actually run.
        let payload = "{\"memories\":[{\"id\":\"m1\"}],\"self_memories\":[{\"id\":\"s1\"}],\
                       \"images\":[{\"id\":\"i1\"}],\"verify_used\":2}";
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", payload)]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let r = mem
            .search_full("q", "alice", 10, &SearchOpts { verify: Some(2), max_images: Some(3) })
            .await
            .unwrap();
        assert_eq!(r.memories.len(), 1);
        assert_eq!(r.self_memories.len(), 1);
        assert_eq!(r.images.len(), 1);
        assert_eq!(r.verify_used, Some(2));
        let reqs = seen.lock().unwrap();
        assert!(reqs[0].contains("\"max_images\":3"), "got {}", reqs[0]);
        assert!(reqs[0].contains("\"verify\":2"), "got {}", reqs[0]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_full_normalizes_a_nulled_field() {
        let (base, _seen) = mock_server_recording(vec![http(
            "200 OK",
            "",
            "{\"memories\":null,\"images\":null}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let r = mem.search_full("q", "alice", 10, &SearchOpts::default()).await.unwrap();
        assert!(r.memories.is_empty());
        assert!(r.images.is_empty());
        assert!(r.self_memories.is_empty());
        assert_eq!(r.verify_used, None, "absent means it was never asked for");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_opts_names_verify_and_max_images() {
        // A typed options struct cannot carry a typo: `verfy` is a compile error here,
        // where an untyped bag has to be checked at runtime.
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"memories\":[]}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.search_opts("q", "alice", 10, &SearchOpts { verify: Some(2), max_images: Some(5) })
            .await
            .unwrap();
        let reqs = seen.lock().unwrap();
        assert!(reqs[0].contains("\"verify\":2"), "got {}", reqs[0]);
        assert!(reqs[0].contains("\"max_images\":5"), "got {}", reqs[0]);
        // Reserved fields must survive on top of opts — that is why this goes through
        // search_with.
        assert!(reqs[0].contains("\"user_id\":\"alice\""), "got {}", reqs[0]);
        assert!(reqs[0].contains("\"max_results\":10"), "got {}", reqs[0]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_opts_default_sends_neither_field() {
        // Re-ask is billed by what it delivers. If Default quietly turned something on,
        // the bill would rise for a caller who changed nothing.
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"memories\":[]}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.search_opts("q", "alice", 10, &SearchOpts::default()).await.unwrap();
        let reqs = seen.lock().unwrap();
        assert!(!reqs[0].contains("verify"), "got {}", reqs[0]);
        assert!(!reqs[0].contains("max_images"), "got {}", reqs[0]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn revisions_asks_for_counts_only() {
        // What this test protects: **the response stays the same size for a store of a
        //   hundred million memories.** It can only be checked by what is NOT sent, never
        //   by a value. Slip a default into `include` one day and calls that asked for
        //   nothing start carrying twenty rows.
        let (base, seen) = mock_server_recording(vec![http(
            "200 OK",
            "",
            "{\"revised\":3,\"unrevised\":37,\"total\":40}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.revisions("alice").await.unwrap();
        let reqs = seen.lock().unwrap();
        assert!(!reqs[0].contains("include"), "a counts call carried include: {}", reqs[0]);
        assert!(!reqs[0].contains("limit"), "a counts call carried limit: {}", reqs[0]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn revisions_page_sends_the_cursor_under_its_wire_name() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"memories\":[]}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let skip = vec!["11111111-1111-1111-1111-111111111111".to_string()];
        mem.revisions_page("alice", "unrevised", 20usize, "2026-08-20T01:00:00Z", Some(&skip))
            .await
            .unwrap();
        let reqs = seen.lock().unwrap();
        assert!(reqs[0].contains("POST /api/v1/won/revisions"), "got {}", reqs[0]);
        assert!(reqs[0].contains("\"include\":\"unrevised\""), "got {}", reqs[0]);
        assert!(reqs[0].contains("\"limit\":20"), "got {}", reqs[0]);
        assert!(reqs[0].contains("\"before\":\"2026-08-20T01:00:00Z\""), "got {}", reqs[0]);
        // If the cursor loses its name the service reads "no cursor", and the caller
        // gets **page one forever**, without a single error.
        assert!(reqs[0].contains("\"skip_ids\""), "cursor did not go out as snake_case: {}", reqs[0]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_filters_reach_the_api() {
        // `filters` worked on the API but appeared in neither the spec nor any SDK.
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"memories\":[]}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.search_with("q", "alice", 10, serde_json::json!({
            "filters": {"categories": ["work"], "event_from": "2026-01-01"}
        })).await.unwrap();
        let reqs = seen.lock().unwrap();
        assert!(reqs[0].contains("\"categories\":[\"work\"]"), "got {}", reqs[0]);
        assert!(reqs[0].contains("\"event_from\":\"2026-01-01\""));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_with_carries_an_image_and_strips_what_breaks_it() {
        // `base64` and `openssl base64` wrap at 76 columns; those newlines, and a
        // data: prefix, are stripped before sending.
        let flat = "AAAABBBBCCCCDDDD".repeat(12); // 192 chars, longer than one wrap
        let wrapped: String = flat
            .as_bytes()
            .chunks(76)
            .map(|c| String::from_utf8_lossy(c).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(wrapped.contains('\n'), "the fixture must actually be wrapped");

        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"id\":\"m1\"}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.add_with(
            "a striped test card",
            "alice",
            serde_json::json!({}),
            serde_json::json!({"image": {
                "data": format!("data:image/png;base64,{wrapped}"),
                "taken_at": "2026-08-15T09:00:00Z",
            }}),
        )
        .await
        .unwrap();

        let reqs = seen.lock().unwrap();
        assert!(reqs[0].contains(&format!("\"data\":\"{flat}\"")), "got {}", reqs[0]);
        assert!(!reqs[0].contains("data:image/png"), "the data: prefix must not be sent");
        assert!(!reqs[0].contains("\\n"), "no line break may survive into the payload");
        assert!(reqs[0].contains("\"taken_at\":\"2026-08-15T09:00:00Z\""), "siblings must pass through");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_with_lets_reserved_fields_win() {
        // Same rule as recall_with/search_with: extra may add fields, never rewrite the
        // three that identify the call. A caller who passes content in `extra` must not
        // be able to store something other than what they passed as `content`.
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"id\":\"m1\"}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.add_with(
            "the real one",
            "alice",
            serde_json::json!({"speaker": "me"}),
            serde_json::json!({"content": "smuggled", "user_id": "bob", "categories": ["work"]}),
        )
        .await
        .unwrap();

        let reqs = seen.lock().unwrap();
        assert!(reqs[0].contains("\"content\":\"the real one\""), "got {}", reqs[0]);
        assert!(!reqs[0].contains("smuggled"));
        assert!(reqs[0].contains("\"user_id\":\"alice\""));
        assert!(!reqs[0].contains("bob"));
        assert!(reqs[0].contains("\"speaker\":\"me\""));
        assert!(reqs[0].contains("\"categories\":[\"work\"]"), "extra fields still reach the API");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_with_refuses_an_image_that_is_only_a_prefix() {
        // A data: URL with nothing after the comma is an empty image, and the service
        // answers "not a readable image" — a message that points at the file. Say it here.
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:1");
        match mem
            .add_with("a caption", "alice", serde_json::json!({}), serde_json::json!({"image": {"data": "data:image/png;base64,"}}))
            .await
        {
            Err(WosError::Api { status: 400, message }) => {
                assert!(message.contains("empty"), "got {message}")
            }
            other => panic!("expected a 400 about an empty image, got {other:?}"),
        }
        match mem
            .add_with("a caption", "alice", serde_json::json!({}), serde_json::json!({"image": {"reference": "s3://x"}}))
            .await
        {
            Err(WosError::Api { status: 400, message }) => {
                assert!(message.contains("image.data is required"), "got {message}")
            }
            other => panic!("expected a 400 about a missing data field, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_deadline_bounds_the_whole_call_not_one_attempt() {
        // A deadline bounds the call across every attempt. The 5-second Retry-After
        // below does not fit in it, so the call stops at once and reports the 429 it
        // got rather than a budget error.
        let (base, hits) = mock_server(vec![
            http(
                "429 Too Many Requests",
                "Retry-After: 5\r\n",
                "{\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"rate limited\",\"request_id\":\"req_rl\"}}",
            ),
            http("200 OK", "", "{\"memories\":[]}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(200));
        let t0 = std::time::Instant::now();
        let e = mem.search("q", "alice", 10).await.unwrap_err();
        assert_eq!(e.status(), Some(429), "got {e}");
        assert!(e.is_rate_limited(), "got {e}");
        assert!(format!("{e}").contains("req_rl"), "the request id must survive: {e}");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert!(t0.elapsed().as_secs_f64() < 0.15, "spent {:?} on a 200ms budget", t0.elapsed());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_clone_keeps_the_deadline() {
        // A clone that dropped the deadline would sleep out the 5s Retry-After.
        let (base, _) = mock_server(vec![http(
            "429 Too Many Requests", "Retry-After: 5\r\n", "{}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(200))
            .with_model("tablet-2");
        let t0 = std::time::Instant::now();
        assert!(mem.search("q", "alice", 10).await.is_err());
        assert!(t0.elapsed().as_secs_f64() < 1.0, "the clone slept for {:?}", t0.elapsed());
    }

    // ── recall's promise, kept ────────────────────────────────────────────
    #[tokio::test(flavor = "current_thread")]
    async fn recall_refuses_a_count_out_of_range_without_sending() {
        // "Out of range is refused, not clamped", before a socket is opened. The mock
        // is handed a response it must never get to serve.
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for bad in [0usize, 1, 4, 21, 500] {
            let opts = RecallOpts { limit: Some(bad), context_limit: None };
            match mem.recall_opts("q", "alice", &opts).await {
                Err(WosError::Api { status: 400, message }) => {
                    assert!(message.contains("between 5 and 20"), "got {message}")
                }
                other => panic!("expected a refusal for limit={bad}, got {other:?}"),
            }
        }
        assert_eq!(seen.lock().unwrap().len(), 0, "nothing may reach the wire");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn recall_refuses_a_context_limit_out_of_range_but_allows_zero() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for bad in [21usize, 100] {
            let opts = RecallOpts { limit: None, context_limit: Some(bad) };
            assert!(mem.recall_opts("q", "alice", &opts).await.is_err(), "context_limit={bad}");
        }
        // 0 is a real answer ("attach none"), not a missing value — it must pass.
        let opts = RecallOpts { limit: Some(20), context_limit: Some(0) };
        mem.recall_opts("q", "alice", &opts).await.expect("0 and 20 are in range");
        let body = &seen.lock().unwrap()[0];
        assert!(body.contains("\"context_limit\":0"), "got {body}");
        assert!(body.contains("\"limit\":20"), "got {body}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn replayed_never_overwrites_what_the_service_sent() {
        // These bodies are widening — a response may carry fields this client has
        // never seen. If a name ever collides, the service's value is the
        // true one and ours is a guess.
        let (base, _) = mock_server(vec![http(
            "200 OK",
            "Idempotent-Replayed: true\r\n",
            "{\"id\":\"m1\",\"replayed\":false}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let v = mem
            .add_with("x", "alice", serde_json::json!({}), serde_json::json!({}))
            .await
            .expect("stored");
        assert_eq!(v["replayed"], serde_json::json!(false), "the service said false");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_200_is_an_error_not_a_silent_ok() {
        // Returning `{}` here would read as a successful write with no id — the
        // caller would never see that a truncating proxy ate the response.
        let (base, _) = mock_server(vec![http("200 OK", "", "")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.add("x", "alice", serde_json::json!({})).await {
            Err(WosError::Api { status: 200, message }) => {
                assert!(message.contains("empty response body"), "got {message}")
            }
            other => panic!("expected an empty-body error, got {other:?}"),
        }
    }
    #[tokio::test(flavor = "current_thread")]
    async fn add_refuses_a_store_name_as_metadata_before_the_network() {
        let off = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:9");
        for md in [
            serde_json::json!({"userId": "bob"}),
            serde_json::json!({"store_id": "t"}),
            serde_json::json!({"Idempotency-Key": "k"}),
        ] {
            match off.add("x", "alice", md.clone()).await {
                Err(WosError::Api { status: 400, message }) => assert!(message.contains("not a metadata field")),
                other => panic!("{md}: expected a refusal, got {other:?}"),
            }
            match off.add_with("x", "alice", md, serde_json::json!({})).await {
                Err(WosError::Api { status: 400, .. }) => {}
                other => panic!("add_with: expected a refusal, got {other:?}"),
            }
        }
        assert!(check_metadata_keys(&serde_json::json!({"store": "Costco", "user_id_2": "b", "model": "m"})).is_ok());
    }

    #[test]
    fn the_accepted_store_id_format() {
        for ok in ["a", "Alice.Smith", "team-a_1", &"x".repeat(64)] {
            assert!(valid_store_id_format(ok), "{ok:?}");
        }
        for bad in ["", "-a", ".a", "bob.lee@example.com", "na\u{ef}ve", "a b", &"x".repeat(65)] {
            assert!(!valid_store_id_format(bad), "{bad:?}");
        }
    }

    // The normalized form the API derives: lowercased, anything outside [a-z0-9_] as '_'.
    #[test]
    fn store_id_normalization_is_what_the_api_does() {
        assert_eq!(normalize_store_id("Alice.Smith"), "alice_smith");
        assert_eq!(normalize_store_id("alice-smith"), "alice_smith");
        assert_eq!(normalize_store_id("bob.lee.x"), "bob_lee_x");
        assert_eq!(normalize_store_id("bob-lee-x"), "bob_lee_x");
        assert_eq!(normalize_store_id("alice_smith"), "alice_smith");
        assert_eq!(normalize_store_id("na\u{ef}ve"), "na_ve");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn idempotent_replay_is_surfaced() {
        // What someone using an idempotency key most wants to know: did my retry write,
        // or was this replayed?
        let (base, _) = mock_server(vec![
            http("200 OK", "Idempotent-Replayed: true\r\n", "{\"id\":\"m1\",\"status\":\"stored\"}"),
            http("200 OK", "", "{\"id\":\"m2\",\"status\":\"stored\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let a = mem
            .add_idempotent("x", "alice", serde_json::json!({}), "k1")
            .await
            .unwrap();
        assert_eq!(a["replayed"], serde_json::json!(true));
        let b = mem.add("y", "alice", serde_json::json!({})).await.unwrap();
        assert!(b.get("replayed").is_none(), "a fresh write must carry no replay marker");
    }
}

#[cfg(test)]
mod store_id_warning_bound_tests {
    use super::*;

    /// The warning record is process-global and tests run in parallel. The tests that
    /// take this lock read that state directly; without it, entries written by another
    /// test bleed in and the result changes from run to run.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Clears only the store-id set: the filter and metadata sets belong to tests
    /// running at the same time.
    fn reset_store_ids() {
        if let Some(m) = WARNED_STORE_IDS.get() {
            m.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
    }

    /// This warning fires on ids whose normalized form differs or that the API refuses,
    /// and with one store per end user it sees one id per user. Unbounded, it grows for
    /// the life of the process.
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

    /// Eviction is oldest-first, so a collision that first appears late must still
    /// get its one warning.
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

    /// An id that does not fold is never recorded, so it cannot waste the cap. Other
    /// tests write folding ids into the same set at the same time, so this looks only
    /// for its own ids.
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
}

#[cfg(test)]
mod plain_http_host_tests {
    //! The warning reads the URL the way the transport does, so a spelling that
    //! connects to a remote host in cleartext cannot pass for loopback.
    use super::plain_http_to_remote;

    #[test]
    fn userinfo_cannot_impersonate_loopback() {
        for base in [
            "http://127.0.0.1:9@evil.example/x",
            "http://localhost@evil.example",
            "http://evil.example\\@127.0.0.1",
            "http://evil.example?@127.0.0.1",
            "http://evil.example#@127.0.0.1",
        ] {
            assert!(plain_http_to_remote(base), "{base} must warn");
        }
    }

    #[test]
    fn spellings_the_transport_accepts_still_warn() {
        for base in [
            "http://evil.example",
            "HTTP://evil.example",
            " http://evil.example",
            "http:/example.net:8080",
            "http://example.net:8080",
        ] {
            assert!(plain_http_to_remote(base), "{base:?} must warn");
        }
    }

    #[test]
    fn real_loopback_and_https_stay_quiet() {
        for base in [
            "http://127.0.0.1:8080/x",
            "http://localhost:3000",
            "http://LOCALHOST",
            "http://[::1]:8080",
            "http://0.0.0.0:1",
            "https://evil.example",
            "not a url",
        ] {
            assert!(!plain_http_to_remote(base), "{base} must not warn");
        }
    }
}

#[cfg(test)]
mod retry_status_tests {
    use super::status_is_retryable;

    #[test]
    fn a_write_is_never_retried_on_an_ambiguous_status() {
        // The whole reason the set is split: the error can arrive after the write
        // was applied, so a retried POST could apply it twice.
        for s in [408, 502, 503, 504] {
            assert!(!status_is_retryable(s, false), "{s} retried a write");
        }
    }

    #[test]
    fn an_idempotent_read_retries_every_ambiguous_status() {
        for s in [408, 502, 503, 504] {
            assert!(status_is_retryable(s, true), "{s} was not retried on a read");
        }
    }

    #[test]
    fn rate_limiting_retries_whatever_the_method() {
        assert!(status_is_retryable(429, false));
        assert!(status_is_retryable(429, true));
    }

    #[test]
    fn a_plain_failure_is_not_retried() {
        for s in [400, 401, 403, 404, 409, 422, 500, 501] {
            assert!(!status_is_retryable(s, true), "{s} should not retry");
            assert!(!status_is_retryable(s, false), "{s} should not retry");
        }
    }
}

#[cfg(test)]
mod tolerance_tests {
    //! One odd field must not delete the record.
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
        // The unreadable fields default; the record and its readable neighbours survive.
        assert_eq!(got[1].content, "");
        assert_eq!(got[1].id, None);
        assert_eq!(got[1].similarity, 0.0);
        assert!(!got[1].is_superseded);
    }

    #[test]
    fn an_element_that_is_not_an_object_is_still_dropped() {
        // Deliberate: an element that is not an object carries nothing to return.
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
}

#[cfg(test)]
mod add_bulk_tests {
    //! These drive `add_bulk_with` itself and read what reached the socket, so the
    //! reserved-field order is tested on the real method.
    use super::tests::{http, mock_server_recording};
    use super::Client;

    fn sent(seen: &std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> serde_json::Value {
        let raw = seen.lock().unwrap()[0].clone();
        let body = raw.split("\r\n\r\n").nth(1).expect("request had no body");
        serde_json::from_str(body).expect("body was not JSON")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_empty_category_is_sent_as_it_is() {
        // An empty category is sent as it is.
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.add_bulk("blob", "alice", "").await.unwrap();
        assert_eq!(sent(&seen)["category"], "");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_timestamp_reaches_the_body() {
        // Without it every backfilled memory carries the upload time, the store sorts
        // wrong, and an event_from/event_to search misses it.
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.add_bulk_with("blob", "alice", "notes", serde_json::json!({"timestamp": "2024-03-01T10:00:00Z"}))
            .await
            .unwrap();
        let b = sent(&seen);
        assert_eq!(b["timestamp"], "2024-03-01T10:00:00Z");
        assert_eq!(b["category"], "notes");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_backfill_can_carry_a_date_and_a_key_at_once() {
        // A dated backfill that is also safe to re-run.
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.add_bulk_with_idempotent(
            "blob", "alice", "notes",
            serde_json::json!({"timestamp": "2024-03-01T10:00:00Z"}),
            "import:run-7",
        ).await.unwrap();
        let raw = seen.lock().unwrap()[0].clone();
        // hyper lowercases header names on the wire, so compare without case.
        assert!(
            raw.to_ascii_lowercase().contains("idempotency-key: import:run-7"),
            "the key did not ride on the write: {raw}"
        );
        assert_eq!(sent(&seen)["timestamp"], "2024-03-01T10:00:00Z");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn extra_cannot_overwrite_the_reserved_fields() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.add_bulk_with("real", "alice", "notes", serde_json::json!({
            "user_id": "somebody_else", "content": "spoofed", "category": "hijacked"
        }))
        .await
        .unwrap();
        let b = sent(&seen);
        assert_eq!(b["user_id"], "alice", "extra redirected the write to another store");
        assert_eq!(b["content"], "real");
        assert_eq!(b["category"], "notes");
    }
}

#[cfg(test)]
mod error_kind_tests {
    //! A caller's own mistake and "nothing usable came back" must not look the same.
    use super::{Client, ErrorKind, WosError};

    fn kind_of(e: WosError) -> ErrorKind {
        e.kind()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_argument_the_client_refuses_is_a_bad_request() {
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:9");
        for e in [
            mem.search("q", "alice", 50).await.unwrap_err(),
            mem.search("q", "alice", 1).await.unwrap_err(),
            mem.usage(400).await.unwrap_err(),
            mem.get("alice", "  ").await.unwrap_err(),
        ] {
            assert_eq!(kind_of(e), ErrorKind::BadRequest, "a caller mistake must read as one");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_exhausted_deadline_reads_like_a_connection_failure() {
        // No answer arrived, so it reads as a connection failure.
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:9")
            .with_deadline(std::time::Duration::from_millis(1));
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let e = mem.list_stores().await.unwrap_err();
        assert_eq!(kind_of(e), ErrorKind::Connection);
    }
}

#[cfg(test)]
mod widening_tests {
    //! A field the service adds must reach the caller.
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
        // The three it does name still land where they did.
        assert_eq!(got.short_term["count"], 0);
    }

    #[test]
    fn a_reply_with_nothing_extra_leaves_it_empty() {
        let v = serde_json::json!({"short_term": {}, "long_term": {}, "context": {}});
        let got: RecallResponse = serde_json::from_value(v).unwrap();
        assert!(got.extra.is_empty());
    }
}

#[cfg(test)]
mod conflict_retry_tests {
    //! 409 means one of two things: another write to the store was in flight
    //! (`retry_after_ms`, nothing stored), or the store id collides with an existing
    //! one (`conflicts_with`, permanent).
    use super::tests::{http, mock_server};
    use super::{Client, ErrorKind};
    use std::sync::atomic::Ordering;

    const WRITE_LOCK: &str = "{\"type\":\"error\",\"error\":{\"type\":\"conflict_error\",\
        \"message\":\"Another write to store 'alice' is already in flight. Nothing was stored or changed.\",\
        \"request_id\":\"req_lock\",\"retry_after_ms\":100}}";
    const COLLISION: &str = "{\"type\":\"error\",\"error\":{\"type\":\"conflict_error\",\
        \"message\":\"Store id 'team.a' collides with existing store 'team-a'.\",\
        \"request_id\":\"req_col\",\"conflicts_with\":\"team-a\"}}";

    #[tokio::test(flavor = "current_thread")]
    async fn a_write_lock_409_is_retried_even_on_a_write() {
        let (base, hits) = mock_server(vec![
            http("409 Conflict", "", WRITE_LOCK),
            http("200 OK", "", "{\"id\":\"m1\",\"status\":\"stored\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let v = mem.add("x", "alice", serde_json::json!({})).await.expect("retried after the lock");
        assert_eq!(v["id"], "m1");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_write_lock_409_waits_at_least_what_it_asked_for() {
        let lock = WRITE_LOCK.replace("\"retry_after_ms\":100", "\"retry_after_ms\":900");
        let (base, hits) = mock_server(vec![
            http("409 Conflict", "", &lock),
            http("200 OK", "", "{\"id\":\"m1\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let t0 = std::time::Instant::now();
        mem.add("x", "alice", serde_json::json!({})).await.unwrap();
        assert!(t0.elapsed() >= std::time::Duration::from_millis(900), "waited only {:?}", t0.elapsed());
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_write_lock_409_stays_inside_the_retry_count() {
        // Default retries (2): three attempts, then the 409. The 200 is never served.
        let (base, hits) = mock_server(vec![
            http("409 Conflict", "", WRITE_LOCK),
            http("409 Conflict", "", WRITE_LOCK),
            http("409 Conflict", "", WRITE_LOCK),
            http("200 OK", "", "{\"id\":\"m1\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(e.status(), Some(409), "got {e}");
        assert_eq!(e.kind(), ErrorKind::Conflict);
        assert!(format!("{e}").contains("req_lock"), "got {e}");
        assert_eq!(hits.load(Ordering::SeqCst), 3);

        let (base, hits) = mock_server(vec![
            http("409 Conflict", "", WRITE_LOCK),
            http("200 OK", "", "{\"id\":\"m1\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_retries(0);
        let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Conflict);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_store_id_collision_is_never_retried() {
        let (base, hits) = mock_server(vec![
            http("409 Conflict", "", COLLISION),
            http("200 OK", "", "{\"user_id\":\"team.a\",\"status\":\"created\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem.create_store("team.a").await.unwrap_err();
        assert_eq!(e.status(), Some(409));
        assert_eq!(e.kind(), ErrorKind::Conflict);
        assert!(format!("{e}").contains("collides"), "got {e}");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_409_with_both_fields_or_neither_is_not_retried() {
        let both = COLLISION.replace("\"conflicts_with\"", "\"retry_after_ms\":100,\"conflicts_with\"");
        let neither = "{\"error\":{\"type\":\"conflict_error\",\"message\":\"conflict\"}}";
        let plain = "{\"error\":\"conflict\"}";
        for body in [both.as_str(), neither, plain] {
            let (base, hits) = mock_server(vec![
                http("409 Conflict", "", body),
                http("200 OK", "", "{\"id\":\"m1\"}"),
            ]);
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
            let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
            assert_eq!(e.status(), Some(409), "{body}");
            assert_eq!(hits.load(Ordering::SeqCst), 1, "{body} was retried");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_image_retries_a_write_lock_409_too() {
        let (base, hits) = mock_server(vec![
            http("409 Conflict", "", WRITE_LOCK),
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPNG!".to_string(),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let (bytes, ctype) = mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await.unwrap();
        assert_eq!(bytes, b"PNG!");
        assert_eq!(ctype, "image/png");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }
}

#[cfg(test)]
mod retry_wait_tests {
    //! What a retry may wait, and what the caller sees when it cannot.
    use super::tests::{http, mock_server};
    use super::{Client, ErrorKind};
    use std::sync::atomic::Ordering;

    #[tokio::test(flavor = "current_thread")]
    async fn a_read_whose_backoff_cannot_fit_reports_the_503() {
        let (base, hits) = mock_server(vec![
            http(
                "503 Service Unavailable",
                "Retry-After: 5\r\n",
                "{\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"busy\",\"request_id\":\"req_503\"}}",
            ),
            http("200 OK", "", "{\"collections\":[]}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(200));
        let t0 = std::time::Instant::now();
        let e = mem.list_stores().await.unwrap_err();
        assert_eq!(e.status(), Some(503), "got {e}");
        assert_eq!(e.kind(), ErrorKind::Server);
        assert!(format!("{e}").contains("req_503"), "got {e}");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert!(t0.elapsed().as_secs_f64() < 0.15, "spent {:?}", t0.elapsed());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_retry_after_beyond_the_cap_is_not_slept_through() {
        let an_hour = httpdate::fmt_http_date(std::time::SystemTime::now() + std::time::Duration::from_secs(3600));
        for ra in ["3600".to_string(), an_hour] {
            let (base, hits) = mock_server(vec![
                http(
                    "429 Too Many Requests",
                    &format!("Retry-After: {ra}\r\n"),
                    "{\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"free endpoint limit\",\"request_id\":\"req_cap\"}}",
                ),
                http("200 OK", "", "{\"days\":7}"),
            ]);
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
            let t0 = std::time::Instant::now();
            let e = mem.usage(7).await.unwrap_err();
            assert!(e.is_rate_limited(), "Retry-After {ra}: got {e}");
            assert!(format!("{e}").contains("req_cap"), "got {e}");
            assert_eq!(hits.load(Ordering::SeqCst), 1, "Retry-After {ra} was retried");
            assert!(t0.elapsed().as_secs_f64() < 1.0, "slept {:?} for Retry-After {ra}", t0.elapsed());
        }
    }

    const LIMITED: &str = "{\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\
        \"message\":\"slow down\",\"request_id\":\"req_429\"}}";

    /// Answer the first connection with a 429 asking for 1s, then take the second
    /// and answer it only after `hold`.
    fn limited_then_stalls(hold: std::time::Duration) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h = hits.clone();
        std::thread::spawn(move || {
            for i in 0..2 {
                let Ok((mut sock, _)) = listener.accept() else { return };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 2048];
                while let Ok(n) = sock.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                h.fetch_add(1, Ordering::SeqCst);
                if i == 0 {
                    let _ = sock.write_all(http("429 Too Many Requests", "Retry-After: 1\r\n", LIMITED).as_bytes());
                } else {
                    std::thread::sleep(hold);
                    let _ = sock.write_all(http("200 OK", "", "{\"collections\":[]}").as_bytes());
                }
            }
        });
        (format!("http://{addr}"), hits)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_read_whose_last_attempt_runs_out_of_budget_reports_the_429() {
        let (base, hits) = limited_then_stalls(std::time::Duration::from_secs(2));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(1200));
        let t0 = std::time::Instant::now();
        let e = mem.list_stores().await.unwrap_err();
        assert_eq!(e.status(), Some(429), "got {e:?}");
        assert!(e.is_rate_limited());
        assert!(format!("{e}").contains("req_429"), "got {e}");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert!(t0.elapsed().as_secs_f64() < 1.6, "spent {:?}", t0.elapsed());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_retry_that_drops_with_no_time_to_back_off_reports_the_answer_before_it() {
        let (base, hits) = mock_server(vec![
            http("429 Too Many Requests", "Retry-After: 0\r\n", r#"{"error":{"type":"rate_limit_error","message":"slow down"}}"#),
            String::new(),
        ]);
        // The next backoff, about 1s, does not fit what is left of 600ms.
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(600));
        let e = mem.list_stores().await.unwrap_err();
        assert_eq!(e.status(), Some(429), "got {e:?}");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_image_whose_last_attempt_runs_out_of_budget_reports_the_429() {
        let (base, hits) = limited_then_stalls(std::time::Duration::from_secs(2));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(1200));
        let e = mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await.unwrap_err();
        assert_eq!(e.status(), Some(429), "got {e:?}");
        assert!(format!("{e}").contains("req_429"), "got {e}");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_write_that_times_out_at_the_deadline_reports_the_deadline() {
        // The second attempt went out and may have been applied, so the 429 before it
        // (nothing written) is not what the caller sees.
        let (base, hits) = limited_then_stalls(std::time::Duration::from_secs(2));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(1200));
        let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Connection, "got {e:?}");
        assert_eq!(e.status(), Some(0), "got {e:?}");
        assert!(format!("{e}").contains("deadline of 1.2s exhausted"), "got {e}");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    /// Take one connection and answer it only after `hold`. With `head_first` the
    /// headers go out at once and the body is what is held.
    fn stalls(hold: std::time::Duration, head_first: bool, body: &'static [u8]) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let Ok((mut sock, _)) = listener.accept() else { return };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 2048];
            while let Ok(n) = sock.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            if head_first {
                let _ = sock.write_all(head.as_bytes());
                let _ = sock.flush();
            }
            std::thread::sleep(hold);
            if !head_first {
                let _ = sock.write_all(head.as_bytes());
            }
            let _ = sock.write_all(body);
        });
        format!("http://{addr}")
    }

    fn assert_deadline(e: &super::WosError, what: &str) {
        assert_eq!(e.status(), Some(0), "{what}: got {e:?}");
        assert_eq!(e.kind(), ErrorKind::Connection, "{what}: got {e:?}");
        assert!(format!("{e}").contains("deadline of 300ms exhausted"), "{what}: got {e}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_first_attempt_cut_short_by_the_deadline_reports_the_deadline() {
        const OK: &[u8] = b"{\"collections\":[],\"id\":\"m1\"}";
        let hold = std::time::Duration::from_secs(2);
        let deadline = std::time::Duration::from_millis(300);
        for head_first in [false, true] {
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &stalls(hold, head_first, OK)).with_deadline(deadline);
            let e = mem.list_stores().await.unwrap_err();
            assert_deadline(&e, &format!("read, head_first={head_first}"));

            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &stalls(hold, head_first, OK)).with_deadline(deadline);
            let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
            assert_deadline(&e, &format!("write, head_first={head_first}"));

            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &stalls(hold, head_first, b"PNG!")).with_deadline(deadline);
            let e = mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await.unwrap_err();
            assert_deadline(&e, &format!("get_image, head_first={head_first}"));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_attempt_timeout_without_a_deadline_still_says_timed_out() {
        let mem = Client::with_base_url(
            "wos-test-xxxxxxxxxx",
            &stalls(std::time::Duration::from_secs(3), false, b"{}"),
        )
        .with_timeout(1)
        .with_retries(0);
        let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(e.status(), None, "got {e:?}");
        assert!(format!("{e}").contains("timed out"), "got {e}");
    }

    /// Two clients that hit the same conflict at the same moment must not retry in step.
    #[test]
    fn backoff_jitter_spreads_retries() {
        let jitters: std::collections::HashSet<u128> =
            (0..64).map(|_| super::backoff(0).as_millis() - 500).collect();
        assert!(jitters.iter().all(|j| *j < 250), "jitter out of range: {jitters:?}");
        assert!(jitters.len() > 8, "jitter barely varies: {jitters:?}");
        for attempt in 1..8 {
            let d = super::backoff(attempt).as_millis();
            let base = (500u128 << attempt.min(4)).min(8_000);
            assert!((base..base + 250).contains(&d), "attempt {attempt}: {d}ms");
        }
    }
}

#[cfg(test)]
mod error_surface_tests {
    //! What an error shows, and what it keeps.
    use super::tests::{http, mock_server};
    use super::{Client, ErrorKind, WosError};
    use std::sync::atomic::Ordering;

    fn closed_port() -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let a = l.local_addr().unwrap();
        drop(l);
        format!("http://{a}")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_network_error_never_carries_the_url_or_its_query() {
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &closed_port()).with_retries(0);
        let e = mem.list_speakers("alice.customer@example.com").await.unwrap_err();
        assert!(matches!(e, WosError::Network(_)), "got {e:?}");
        for shown in [format!("{e}"), format!("{e:?}")] {
            assert!(!shown.contains("alice"), "the store id leaked: {shown}");
            assert!(!shown.contains("user_id="), "the query leaked: {shown}");
            assert!(!shown.contains("/api/v1"), "the path leaked: {shown}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_network_error_names_its_cause_and_keeps_its_source() {
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &closed_port()).with_retries(0);
        let e = mem.list_stores().await.unwrap_err();
        assert!(format!("{e}").contains("connection refused"), "got {e}");
        assert!(std::error::Error::source(&e).is_some(), "the transport error must be reachable");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_invalid_base_url_is_a_bad_request_and_is_not_retried() {
        for bad in ["not a url", "ftp://example.com", "http://", "https://"] {
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", bad);
            let t0 = std::time::Instant::now();
            let e = mem.list_stores().await.unwrap_err();
            assert_eq!(e.kind(), ErrorKind::BadRequest, "{bad}: got {e:?}");
            assert!(format!("{e}").contains("invalid base_url"), "{bad}: got {e}");
            assert!(t0.elapsed().as_secs_f64() < 0.3, "{bad}: retried for {:?}", t0.elapsed());
            let e = mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await.unwrap_err();
            assert_eq!(e.kind(), ErrorKind::BadRequest, "{bad} get_image: got {e:?}");
            assert!(t0.elapsed().as_secs_f64() < 0.3, "{bad}: get_image retried for {:?}", t0.elapsed());
        }
    }

    #[test]
    fn a_refused_body_is_a_bad_request() {
        for status in [400u16, 413, 422] {
            let e = WosError::Api { status, message: "x".into() };
            assert_eq!(e.kind(), ErrorKind::BadRequest, "{status}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_413_from_the_service_is_a_bad_request() {
        let (base, _) = mock_server(vec![http(
            "413 Payload Too Large",
            "",
            "{\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"Request body too large (max 10MB)\"}}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::BadRequest);
        assert!(format!("{e}").contains("10MB"), "got {e}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_object_valued_message_falls_back_to_the_type() {
        let (base, _) = mock_server(vec![http(
            "404 Not Found",
            "",
            "{\"type\":\"error\",\"error\":{\"type\":\"not_found_error\",\"message\":{\"text\":\"x\"},\"request_id\":\"r-9\"}}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem.stats("alice").await.unwrap_err();
        let shown = format!("{e}");
        assert!(shown.contains("not_found_error") && shown.contains("r-9"), "got {shown}");
        assert!(!shown.contains("{\"text\""), "got {shown}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_image_errors_carry_the_request_id() {
        let (base, _) = mock_server(vec![http(
            "404 Not Found",
            "",
            "{\"type\":\"error\",\"error\":{\"type\":\"not_found_error\",\"message\":\"Not found.\",\"request_id\":\"req_img\"}}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotFound);
        assert!(format!("{e}").contains("req_img"), "got {e}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn control_characters_from_the_server_never_reach_the_message() {
        let hostile = "{\"error\":{\"message\":\"boom\\u001b[31mRED\\r\\nInjected: line\\u0000\\u007f\\t!\"}}";
        let raw = "oops\u{1b}[2J\r\nfake log line";
        let (base, _) = mock_server(vec![
            http("500 Internal Server Error", "", hostile),
            http("500 Internal Server Error", "", raw),
            http("500 Internal Server Error", "", hostile),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let a = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        let b = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        let c = mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await.unwrap_err();
        for e in [a, b, c] {
            let shown = format!("{e}");
            assert!(
                !shown.chars().any(|ch| (ch as u32) < 0x20 || ch == '\u{7f}'),
                "a control character survived: {shown:?}"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_image_reports_why_it_could_not_read_an_error_body() {
        let (base, _) = mock_server(vec![format!(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{{}}",
            70 * 1024 * 1024
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await {
            Err(WosError::Api { status: 500, message }) => {
                assert!(message.contains("too large"), "got {message:?}")
            }
            other => panic!("expected a 500 that says why, got {other:?}"),
        }
    }

    #[test]
    fn debug_hides_credentials_in_the_base_url() {
        for base in [
            "https://user:hunter2pass@proxy.example.com",
            "https://hunter2pass@proxy.example.com/prefix",
            "http://u:hunter2pass@[::1]:8080",
            "http://u:hunter2pass@proxy.example.com:notaport",
        ] {
            let dbg = format!("{:?}", Client::with_base_url("wos-test-xxxxxxxxxx", base));
            assert!(!dbg.contains("hunter2pass"), "{base} leaked into {dbg}");
        }
        let dbg = format!("{:?}", Client::with_base_url("wos-test-xxxxxxxxxx", "https://user:pw@proxy.example.com"));
        assert!(dbg.contains("proxy.example.com"), "the host is still useful: {dbg}");
    }

    #[test]
    fn debug_hides_credentials_the_url_parser_does_not_read_as_such() {
        for (base, shown) in [
            ("https://user:s3/cr3t@api.example.com", "https://***@api.example.com"),
            ("https://user:s3cr3t#x@api.example.com", "https://***@api.example.com"),
            ("https://user:s3?cr3t@api.example.com", "https://***@api.example.com"),
            ("https://user:123/cr3t@api.example.com", "https://***@api.example.com"),
            ("user:s3cr3t@api.example.com", "***@api.example.com"),
            // Read as user "user", password "p", host "ss".
            ("https://user:p@ss/cr3t@api.example.com", "https://***@api.example.com"),
            ("user:cr3t://x@api.example.com", "***@api.example.com"),
        ] {
            let dbg = format!("{:?}", Client::with_base_url("wos-test-xxxxxxxxxx", base));
            assert!(!dbg.contains("cr3t"), "{base} leaked into {dbg}");
            assert!(dbg.contains(&format!("{shown:?}")), "{base}: got {dbg}");
        }
        // Nothing to hide: shown as written.
        for base in ["https://api.example.com", "http://127.0.0.1:9/prefix", "not a url"] {
            let dbg = format!("{:?}", Client::with_base_url("wos-test-xxxxxxxxxx", base));
            assert!(dbg.contains(&format!("{base:?}")), "{base}: got {dbg}");
        }
    }

    #[test]
    fn a_base_url_with_no_host_is_refused_before_sending() {
        for bad in ["http://", "https://", "http:", "https:///"] {
            match Client::with_base_url("wos-test-xxxxxxxxxx", bad).preflight() {
                Err(WosError::Api { status: 400, message }) => {
                    assert!(message.contains("invalid base_url"), "{bad}: got {message}")
                }
                other => panic!("{bad} must be refused before sending, got {other:?}"),
            }
        }
        for good in ["https://api.wontopos.com", "http://127.0.0.1:9", "https://proxy.example.com/wos/"] {
            assert!(Client::with_base_url("wos-test-xxxxxxxxxx", good).preflight().is_ok(), "{good}");
        }
    }

    #[test]
    fn a_base_url_the_parser_would_reread_is_refused_and_its_ends_are_trimmed() {
        for bad in ["https:/\\evil.example", "http://evil.example\\@127.0.0.1", "https://api.won\ttopos.com", "https://a.example/x y", "https://a.example/\u{0}"] {
            match Client::with_base_url("wos-test-xxxxxxxxxx", bad).preflight() {
                Err(WosError::Api { status: 400, message }) => assert!(message.contains("backslash"), "{bad:?}: got {message}"),
                other => panic!("{bad:?} must be refused before sending, got {other:?}"),
            }
        }
        for padded in ["https://api.wontopos.com\n", " https://api.wontopos.com/ ", "\thttps://api.wontopos.com\r\n"] {
            let c = Client::with_base_url("wos-test-xxxxxxxxxx", padded);
            assert!(c.preflight().is_ok(), "{padded:?}");
            assert_eq!(c.base_url, "https://api.wontopos.com", "{padded:?}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_invalid_base_url_error_does_not_show_its_credentials() {
        for bad in ["https://u:hunter2@host:99999", "u:hunter2@host", "https://u:hunter2@", "ftp://u:hunter2@host"] {
            let e = Client::with_base_url("wos-test-xxxxxxxxxx", bad).list_stores().await.unwrap_err();
            assert!(format!("{e}").contains("invalid base_url"), "{bad}: got {e}");
            assert!(!format!("{e} {e:?}").contains("hunter2"), "{bad} leaked into {e:?}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn new_error_messages_use_plain_punctuation() {
        let e = Client::with_base_url("wos-test-xxxxxxxxxx", "not a url").list_stores().await.unwrap_err();
        assert!(format!("{e}").contains("invalid base_url"), "got {e}");
        assert!(!format!("{e}").contains('\u{2014}'), "got {e}");
        // The two errors both page walks return, in full.
        assert_eq!(
            format!("{}", super::page_ceiling()),
            "[200] stopped after 20000 pages \u{2014} the store did not end. This is a truncated answer, not the whole store."
        );
        assert_eq!(
            format!("{}", super::repeated_cursor()),
            "[200] the service handed back a cursor it had already given, so the store did not end. \
             This is a truncated answer, not the whole store."
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_image_checks_the_model_name_before_sending() {
        let (base, hits) = mock_server(vec![]);
        for bad in ["bad model", "", "bad\r\nX-Evil: 1"] {
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_model(bad);
            match mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await {
                Err(WosError::Api { status: 400, message }) => {
                    assert!(message.contains("invalid model name"), "{bad:?}: got {message}")
                }
                other => panic!("{bad:?} should be refused locally, got {other:?}"),
            }
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing may reach the network");
    }

    #[test]
    fn an_empty_message_falls_back_to_the_type_and_keeps_the_request_id() {
        let (m, rid) = super::parse_error(
            "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"\",\"request_id\":\"req_e\"}}",
        );
        assert_eq!(m, "invalid_request_error");
        assert_eq!(rid.as_deref(), Some("req_e"));

        // Neither a usable message nor a type: the raw body, still with its request id.
        let raw = "{\"error\":{\"message\":{\"a\":1},\"request_id\":\"req_o\"}}";
        let (m, rid) = super::parse_error(raw);
        assert_eq!(m, raw);
        assert_eq!(rid.as_deref(), Some("req_o"));

        // A request id that is empty is no request id.
        let (_, rid) = super::parse_error("{\"error\":{\"type\":\"t\",\"message\":\"m\",\"request_id\":\"\"}}");
        assert_eq!(rid, None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_error_with_nothing_to_say_names_its_status() {
        let (base, _) = mock_server(vec![
            http("502 Bad Gateway", "", ""),
            http("400 Bad Request", "", "{\"error\":\"\"}"),
            http("400 Bad Request", "", "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"\",\"request_id\":\"req_e\"}}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_retries(0);
        let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(format!("{e}"), "[502] HTTP 502");
        let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(format!("{e}"), "[400] HTTP 400");
        let e = mem.add("x", "alice", serde_json::json!({})).await.unwrap_err();
        assert_eq!(format!("{e}"), "[400] invalid_request_error (request_id: req_e)");
    }
}

#[cfg(test)]
mod page_size_tests {
    //! Page sizes are checked before sending, with the range in the message.
    use super::tests::{http, mock_server};
    use super::{Client, SearchOpts, WosError};
    use std::sync::atomic::Ordering;

    fn refused(r: Result<(), WosError>, range: &str, what: &str) {
        match r {
            Err(WosError::Api { status: 400, message }) => {
                assert!(message.contains(range), "{what}: got {message}")
            }
            other => panic!("{what} should be refused locally, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn image_speaker_and_revision_pages_are_5_to_20() {
        let (base, hits) = mock_server(vec![]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for bad in [0usize, 4, 21, 100, usize::MAX] {
            refused(mem.list_images("alice", bad, None, None).await.map(|_| ()), "between 5 and 20", "list_images");
            refused(mem.list_all_images("alice", bad).await.map(|_| ()), "between 5 and 20", "list_all_images");
            refused(mem.export_images("alice", bad).await.map(|_| ()), "between 5 and 20", "export_images");
            refused(mem.iter_images("alice", bad).await.map(|_| ()), "between 5 and 20", "iter_images");
            refused(mem.by_speaker("Bob", "alice", bad, None, None).await.map(|_| ()), "between 5 and 20", "by_speaker");
            refused(
                mem.revisions_page("alice", "revised", bad, None, None).await.map(|_| ()),
                "between 5 and 20",
                "revisions_page",
            );
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing may reach the network");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_ends_of_each_range_are_sent() {
        let ok = || http("200 OK", "", "{\"images\":[],\"memories\":[],\"has_more\":false}");
        let (base, hits) = mock_server((0..12).map(|_| ok()).collect());
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for n in [5usize, 20] {
            mem.list_images("alice", n, None, None).await.unwrap();
            mem.list_all_images("alice", n).await.unwrap();
            mem.by_speaker("Bob", "alice", n, None, None).await.unwrap();
            mem.revisions_page("alice", "revised", n, None, None).await.unwrap();
        }
        mem.list_memories("alice", 1, None).await.unwrap();
        mem.list_memories("alice", 500, None).await.unwrap();
        mem.search_opts("q", "alice", 10, &SearchOpts { max_images: Some(0), ..Default::default() }).await.unwrap();
        mem.search_opts("q", "alice", 10, &SearchOpts { max_images: Some(5), ..Default::default() }).await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 12);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn max_images_is_0_to_5() {
        let (base, hits) = mock_server(vec![]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for bad in [6usize, 100] {
            let opts = SearchOpts { max_images: Some(bad), ..Default::default() };
            refused(mem.search_opts("q", "alice", 10, &opts).await.map(|_| ()), "between 0 and 5", "search_opts");
            refused(mem.search_full("q", "alice", 10, &opts).await.map(|_| ()), "between 0 and 5", "search_full");
        }
        for bad in [serde_json::json!(true), serde_json::json!(6), serde_json::json!(-1), serde_json::json!(2.5), serde_json::json!("3")] {
            refused(
                mem.search_with("q", "alice", 10, serde_json::json!({ "max_images": bad })).await.map(|_| ()),
                "between 0 and 5",
                "search_with",
            );
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing may reach the network");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_null_max_images_is_read_as_absent() {
        let ok = || http("200 OK", "", "{\"memories\":[],\"self_memories\":[]}");
        let (base, hits) = mock_server((0..3).map(|_| ok()).collect());
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let none: Option<usize> = None;
        let extra = || serde_json::json!({ "max_images": none });
        mem.search_with("q", "alice", 10, extra()).await.expect("search_with");
        mem.search_self_with("q", "alice", 10, extra()).await.expect("search_self_with");
        mem.search_full_with("q", "alice", 10, &SearchOpts::default(), extra()).await.expect("search_full_with");
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_memories_limit_is_1_to_500() {
        let (base, hits) = mock_server(vec![]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for bad in [0usize, 501, usize::MAX] {
            refused(mem.list_memories("alice", bad, None).await.map(|_| ()), "between 1 and 500", "list_memories");
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing may reach the network");
    }
}

#[cfg(test)]
mod list_shape_tests {
    //! A list keeps only the elements that are objects.
    use super::tests::{http, mock_server};
    use super::Client;
    use std::sync::atomic::Ordering;

    #[tokio::test(flavor = "current_thread")]
    async fn lists_keep_only_objects() {
        let (base, _) = mock_server(vec![
            http("200 OK", "", "{\"models\":[{\"id\":\"tablet-2\"},null,1,\"x\",[]]}"),
            http("200 OK", "", "{\"collections\":[{\"user_id\":\"a\"},null,{\"user_id\":\"b\"},false]}"),
            http("200 OK", "", "{\"turns\":[{\"user\":\"hi\"},null,\"t\"]}"),
            http("200 OK", "", "{\"memories\":[{\"id\":\"1\"},null,7],\"next_cursor\":null}"),
            http("200 OK", "", "{\"images\":[{\"id\":\"a\"},null,\"b\"],\"has_more\":false}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert_eq!(mem.list_models().await.unwrap().len(), 1);
        assert_eq!(mem.list_stores().await.unwrap().len(), 2);
        assert_eq!(mem.history("alice").await.unwrap().len(), 1);
        assert_eq!(mem.list_all_memories("alice").await.unwrap().len(), 1);
        assert_eq!(mem.list_all_images("alice", None).await.unwrap().len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_empty_cursor_ends_the_walk() {
        let (base, hits) = mock_server(vec![
            http("200 OK", "", "{\"memories\":[{\"id\":\"1\"}],\"next_cursor\":\"\"}"),
            http("200 OK", "", "{\"memories\":[{\"id\":\"1\"}],\"next_cursor\":null}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert_eq!(mem.list_all_memories("alice").await.unwrap().len(), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        let (base, hits) = mock_server(vec![
            http("200 OK", "", "{\"images\":[{\"id\":\"a\"}],\"has_more\":true,\"next_before\":\"\"}"),
            http("200 OK", "", "{\"images\":[{\"id\":\"a\"}],\"has_more\":false}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert_eq!(mem.list_all_images("alice", None).await.unwrap().len(), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_repeated_cursor_after_a_page_with_no_objects_ends_the_walk() {
        let (base, _) = mock_server(vec![
            http("200 OK", "", "{\"memories\":[{\"id\":\"1\"}],\"next_cursor\":\"C\"}"),
            http("200 OK", "", "{\"memories\":[null,7],\"next_cursor\":\"C\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert_eq!(mem.list_all_memories("alice").await.unwrap().len(), 1);

        let page1 = "{\"images\":[{\"id\":\"a\"}],\"has_more\":true,\"next_before\":\"T\",\"next_skip_ids\":[\"a\"]}";
        let page2 = "{\"images\":[null,\"b\"],\"has_more\":true,\"next_before\":\"T\",\"next_skip_ids\":[\"a\"]}";
        let (base, _) = mock_server(vec![http("200 OK", "", page1), http("200 OK", "", page2)]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert_eq!(mem.list_all_images("alice", None).await.unwrap().len(), 1);
    }
}

#[cfg(test)]
mod delete_retry_tests {
    //! A DELETE retried after an ambiguous failure may find its target already gone.
    use super::tests::{dropping_server, http, mock_server};
    use super::{Client, ErrorKind};

    const NOTE: &str = "(an earlier attempt may already have deleted it)";
    const GONE: &str = "{\"type\":\"error\",\"error\":{\"type\":\"not_found_error\",\"message\":\"Store not found.\",\"request_id\":\"req_gone\"}}";

    #[tokio::test(flavor = "current_thread")]
    async fn a_404_after_an_ambiguous_retry_says_so() {
        for first in ["502 Bad Gateway", "504 Gateway Timeout", "503 Service Unavailable", "408 Request Timeout"] {
            let (base, _) = mock_server(vec![
                http(first, "Retry-After: 0\r\n", "{\"error\":\"late\"}"),
                http("404 Not Found", "", GONE),
                http(first, "Retry-After: 0\r\n", "{\"error\":\"late\"}"),
                http("404 Not Found", "", GONE),
            ]);
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
            for e in [
                mem.delete_store("tenant_a").await.unwrap_err(),
                mem.remove_speaker("Bob", "tenant_a").await.unwrap_err(),
            ] {
                assert_eq!(e.kind(), ErrorKind::NotFound, "after {first}: {e}");
                let shown = format!("{e}");
                assert!(shown.ends_with(NOTE), "after {first}: {shown}");
                assert!(shown.contains("req_gone"), "after {first}: {shown}");
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_404_after_a_dropped_response_says_so() {
        let (base, _) = dropping_server(1, http("404 Not Found", "", GONE));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem
            .forget_image("tenant_a", "11111111-1111-1111-1111-111111111111", false)
            .await
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotFound);
        assert!(format!("{e}").ends_with(NOTE), "got {e}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_preview_404_after_an_ambiguous_retry_says_nothing_more() {
        // A preview deletes nothing, so no earlier attempt can have deleted it.
        let (base, hits) = mock_server(vec![
            http("503 Service Unavailable", "Retry-After: 0\r\n", "{\"error\":\"late\"}"),
            http("404 Not Found", "", GONE),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem
            .forget_image("tenant_a", "11111111-1111-1111-1111-111111111111", true)
            .await
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotFound, "got {e}");
        assert!(!format!("{e}").contains("earlier attempt"), "got {e}");
        assert!(format!("{e}").contains("req_gone"), "got {e}");
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);

        let (base, _) = mock_server(vec![
            http("503 Service Unavailable", "Retry-After: 0\r\n", "{\"error\":\"late\"}"),
            http("404 Not Found", "", GONE),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let e = mem
            .forget_image("tenant_a", "11111111-1111-1111-1111-111111111111", false)
            .await
            .unwrap_err();
        assert!(format!("{e}").ends_with(NOTE), "got {e}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_404_without_an_ambiguous_attempt_says_nothing_more() {
        let (base, _) = mock_server(vec![
            http("404 Not Found", "", GONE),
            http("429 Too Many Requests", "Retry-After: 0\r\n", "{\"error\":\"slow\"}"),
            http("404 Not Found", "", GONE),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for e in [
            mem.delete_store("tenant_a").await.unwrap_err(),
            mem.delete_store("tenant_a").await.unwrap_err(),
        ] {
            assert_eq!(e.kind(), ErrorKind::NotFound);
            assert!(!format!("{e}").contains("earlier attempt"), "got {e}");
        }
    }
}

#[cfg(test)]
mod metadata_warning_tests {
    //! The service keeps four metadata keys. Any other key is warned about once.
    use super::tests::{http, mock_server};
    use super::{Client, WARNED_METADATA_KEYS};

    fn warned(key: &str) -> bool {
        WARNED_METADATA_KEYS
            .get()
            .map(|m| m.lock().unwrap().contains(key))
            .unwrap_or(false)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn every_add_warns_on_an_unknown_metadata_key() {
        let ok = || http("200 OK", "", "{\"id\":\"m1\"}");
        let (base, _) = mock_server(vec![ok(), ok(), ok(), ok(), ok()]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let md = |k: &str| serde_json::json!({ k: "x", "speaker": "me", "event_date": "2026-03-14",
                                                 "category": "work", "conversation_id": "c1" });
        mem.add("x", "alice", md("speakr_mdw_add")).await.unwrap();
        mem.store("x", "alice", md("speakr_mdw_store")).await.unwrap();
        mem.add_idempotent("x", "alice", md("speakr_mdw_idem"), "k1").await.unwrap();
        mem.add_with("x", "alice", md("speakr_mdw_with"), serde_json::json!({})).await.unwrap();
        mem.add_with_idempotent("x", "alice", md("speakr_mdw_withidem"), serde_json::json!({}), "k2")
            .await
            .unwrap();
        for k in ["speakr_mdw_add", "speakr_mdw_store", "speakr_mdw_idem", "speakr_mdw_with", "speakr_mdw_withidem"] {
            assert!(warned(k), "{k} was not warned about");
        }
        for k in ["speaker", "event_date", "category", "conversation_id"] {
            assert!(!warned(k), "{k} is kept by the service and must not warn");
        }
    }
}

#[cfg(test)]
mod search_full_with_tests {
    //! `search_full_with`: free-form fields and the typed options in one call, with
    //! every field of the answer kept.
    use super::tests::{http, mock_server_recording};
    use super::{Client, SearchOpts, WosError, WARNED_FILTER_KEYS};

    fn body(raw: &str) -> serde_json::Value {
        serde_json::from_str(raw.split("\r\n\r\n").nth(1).expect("no body")).expect("not JSON")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn filters_and_images_travel_together() {
        let payload = "{\"memories\":[{\"id\":\"m1\"}],\"self_memories\":[{\"id\":\"s1\"}],\
                       \"images\":[{\"id\":\"i1\"}],\"verify_used\":1}";
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", payload)]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let r = mem
            .search_full_with(
                "photos from june",
                "alice",
                10,
                &SearchOpts { max_images: Some(3), ..Default::default() },
                serde_json::json!({
                    "filters": {"event_from": "2026-06-01", "event_to": "2026-06-30"},
                    "speaker": "Bob",
                    "max_images": 1,
                    "user_id": "bob",
                    "query": "smuggled",
                    "max_results": 50,
                }),
            )
            .await
            .unwrap();
        assert_eq!(r.memories.len(), 1);
        assert_eq!(r.self_memories.len(), 1);
        assert_eq!(r.images.len(), 1);
        assert_eq!(r.verify_used, Some(1));
        let b = body(&seen.lock().unwrap()[0]);
        assert_eq!(b["filters"]["event_from"], "2026-06-01");
        assert_eq!(b["speaker"], "Bob");
        assert_eq!(b["max_images"], 3, "opts win over extra");
        assert_eq!(b["user_id"], "alice", "reserved fields win over extra");
        assert_eq!(b["query"], "photos from june");
        assert_eq!(b["max_results"], 10);
        assert!(b.get("verify").is_none(), "an unset option is not sent");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unknown_filters_are_warned_about_and_limits_checked() {
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"memories\":[]}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem
            .search_full_with("q", "alice", 50, &SearchOpts::default(), serde_json::json!({}))
            .await
        {
            Err(WosError::Api { status: 400, message }) => assert!(message.contains("between 5 and 20")),
            other => panic!("expected a local refusal, got {other:?}"),
        }
        mem.search_full_with(
            "q",
            "alice",
            10,
            &SearchOpts::default(),
            serde_json::json!({"filters": {"evnt_from_sfw": "2026-01-01"}}),
        )
        .await
        .unwrap();
        let hit = WARNED_FILTER_KEYS
            .get()
            .map(|m| m.lock().unwrap().contains("evnt_from_sfw"))
            .unwrap_or(false);
        assert!(hit, "an unknown filter key must be warned about");
        assert_eq!(seen.lock().unwrap().len(), 1, "only the valid call is sent");
    }
}

#[cfg(test)]
mod error_body_drop_tests {
    //! A non-2xx answer whose body stops mid-stream.
    use super::tests::{http, mock_server};
    use super::{Client, WosError};
    use std::io::{Read, Write};
    use std::sync::atomic::Ordering;

    /// Headers that promise 100 bytes, then 3 of them and a hang-up.
    fn cut(status: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{{\"e"
        )
    }

    const STORES: &str = "{\"collections\":[],\"count\":0}";

    #[tokio::test(flavor = "current_thread")]
    async fn a_read_whose_error_body_drops_reports_its_status() {
        for (status, code) in [("500 Internal Server Error", 500), ("404 Not Found", 404), ("409 Conflict", 409), ("400 Bad Request", 400)] {
            let (base, hits) = mock_server(vec![cut(status), http("200 OK", "", STORES)]);
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
            match mem.list_stores().await {
                Err(WosError::Api { status: s, message }) if s == code => {
                    assert!(message.contains("could not be read"), "{status}: got {message}")
                }
                other => panic!("{status}: got {other:?}"),
            }
            assert_eq!(hits.load(Ordering::SeqCst), 1, "{status}");
        }
        let (base, hits) = mock_server(vec![cut("500 Internal Server Error"), http("200 OK", "", "{\"deleted\":true}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert_eq!(mem.delete_store("tenant_a").await.unwrap_err().status(), Some(500));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_write_whose_error_body_drops_is_not_sent_again() {
        let (base, hits) = mock_server(vec![cut("500 Internal Server Error"), http("200 OK", "", "{\"id\":\"m1\"}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        match mem.add("hello", "alice", serde_json::json!({})).await {
            Err(WosError::Api { status: 500, message }) => {
                assert!(message.contains("could not be read"), "got {message}")
            }
            other => panic!("expected the 500, got {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1, "a write is attempted exactly once");
    }

    /// The first connection gets `first` and is held open for 6s, so a body it cuts
    /// short stalls rather than drops. Every later connection gets `then`.
    fn stalling_server(first: String, then: String) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h = hits.clone();
        std::thread::spawn(move || {
            while let Ok((mut sock, _)) = listener.accept() {
                let n = h.fetch_add(1, Ordering::SeqCst);
                let mut chunk = [0u8; 8192];
                let _ = sock.read(&mut chunk);
                if n > 0 {
                    let _ = sock.write_all(then.as_bytes());
                    continue;
                }
                let _ = sock.write_all(first.as_bytes());
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(6));
                    drop(sock);
                });
            }
        });
        (base, hits)
    }

    /// `cut`, with extra header lines.
    fn cut_with(status: &str, headers: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n{headers}\r\n{{\"e"
        )
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_error_body_that_stalls_is_not_sent_again() {
        let (base, hits) = stalling_server(cut("500 Internal Server Error"), http("200 OK", "", STORES));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_timeout(1);
        match mem.list_stores().await {
            Err(WosError::Api { status: 500, .. }) => {}
            other => panic!("expected the 500, got {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1, "a timeout is not a drop");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_429_whose_body_stalls_is_sent_again_without_waiting_for_the_body() {
        let (base, hits) = stalling_server(
            cut_with("429 Too Many Requests", "Retry-After: 0\r\n"),
            http("200 OK", "", "{\"id\":\"m1\"}"),
        );
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_timeout(4);
        let t0 = std::time::Instant::now();
        let got = mem.add("hello", "alice", serde_json::json!({})).await;
        assert!(got.is_ok(), "got {got:?}");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert!(t0.elapsed() < std::time::Duration::from_secs(3), "waited {:?}", t0.elapsed());
    }

    /// Headers at once, the body 1.5s later: past the brief wait a retried answer gets.
    fn late_body_server(head: &str, body: &str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (head, body) = (head.to_string(), body.to_string());
        std::thread::spawn(move || {
            while let Ok((mut sock, _)) = listener.accept() {
                let (head, body) = (head.clone(), body.clone());
                std::thread::spawn(move || {
                    let mut chunk = [0u8; 8192];
                    let _ = sock.read(&mut chunk);
                    let _ = sock.write_all(
                        format!("{head}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes(),
                    );
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    let _ = sock.write_all(body.as_bytes());
                });
            }
        });
        base
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_429_that_is_not_retried_waits_for_its_body() {
        let body = r#"{"error":{"type":"rate_limit_error","message":"slow down"}}"#;
        for (retry_after, retries) in [("60", 2), ("0", 0)] {
            let head = format!("HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nRetry-After: {retry_after}\r\n");
            let base = late_body_server(&head, body);
            let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_timeout(4).with_retries(retries);
            let e = mem.stats("alice").await.unwrap_err();
            assert_eq!(e.status(), Some(429), "{e:?}");
            assert!(e.to_string().contains("slow down"), "Retry-After {retry_after}: {e}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_image_does_not_wait_for_a_429_body_that_stalls() {
        let (base, hits) = stalling_server(
            cut_with("429 Too Many Requests", "Retry-After: 0\r\n"),
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPNG!".to_string(),
        );
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_timeout(4);
        let t0 = std::time::Instant::now();
        let (bytes, _) = mem.get_image("alice", "11111111-1111-1111-1111-111111111111").await.unwrap();
        assert_eq!(bytes, b"PNG!");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert!(t0.elapsed() < std::time::Duration::from_secs(3), "waited {:?}", t0.elapsed());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_read_503_whose_body_stalls_is_sent_again_inside_the_deadline() {
        let (base, hits) = stalling_server(
            cut_with("503 Service Unavailable", "Retry-After: 0\r\n"),
            http("200 OK", "", STORES),
        );
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_timeout(4)
            .with_deadline(std::time::Duration::from_secs(3));
        let got = mem.list_stores().await;
        assert!(got.is_ok(), "got {got:?}");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_retryable_answer_whose_body_does_not_arrive_says_so() {
        // The 2s wait fits the deadline when the answer arrives and no longer does after
        // the brief wait for its body, so this answer ends the call.
        let (base, hits) = stalling_server(
            cut_with("503 Service Unavailable", "Retry-After: 2\r\n"),
            http("200 OK", "", STORES),
        );
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_timeout(4)
            .with_deadline(std::time::Duration::from_millis(2500));
        match mem.list_stores().await {
            Err(WosError::Api { status: 503, message }) => {
                assert!(message.contains("could not be read: not received within 1s"), "got {message}")
            }
            other => panic!("expected the 503, got {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_429_whose_body_stalls_on_the_last_attempt_reports_the_429() {
        // Nothing follows, so the body is waited for until the attempt ends; the status
        // stands when it never comes.
        let (base, hits) = stalling_server(
            cut_with("429 Too Many Requests", "Retry-After: 0\r\n"),
            http("200 OK", "", STORES),
        );
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_timeout(2).with_retries(0);
        match mem.list_stores().await {
            Err(WosError::Api { status: 429, .. }) => {}
            other => panic!("expected the 429, got {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
