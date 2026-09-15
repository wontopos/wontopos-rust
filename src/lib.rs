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
//! and jitter, honoring `Retry-After` — 429 always; 502/503 and network errors
//! only when a retry can never double-process a write (idempotent calls, or a
//! failure at connect time). Timeouts are never retried: the write may have
//! landed, and re-sending would bill it twice.
//! Requests time out after 30s (connect 10s). Tune with
//! [`Client::with_retries`] (0 disables) and [`Client::with_timeout`].
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

/// The API normalizes a store id: lowercased, and every character outside
/// `[a-z0-9_]` becomes `_`. So `Alice.Smith`, `alice-smith` and `alice_smith` are
/// ALL the same store.
///
/// That is a data-exposure hazard for the most common way this SDK is used — one
/// store per end user. Two accounts whose ids differ only by punctuation or case
/// (`bob.lee@x.com` / `bob-lee@x.com`) silently share every memory, and nothing in
/// the response says so: the note only appears when `create_store` creates one, and
/// an app that reuses an existing store never sees it. Measured live 2026-08-05.
///
/// We cannot refuse the id — the API accepts it, and callers may have written it
/// this way for a year. So we say it once, on stderr, at the moment it happens.
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

fn warn_if_store_id_collapses(id: &str) {
    if id.is_empty() {
        return;
    }
    let normalized = normalize_store_id(id);
    if normalized == id {
        return;
    }
    let seen = WARNED_STORE_IDS.get_or_init(|| std::sync::Mutex::new(std::collections::VecDeque::new()));
    // Losing the warning because the lock is poisoned is worse than repeating it.
    let first = match seen.lock() {
        Ok(mut g) => {
            if g.iter().any(|s| s == id) {
                false
            } else {
                // This needs a ceiling. The shape the warning is aimed at is an email
                // address, and the one-store-per-end-user pattern this SDK recommends
                // accumulates **one entry per user**, never released for the life of the
                // process. Fifty thousand users means fifty thousand entries (measured) —
                // leaking in exactly the situation the warning exists to describe. Evict
                // oldest-first, so a collision that first appears late still gets its
                // warning.
                g.push_back(id.to_string());
                while g.len() > WARNED_STORE_IDS_MAX {
                    g.pop_front();
                }
                true
            }
        }
        Err(_) => true,
    };
    if first {
        eprintln!(
            "wontopos: store id {id:?} is stored as {normalized:?} (lowercased, and anything \
             outside [a-z0-9_] becomes '_'). Ids that differ only by case or punctuation share \
             ONE store and therefore one set of memories — if these ids come from your end \
             users, normalize them yourself first so two people can never collide."
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

/// Same cap, same reason, as the store-id warn set.
const WARNED_FILTER_KEYS_MAX: usize = 1024;
static WARNED_FILTER_KEYS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

fn warn_on_unknown_filters(body: &serde_json::Value) {
    let Some(filters) = body.get("filters").and_then(|f| f.as_object()) else { return };
    let seen = WARNED_FILTER_KEYS.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    for k in filters.keys() {
        if KNOWN_FILTER_KEYS.contains(&k.as_str()) {
            continue;
        }
        let first = match seen.lock() {
            Ok(mut g) => {
                let inserted = g.insert(k.clone());
                // 2.2.27 capped the store-id warn set and left this sibling unbounded.
                // An app forwarding user-supplied filter keys grows it forever, one
                // entry per distinct typo. Losing an entry costs a repeated warning,
                // never correctness.
                while g.len() > WARNED_FILTER_KEYS_MAX {
                    let Some(victim) = g.iter().next().cloned() else { break };
                    g.remove(&victim);
                }
                inserted
            }
            Err(_) => true,
        };
        if first {
            eprintln!(
                "wontopos: unknown search filter {k:?} — the API drops keys it does not know, so \
                 this filter has NO effect and the search is wider than you think. Known keys: {}",
                KNOWN_FILTER_KEYS.join(", ")
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
    if let Some(m) = WARNED_FILTER_KEYS.get() {
        if let Ok(mut g) = m.lock() { g.clear(); }
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
            WosError::Network(e) => write!(f, "network error: {e}"),
            WosError::Api { status, message } => write!(f, "[{status}] {message}"),
        }
    }
}
impl std::error::Error for WosError {}
impl From<reqwest::Error> for WosError {
    fn from(e: reqwest::Error) -> Self {
        // An `error_for_status()` failure carries the HTTP status — surface it
        // as Api (kind() = Auth/NotFound/...), not as a bogus Connection error.
        match e.status() {
            Some(s) => WosError::Api { status: s.as_u16(), message: e.to_string() },
            None => WosError::Network(e),
        }
    }
}

/// Coarse classification of a failure — match on this instead of raw status codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// No response reached us (DNS/TLS/timeout/connection).
    Connection,
    /// 400 — malformed request.
    BadRequest,
    /// 401 — missing or invalid API key.
    Auth,
    /// 402 — no card on file or depleted balance.
    PaymentRequired,
    /// 403 — not allowed.
    PermissionDenied,
    /// 404 — store or resource not found.
    NotFound,
    /// 409 — concurrent write conflict.
    Conflict,
    /// 429 — rate limited.
    RateLimited,
    /// 5xx — server failure.
    ///
    /// `502` / `503` are transient: this client already retries them where a retry
    /// cannot double-process a write. `501` is NOT — it means the engine behind the
    /// selected model does not implement that endpoint at all, so retrying can never
    /// succeed. Pick a model that supports it ([`Client::list_models`]) instead.
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
                400 => ErrorKind::BadRequest,
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

/// Deserialize a scalar the server *should* always send, but tolerate an explicit
/// `null` (or an absent key) by falling back to the type's default. `#[serde(default)]`
/// alone only covers a MISSING key — a present `null` on a non-`Option` field
/// (`"content": null`, `"similarity": null`) is an "invalid type: null" hard error
/// that fails the WHOLE `search`/`list` batch, dropping every good memory alongside
/// the one odd element. The Python (raw dict) and TS (structural cast) SDKs pass such
/// responses through; this keeps the strongly-typed Rust SDK at the same tolerance.
fn null_to_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// One retrieved memory. Known fields are typed; everything else (e.g. `speaker`:
/// `"me"` for the assistant's own words, or a person's name) lands in `extra`.
#[derive(Debug, Deserialize)]
pub struct Memory {
    #[serde(default)]
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
    #[serde(default)]
    pub time_bucket: Option<String>,
    /// True if a later memory has superseded this one.
    #[serde(default, deserialize_with = "null_to_default")]
    pub is_superseded: bool,
    /// Id of the memory that superseded this one, if any.
    #[serde(default)]
    pub superseded_by: Option<String>,
    /// When the memory was stored (RFC3339). Absent when temporal fields are stripped.
    #[serde(default)]
    pub created_at: Option<String>,
    /// When the content actually happened (RFC3339), if known.
    #[serde(default)]
    pub event_date: Option<String>,
    /// WHO said it: `"me"` for the agent's own words, or a registered person's name.
    /// `None` when the memory carries no speaker tag. Every search result carries this
    /// (the docs say so), so it is a named field rather than something to dig for
    /// under `extra`.
    #[serde(default)]
    pub speaker: Option<String>,
    /// The memory's time written in the requested delivery form — `"a couple weeks
    /// ago"` (memoir) or `"2 weeks ago (Jun 09)"` (archive). Present only when the
    /// call asked for a form on a form-capable model. Same story as `speaker`: it
    /// arrived, but only in `extra`.
    #[serde(default)]
    pub time: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// One-call LLM context: short-term turns + long-term matches + surrounding context.
#[derive(Debug, Deserialize)]
pub struct RecallResponse {
    #[serde(default)]
    pub short_term: serde_json::Value,
    #[serde(default)]
    pub long_term: serde_json::Value,
    #[serde(default)]
    pub context: serde_json::Value,
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
    /// Each pass is another engine call that can add up to `limit` more memories, so it
    /// costs more — you are billed for what is delivered. Helps most on questions needing
    /// several distinct memories from far apart in the history; does little on a
    /// single-fact lookup.
    ///
    /// Needs a re-ask-capable model. An older one REFUSES the call (403) rather than
    /// charging for passes that never happened.
    pub verify: Option<u8>,
    /// How many image memories the answer may carry, 0–5. `None` lets the service use
    /// 1; the MCP server defaults its own tool to 0 instead. `Some(0)` asks for
    /// none. Out of range is refused, not clamped — quietly cutting 5 to 1 would leave
    /// you believing you got five.
    ///
    /// Needs an image-capable model, and is refused (403) on one without it rather than
    /// answering with no images.
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
    /// Needs a limit-aware model. An older one recalls a fixed ten whatever you send, so
    /// the API refuses the call (403) rather than answering with a number you did not ask for.
    pub limit: Option<usize>,
    /// How much surrounding context is attached around the best match, 0–20 (default 10).
    /// `Some(0)` attaches none. Same model floor as `limit`.
    pub context_limit: Option<usize>,
}

/// [`Client::search_self`] result: general memories and the assistant's own, kept apart.
#[derive(Debug)]
pub struct SelfSearch {
    /// What others said, and general memories.
    pub memories: Vec<Memory>,
    /// The assistant's own words (stored with speaker "me"); empty on non-self models.
    pub self_memories: Vec<Memory>,
}

/// Everything one search answered with, not just the merged memories.
///
/// [`Client::search`] returns the memories and nothing else, which is the right answer
/// for almost every call. Two options make it the wrong one: `max_images` asks for
/// photos, which arrive in their own field, and `verify` is reported on by
/// `verify_used`. Merging away both means paying for a search you shaped and never
/// seeing what came of it.
#[derive(Debug, Default)]
pub struct SearchFull {
    /// What others said, and general memories.
    pub memories: Vec<Memory>,
    /// The assistant's own words (speaker "me"); empty on a model that does not keep
    /// them apart.
    pub self_memories: Vec<Memory>,
    /// Image memories, when `max_images` asked for any; empty otherwise.
    pub images: Vec<Memory>,
    /// Re-ask passes actually performed. `None` unless `verify` was sent; lower than
    /// requested means the store had nothing further to add.
    pub verify_used: Option<u8>,
    /// The response as it arrived, for anything this struct does not name yet.
    pub raw: serde_json::Value,
}

/// The engine every call uses unless the caller names another.
///
/// Tablet 2 costs the same per token as Tablet 1 and is the one that serves images,
/// re-ask passes (`verify`), and `self_memories`, so a caller who names nothing gets
/// the engine that can answer the most. All models read the same memory, so switching
/// is a header, not a migration. Pin an older one explicitly with
/// `Client::new(key).with_model("tablet-1")`.
///
/// This paragraph used to sit above `SearchFull` with nothing between them, so rustdoc
/// read it as that struct's documentation and docs.rs published `SearchFull` under the
/// summary line "The engine every call uses…" — while the constant it describes had no
/// documentation at all.
const DEFAULT_MODEL: &str = "tablet-2";

/// A page walk has to stop somewhere, and 1,000,000 pages was not a stop: at 100 per
/// page that is 100 million memories, so a server minting a fresh cursor every time
/// would spend hours and a million billed requests before it fired. 20,000 pages is
/// two million memories — past any real store, reached in minutes.
///
/// Falling out of the loop returned a truncated Vec that looks exactly like a complete
/// one, so hitting this is an error rather than a quiet end.
const MAX_PAGES: u32 = 20_000;

/// The count shared by `search` and `recall`. 5 to 20 inclusive.
pub const SEARCH_LIMIT_MIN: usize = 5;
/// The count shared by `search` and `recall`. 5 to 20 inclusive.
pub const SEARCH_LIMIT_MAX: usize = 20;
/// `recall`'s surrounding-context count. 0 to 20 inclusive; 0 attaches none.
pub const CONTEXT_LIMIT_MIN: usize = 0;
/// `recall`'s surrounding-context count. 0 to 20 inclusive; 0 attaches none.
pub const CONTEXT_LIMIT_MAX: usize = 20;

/// Refuse a count outside the range rather than quietly adjusting it. `search` and
/// `recall` share it.
///
/// The service has refused anything else from the start, because asking for 20 and
/// silently getting 10 reads as "that is all there is". Search had no contract at
/// all: the three clients sent
/// whatever they were given, the MCP server allowed 1 to 60, and the service quietly
/// capped at 50 with no floor. Four surfaces, four answers, and the caller could not
/// tell which one they got.
///
/// Refusing rather than clamping is the same decision as 2.2.35's, where a `limit`
/// of 0 here had been rewritten to 10 and the caller was handed ten memories they
/// had not asked for, and the bill for them. A zero is still not silently changed —
/// it is now refused, which is the part that was missing.
/// `recall`'s `context_limit`, 0 to 20. Same reason as the count: [`RecallOpts`]
/// promises out-of-range is refused, and a promise the client does not keep is worse
/// than no promise — the caller reads the doc, sends 50, and the failure arrives from
/// the service with no hint the SDK knew all along.
///
/// `Some(0)` is a real answer ("attach none"), not a missing value, so it passes.
/// A backoff that never sleeps past the budget — sleeping through the deadline
/// spends the caller's whole allowance on waiting.
///
/// Errors when the wait does not fit. Clamping it to what is left and retrying anyway
/// is the same answer as having no deadline: the call still spends the whole
/// allowance, and the attempt it buys has nothing left to finish in. `attempt_budget`
/// refuses that attempt one line later, so all the clamped sleep bought was the delay
/// before saying so.
fn sleep_within(
    delay: std::time::Duration,
    deadline_at: Option<std::time::Instant>,
    deadline: Option<std::time::Duration>,
) -> Result<std::time::Duration, WosError> {
    let Some(at) = deadline_at else { return Ok(delay) };
    if delay > at.saturating_duration_since(std::time::Instant::now()) {
        return Err(WosError::Api {
            status: 0,
            message: format!("deadline of {:?} exhausted", deadline.unwrap_or_default()),
        });
    }
    Ok(delay)
}

fn check_context_limit(n: usize) -> Result<(), WosError> {
    // A range, not two comparisons: `n < CONTEXT_LIMIT_MIN` can never be true for a
    // usize, and a guard half of which is dead is a guard nobody can read.
    if !(CONTEXT_LIMIT_MIN..=CONTEXT_LIMIT_MAX).contains(&n) {
        return Err(WosError::Api {
            status: 0,
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
            status: 0,
            message: format!(
                "limit must be between {SEARCH_LIMIT_MIN} and {SEARCH_LIMIT_MAX}, got {limit}. \
                 Out of range is refused rather than adjusted, so a short answer always \
                 means the store was short."
            ),
        });
    }
    Ok(())
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

/// The same ceiling, for a body that is not text.
///
/// `get_image` called `resp.bytes()`, which buffers whatever arrives. The cap exists for
/// a hostile or broken `base_url`, and an image endpoint is exactly where that shows up —
/// the JSON paths were guarded while the one path that returns megabytes was not. One
/// implementation, two callers, so the two cannot drift apart.
async fn read_capped_bytes(resp: reqwest::Response) -> Result<Vec<u8>, WosError> {
    if let Some(cl) = resp.content_length() {
        if cl as usize > MAX_RESPONSE_BYTES {
            return Err(WosError::Api {
                status: 0,
                message: format!("response too large ({cl} bytes) — refusing to buffer it"),
            });
        }
    }
    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(WosError::Network)?;
        if buf.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(WosError::Api {
                status: 0,
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
    // Scheme compare is case-insensitive: `HTTP://` connects in plaintext too.
    let lower = base_url.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("http://") else { return };
    let authority = rest.split('/').next().unwrap_or("");
    // Strip userinfo FIRST. `http://127.0.0.1:9@evil.example` has authority
    // "127.0.0.1:9@evil.example"; splitting on ':' first yields "127.0.0.1", so this
    // check called it loopback and said nothing while the key travelled in cleartext to
    // evil.example. The host is what follows the LAST '@'. (Python's urlsplit already
    // did this correctly; TypeScript and Rust were the copies that did not.)
    let hostport = authority.rsplit('@').next().unwrap_or("");
    let host = if let Some(v6) = hostport.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    if !matches!(host.to_ascii_lowercase().as_str(), "localhost" | "127.0.0.1" | "::1" | "0.0.0.0") {
        eprintln!("wontopos: base_url uses plain HTTP on a non-local host, so the API key travels unencrypted. Use https://.");
    }
}

/// An instant far enough out that nothing waits for it — what a deadline past the
/// clock's range means in practice.
fn far_future() -> std::time::Instant {
    std::time::Instant::now() + std::time::Duration::from_secs(60 * 60 * 24 * 365)
}

/// Seconds to sleep before retry `attempt` (0-based). Honors `Retry-After`.
fn backoff(attempt: u32, retry_after: Option<&str>) -> std::time::Duration {
    if let Some(ra) = retry_after {
        let ra = ra.trim();
        if let Ok(secs) = ra.parse::<f64>() {
            if secs >= 0.0 {
                return std::time::Duration::from_millis((secs.min(30.0) * 1000.0) as u64);
            }
        }
        // HTTP-date form (RFC 9110), e.g. "Wed, 21 Oct 2015 07:28:00 GMT".
        if let Ok(when) = httpdate::parse_http_date(ra) {
            if let Ok(delta) = when.duration_since(std::time::SystemTime::now()) {
                return std::time::Duration::from_millis((delta.as_secs_f64().min(30.0) * 1000.0) as u64);
            }
            // A past date → retry immediately.
            return std::time::Duration::from_millis(0);
        }
    }
    let base = 500u64.saturating_mul(1 << attempt.min(4)).min(8_000);
    // Cheap jitter without a rand dependency: sub-second clock noise.
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.subsec_nanos() % 250) as u64)
        .unwrap_or(0);
    std::time::Duration::from_millis(base + jitter)
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
/// alphabet has no whitespace in it. Python and TypeScript have stripped these since
/// 2.2.31; this client did not, which is the whole reason the three were not
/// interchangeable on the same input.
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
/// The byte ceiling is deliberately NOT checked. That is a server setting (`/health`
/// reports `memory.images.max_bytes`), so a number baked into the SDK would drift the
/// first time the service is reconfigured and would refuse an image it would have taken.
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

/// Server may return either the envelope
/// `{"type":"error","error":{"type":...,"message":...,"request_id":...}}`
/// or simple `{"error":"reason"}`. Falls back to the raw text.
fn parse_error(text: &str) -> (String, Option<String>) {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(err) = v.get("error") {
            if let Some(s) = err.as_str() {
                return (cap_msg(s.to_string()), None);
            }
            if let Some(obj) = err.as_object() {
                let msg = obj
                    .get("message")
                    .and_then(|m| m.as_str())
                    .or_else(|| obj.get("type").and_then(|t| t.as_str()))
                    .map(String::from);
                let rid = obj.get("request_id").and_then(|r| r.as_str()).map(String::from);
                if let Some(m) = msg {
                    return (cap_msg(m), rid);
                }
            }
        }
    }
    (cap_msg(text.to_string()), None)
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
    /// A TOTAL budget for one call, across every attempt. `timeout_secs` bounds one
    /// attempt: at the defaults a call can hold for 30s + backoff + 30s + backoff +
    /// 30s, over a minute, and a handler awaiting it had no way to say how long it
    /// actually had. `None` means no overall budget.
    deadline: Option<std::time::Duration>,
    retries: u32,
    http: HttpClient,
    /// Quota from the most recent response's `X-RateLimit-*` headers. Interior
    /// mutability so `&self` request methods can refresh it.
    rl: std::sync::Mutex<Option<RateLimit>>,
}

/// Back-compat alias.
pub type WME = Client;

/// Every memory a search returned, from both fields, as one `Vec`.
///
/// Some models answer with the assistant's own words in `self_memories`, not repeated
/// in `memories`. `search` read only `memories`, so an assistant turn stored with
/// `add_turn` was missing from its results on those models while the same query
/// returned it on others — upgrading made search return LESS, and what went missing
/// had already been retrieved and paid for.
///
/// Both fields, de-duplicated by id, each memory keeping its `speaker` so the caller
/// can still tell who said what. Callers who want them kept apart use
/// [`Client::search_self`], which exists for exactly that.
fn merge_results(v: &serde_json::Value) -> Vec<Memory> {
    let mut out = memories_from(v.get("memories"));
    let mine = memories_from(v.get("self_memories"));
    if mine.is_empty() {
        return out;
    }
    let seen: std::collections::HashSet<String> =
        out.iter().filter_map(|m| m.id.clone()).collect();
    out.extend(mine.into_iter().filter(|m| match &m.id {
        Some(id) => !seen.contains(id),
        None => true,
    }));
    out
}

/// Parse a memory array element-wise, KEEPING every valid record.
///
/// A missing field or a `null` scalar inside one memory is already tolerated (see
/// `null_to_default`), but a malformed ELEMENT — `null`, a number, a string, from a
/// hostile server, a broken proxy, or an unfamiliar host — would fail the whole call
/// through `from_value::<Vec<Memory>>`, letting one bad element destroy every good
/// memory in the batch. Python and TypeScript skip the bad element and return the
/// rest; this matches them. A missing value or non-array yields an empty vec.
///
/// This paragraph used to sit above `merge_results` with nothing between them, so both
/// docs were stacked on that one function and this one had none.
fn memories_from(v: Option<&serde_json::Value>) -> Vec<Memory> {
    match v.and_then(|x| x.as_array()) {
        Some(arr) => arr
            .iter()
            .filter_map(|el| serde_json::from_value::<Memory>(el.clone()).ok())
            .collect(),
        None => Vec::new(),
    }
}

// Never show the key — debug output ends up in logs.
impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.base_url)
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
            status: 0,
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
            base_url: base_url.trim_end_matches('/').to_string(),
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
            // A derived client hasn't made a call yet — start with no snapshot, same
            // as a fresh Python/TS client (consistency across the three SDKs).
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
        // `Instant + Duration` PANICS on overflow, so `with_deadline(Duration::MAX)` —
        // a natural spelling of "no budget", since ZERO already means that — built fine
        // and then aborted the caller's task on every call, from inside the SDK, where
        // every other failure is a Result. Saturate: a budget past the clock's range is
        // a budget that never runs out.
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
            return Err(WosError::Api {
                status: 0,
                message: format!(
                    "deadline of {:?} exhausted",
                    self.deadline.unwrap_or_default()
                ),
            });
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

    /// Return a client that retries transient failures (429/502/503 and connect
    /// errors) `retries` times before giving up. 0 disables retries (default 2).
    pub fn with_retries(&self, retries: u32) -> Self {
        let mut c = self.clone_with(None, None);
        c.retries = retries;
        c
    }

    /// Resolve a call's store: the explicit `Some(user_id)`, else the client default.
    fn uid(&self, user_id: Option<&str>) -> Result<String, WosError> {
        // An omitted id uses the client default — the documented shortcut. A PASSED but
        // blank id is a bug at the call site: the caller computed a tenant id and got
        // nothing back.
        //
        // Falling back is not a safe default here, it is a silent redirect. 2.2.27 sent
        // the blank string on and the server mapped it to the built-in `default` store;
        // 2.2.28 trimmed it and so redirected the call to whatever store this client is
        // bound to, which inside one B2B workspace means one end-user's memories land in
        // another end-user's store and stay readable there. The warning it printed goes
        // to stderr, which a server process usually discards.
        //
        // Every method here already returns Result, so reject it the way the TypeScript
        // and Python clients do — the three surfaces are advertised as identical, and a
        // destination that differs by language is the worst kind of difference.
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
        // The guard above only sees a PASSED id, so `with_user("")` walked around it:
        // the blank landed in `default_user` and every later call resolved to it. The
        // builder cannot return an error (it returns Self), so the check belongs here,
        // on the RESOLVED id — which is the value that decides the destination anyway.
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

    /// Available models: `[{ "id", "name", "available", "memory" }, ...]`.
    /// `memory` is `"shared"` or `"isolated"`. Needs no API key.
    pub async fn list_models(&self) -> Result<Vec<serde_json::Value>, WosError> {
        let v = self.request(reqwest::Method::GET, "/api/v1/models", None, None).await?;
        Ok(v.get("models").and_then(|m| m.as_array()).cloned().unwrap_or_default())
    }

    /// The engrams (and delivery forms) the selected model can actually run.
    ///
    /// Ask rather than hard-code: a name copied from the docs freezes a caller to the
    /// catalogue as it was that day, and anything added later stays invisible. The
    /// service is the authority, and the answer
    /// depends on the model (delivery forms need Scroll 1.2+), so use
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
    pub async fn add(&self, content: &str, user_id: impl Into<Option<&str>>, metadata: serde_json::Value) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/memory/store", serde_json::json!({"user_id": user_id, "content": content, "metadata": metadata})).await
    }

    /// Alias of `add` — store one memory. Same surface as the Python SDK's `store`.
    pub async fn store(&self, content: &str, user_id: impl Into<Option<&str>>, metadata: serde_json::Value) -> Result<serde_json::Value, WosError> {
        self.add(content, user_id, metadata).await
    }

    /// Like [`Client::add`], but merges extra body fields — e.g.
    /// `json!({"image": {"data": "<base64>"}})` to store an image with the memory.
    /// Reserved fields (user_id, content, metadata) are set last so they win.
    ///
    /// **This is where images go, and there is no `add_image`.** Python and TypeScript
    /// take the image as one more option on the ordinary store call; a method of its
    /// own here would have been a third shape for one request, and a new method for
    /// every option that came after it. `recall`/`recall_with`, `search`/`search_with`
    /// and `engram`/`engram_with` already work this way — `add` was the one that had
    /// no way to carry an option at all.
    ///
    /// `content` may be empty: then the image is the memory and is searchable on its
    /// own. Inside `image`, `reference` is where YOUR copy of the original lives (stored
    /// as a string, never fetched by us), and `taken_at` (RFC3339, usually from EXIF)
    /// fills `event_date` when that is empty, so the memory sorts by when the image
    /// was TAKEN rather than by when it was uploaded. `image.data` is normalised on the
    /// way through — see [`normalize_image_b64`].
    ///
    /// What the service keeps is NOT your original. Over 1568px on the long edge the
    /// picture is downscaled to 1568 on the way in, and downscaling means re-encoding:
    /// lossless formats are written as WebP, so a PNG comes back from [`Client::get_image`]
    /// as `image/webp`; JPEG stays JPEG. Under 1568px the bytes are untouched. This is a
    /// memory engine, not a photo host — keep the full-resolution file yourself and put
    /// its URL in `reference`.
    pub async fn add_with(&self, content: &str, user_id: impl Into<Option<&str>>, metadata: serde_json::Value, extra: serde_json::Value) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
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
        self.post("/api/v1/memory/store", serde_json::Value::Object(obj)).await
    }

    /// Store a conversation turn (user + assistant). Payload first, user_id last — same shape as `add`/`search`.
    pub async fn add_turn(&self, user_msg: &str, assistant_msg: &str, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/memory/store-turn", serde_json::json!({"user_id": user_id, "user_msg": user_msg, "assistant_msg": assistant_msg})).await
    }

    /// Bulk-ingest a large blob of text in one call.
    pub async fn add_bulk(&self, content: &str, user_id: impl Into<Option<&str>>, category: &str) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let category = if category.is_empty() { "general" } else { category };
        self.post("/api/v1/memory/bulk-store", serde_json::json!({"user_id": user_id, "content": content, "category": category})).await
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
    // was re-run, a queue that redelivers. This client retries a write on exactly one
    // status: 429, which the service answers before it processes anything, so nothing was
    // stored. It never retries a write on 502 / 503 or a dropped body, where the request
    // may already have been stored and billed — without a key it cannot know whether that
    // first attempt landed.
    //
    // The key must be UNIQUE PER LOGICAL WRITE — derive it from the thing being stored
    // (`format!("import:{}", row.id)`), never a constant, or the second write replays the
    // first and is silently lost. That is also why these are separate methods rather than
    // a `with_idempotency_key()` clone: a clone invites reuse across different writes,
    // which is exactly the mistake that loses data.
    //
    // The window lives in memory on the API, so a deploy or restart clears it early. It is
    // a guard against a retry storm, not a durable ledger.
    // Format: 1-128 chars of `[A-Za-z0-9._:-]`, rejected locally before the request.

    /// [`Client::add`] with an idempotency key.
    pub async fn add_idempotent(&self, content: &str, user_id: impl Into<Option<&str>>, metadata: serde_json::Value, idempotency_key: &str) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post_idem("/api/v1/memory/store", serde_json::json!({"user_id": user_id, "content": content, "metadata": metadata}), Some(idempotency_key)).await
    }

    /// [`Client::add_turn`] with an idempotency key.
    pub async fn add_turn_idempotent(&self, user_msg: &str, assistant_msg: &str, user_id: impl Into<Option<&str>>, idempotency_key: &str) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post_idem("/api/v1/memory/store-turn", serde_json::json!({"user_id": user_id, "user_msg": user_msg, "assistant_msg": assistant_msg}), Some(idempotency_key)).await
    }

    /// [`Client::add_bulk`] with an idempotency key. The call most worth one: a backfill
    /// that dies halfway and is re-run would otherwise re-ingest and re-bill the whole blob.
    pub async fn add_bulk_idempotent(&self, content: &str, user_id: impl Into<Option<&str>>, category: &str, idempotency_key: &str) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let category = if category.is_empty() { "general" } else { category };
        self.post_idem("/api/v1/memory/bulk-store", serde_json::json!({"user_id": user_id, "content": content, "category": category}), Some(idempotency_key)).await
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
    /// 50 and silently receiving 20 reads as "that is all there is". Before 2.2.35 the
    /// count was sent on unchecked, and a 0 was rewritten to 10 here in the client —
    /// so a budget that computed zero was answered with ten memories, and billed.
    ///
    /// `limit` bounds `memories`, not the returned `Vec`. On a model that keeps the
    /// assistant's own words separate (Scroll 1.2+) those come back as well, so the
    /// `Vec` can hold
    /// more than `limit`. They were retrieved and billed either way; dropping them
    /// would only hide what you already paid for. Size a prompt window on what you get
    /// back, not on `limit`. [`Client::search_self`] hands the two back apart.
    pub async fn search(&self, query: &str, user_id: impl Into<Option<&str>>, limit: usize) -> Result<Vec<Memory>, WosError> {
        let user_id = self.uid(user_id.into())?;
        // A zero used to be rewritten to ten here, and the caller who computed it —
        // a prompt budget that ran out — was handed ten memories and the bill for
        // them. 2.2.35 stopped rewriting it. It is now refused instead, along with
        // everything else outside 5..=20, which is the contract `recall` has always
        // had and search never did.
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
    /// Accepted filter keys: `categories`, `event_from`, `event_to`, `time_from`,
    /// `time_to`, `min_importance`. Filtering behaves identically in every language.
    /// Unlisted keys are dropped by the API
    /// rather than rejected — a typo silently widens the search, so spell them exactly.
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
        // Built once. This used to clone the whole map, warn on the copy, and then
        // rebuild from the original — a full deep copy of every request body, thrown
        // away on the next line. `warn_on_unknown_filters` only needs a reference.
        let body = serde_json::Value::Object(obj);
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
    /// field, and comes back 200 having changed nothing. The caller paid for a search and
    /// believes they turned on re-asking. These fields exist so the compiler reads them.
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
    ///                         &SearchOpts { max_images: Some(3), verify: Some(2) }).await?;
    /// r.images.len();   // the photos, which `search` drops
    /// r.verify_used;    // re-ask passes that actually ran (billed per pass)
    /// # Ok(()) }
    /// ```
    pub async fn search_full(
        &self,
        query: &str,
        user_id: impl Into<Option<&str>>,
        limit: usize,
        opts: &SearchOpts,
    ) -> Result<SearchFull, WosError> {
        let user_id = self.uid(user_id.into())?;
        let mut obj = serde_json::Map::new();
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
        let v = self.post("/api/v1/memory/search", serde_json::Value::Object(obj)).await?;
        Ok(SearchFull {
            memories: memories_from(v.get("memories")),
            self_memories: memories_from(v.get("self_memories")),
            images: memories_from(v.get("images")),
            verify_used: v.get("verify_used").and_then(|n| n.as_u64()).map(|n| u8::try_from(n).unwrap_or(u8::MAX)),
            raw: v,
        })
    }

    /// Search a self-memory model (Scroll 1.2+): both fields from ONE call.
    ///
    /// Returns `{ memories, self_memories }` — `memories` is what others said and
    /// general memories, `self_memories` is the assistant's OWN words (stored with
    /// speaker "me"), kept apart so whoever reads them never confuses who said what.
    /// On a model that does not keep them apart, `self_memories` is empty.
    // This doc used to sit above `search_full` with nothing between them, so rustdoc
    // put both blocks on that function and published this one with no documentation
    // at all. `//`, not `///` — the note is for us, and a `///` line here would render
    // it on docs.rs.
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
        // ★ Checked here, where the body is built. The doc on `RecallOpts` promised
        // "out of range is refused, not clamped" and nothing enforced it in any of the
        // three SDKs, so `limit: 500` travelled to the engine and died there — a
        // service error for a mistake we could see before opening a socket.
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
    /// delivery form (Scroll 1.2+). Reserved fields (user_id, query) are set last so they win.
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
    /// `json!({"form": "memoir", "tz": 9})` or `json!({"speaker": "Bob"})`. `search` had
    /// `search_with` for this; the self variant did not — this closes that gap.
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
        // Built once — see `search_with`: cloning the map only to warn on the copy
        // deep-copies every request body and throws it away a line later.
        let body = serde_json::Value::Object(obj);
        warn_on_unknown_filters(&body);
        let v = self.post("/api/v1/memory/search", body).await?;
        // Element-wise per field: corrupt elements skipped, good memories survive.
        Ok(SelfSearch {
            memories: memories_from(v.get("memories")),
            self_memories: memories_from(v.get("self_memories")),
        })
    }

    /// Recent conversation turns (short-term memory).
    pub async fn history(&self, user_id: impl Into<Option<&str>>) -> Result<Vec<serde_json::Value>, WosError> {
        let user_id = self.uid(user_id.into())?;
        let v = self.post("/api/v1/memory/history", serde_json::json!({"user_id": user_id})).await?;
        Ok(v.get("turns").and_then(|t| t.as_array()).cloned().unwrap_or_default())
    }

    /// Memory counts for a store.
    pub async fn stats(&self, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/memory/stats", serde_json::json!({"user_id": user_id})).await
    }

    /// Fetch ONE memory by id — the text you stored, and its metadata.
    /// The id is what `add`/`store` or `list_memories` returned. Same visibility
    /// as `list_memories`: an id from another store, an internal record id,
    /// or an invalidated memory is a 404 (`ErrorKind::NotFound`).
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
                // Validated by trimming, so send the trimmed form. Python strips before
                // sending; an id pasted from a log with a stray space came back in one
                // client and 404'd in the others, on a surface advertised as identical.
                serde_json::json!({"user_id": user_id, "memory_id": memory_id.trim()}),
            )
            .await?;
        Ok(v.get("memory").cloned().unwrap_or_else(|| serde_json::json!({})))
    }

    /// List a store's stored memories — the text you stored, plus its metadata.
    /// Paginated: pass the returned `next_cursor` back as `cursor` for the
    /// next page; a null cursor means the last page. Use it to browse or export a
    /// store. `limit` and `cursor` accept `None` (defaults: 100, first page).
    /// Returns `{ "memories": [...], "count", "next_cursor" }`.
    pub async fn list_memories(
        &self,
        user_id: impl Into<Option<&str>>,
        limit: impl Into<Option<usize>>,
        cursor: impl Into<Option<&str>>,
    ) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let limit = limit.into().unwrap_or(100);
        let mut body = serde_json::json!({ "user_id": user_id, "limit": limit });
        if let Some(c) = cursor.into() {
            body["cursor"] = serde_json::json!(c);
        }
        self.post("/api/v1/memory/list", body).await
    }

    // ----- images (Tablet 2 and newer) -----
    //
    // Storing one is not here: it is [`Client::add_with`], because an image is an
    // option on the ordinary store call in the other two SDKs and inventing a second
    // entry point for it here is what made the three stop matching.

    /// Fetch the bytes of an image memory → `(bytes, content_type)`.
    ///
    /// This is the picture the SERVICE holds, not necessarily your upload — in size or
    /// in format. An image whose long edge was over 1568px was downscaled to 1568 on the
    /// way in and re-encoded (lossless formats as WebP, so a PNG comes back as
    /// `image/webp`; JPEG stays JPEG), and that smaller picture is what is stored and
    /// comes back here. Nothing on our side ever uses more than 1568, so the extra
    /// pixels would be bytes nobody reads. Keep your own copy if you need the
    /// full-resolution file.
    ///
    /// The type is sniffed from the BYTES, not from whatever the upload was named, so
    /// take the file extension from the returned type rather than from what you sent.
    /// When the format changed, the response also carries an
    /// `x-wos-image-converted-from` header naming what you uploaded.
    /// Answers `NotFound` when the memory has no image, or when this service keeps no
    /// image bytes at all — it says "no" rather than handing back something
    /// empty, so "a memory with no image" never looks like "an image we lost".
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
    /// Except when there is no text: an image stored without a caption *is* the memory, so
    /// deleting the image deletes it. Pass `preview = true` to find out first — it
    /// reports `memory_kept` and changes nothing.
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
        self.request(reqwest::Method::DELETE, "/api/v1/memory/image", Some(&body), None).await
    }

    /// One page of image memories, newest first, plus the store's TOTAL `count`.
    ///
    /// `count` is the total, not the size of the page. Paging is by cursor: hand
    /// `next_before` and `next_skip_ids` back as `before` / `skip_ids`. Both are needed
    /// because several images can share a timestamp, and a timestamp alone would either
    /// repeat them or skip them.
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
    /// TypeScript and Python spell this `iterImages` / `iter_images` and hand pages back
    /// lazily. Rust has no async generator in the stable language, and this crate already
    /// answers the same question for text with [`Client::list_all_memories`], so it keeps
    /// that shape: collect and return. Reach for `list_images` when a store is big enough
    /// that holding every image row at once matters.
    ///
    /// This walks ROWS, not pixels — the bytes come from [`Client::get_image`] one at a
    /// time. A thousand images here is a thousand small JSON records, not a thousand JPEGs.
    pub async fn list_all_images(
        &self,
        user_id: impl Into<Option<&str>>,
        page_size: impl Into<Option<usize>>,
    ) -> Result<Vec<serde_json::Value>, WosError> {
        let user_id = self.uid(user_id.into())?;
        let page_size = page_size.into();
        let mut out: Vec<serde_json::Value> = Vec::new();
        let mut before: Option<String> = None;
        let mut skip_ids: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Backstop, same reason as `list_all_memories`: a server that keeps saying
        // "there is more" forever must not turn a browse into an infinite loop.
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
            if let Some(arr) = page.get("images").and_then(|m| m.as_array()) {
                out.extend(arr.iter().cloned());
            }
            if !page.get("has_more").and_then(|v| v.as_bool()).unwrap_or(false) {
                {
                    ended = true;
                    break;
                }
            }
            let Some(next_before) = page.get("next_before").and_then(|v| v.as_str()) else {
                {
                    ended = true;
                    break;
                }
            };
            let next_skip: Vec<String> = page
                .get("next_skip_ids")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default();
            // The image cursor is not `before` alone, it is **the pair `before` and
            //  `skip_ids`**. Several images can share a timestamp, so a timestamp on its
            //  own either skips them or repeats them. Progress has to be judged on the
            //  pair too: comparing `before` alone reads "nothing moved" while walking a
            //  group that shares one timestamp, and stops there.
            let key = format!("{next_before}|{}", next_skip.join(","));
            if !seen.insert(key) {
                {
                    ended = true;
                    break;
                }
            }
            before = Some(next_before.to_string());
            skip_ids = next_skip;
        }
        if !ended {
            return Err(WosError::Api {
                status: 0,
                message: format!(
                    "stopped after {MAX_PAGES} pages — the store did not end. This is a \
truncated answer, not the whole store."
                ),
            });
        }
        Ok(out)
    }

    /// Every image in a store — the cross-language name for [`Client::list_all_images`].
    ///
    /// Same reason [`Client::export_memories`] exists: the published docs advertise
    /// `iter_images`, so a Rust reader following them hit a method that was not there.
    /// It collects rather than streams — see [`Client::list_all_images`].
    pub async fn iter_images(
        &self,
        user_id: impl Into<Option<&str>>,
        page_size: impl Into<Option<usize>>,
    ) -> Result<Vec<serde_json::Value>, WosError> {
        self.list_all_images(user_id, page_size).await
    }

    // ----- how much has this memory been edited -----

    /// How much of this store has been altered since it was written.
    ///
    /// Aimed at the MODEL rather than at the developer: an assistant leaning on its own
    /// memory should be able to ask how far that memory has been edited underneath it.
    /// Returns `revised` / `unrevised` / `total`, plus a plain-language `counts` and
    /// `excludes`.
    ///
    /// COUNTS ONLY, and the answer is the same size for a store of a hundred memories
    /// and a store of a hundred million. That is the point: a model asks this
    /// mid-conversation, and an answer that grew with the store would be unusable for
    /// exactly the customers who most need to ask. To see WHICH memories, call
    /// [`Client::revisions_page`] — a separate method, so a page can never arrive
    /// because of a default nobody chose.
    ///
    /// Counts memories a transform touched (supersede, update, retract, image removed).
    /// Deletions are NOT counted — a deleted memory leaves nothing to count. Neither are
    /// the internal records derived from what you stored: nobody stored those, so they
    /// do not belong in a ratio that answers "how much of MY memory changed".
    /// Served from `/api/v1/won/*`, not `/api/v1/memory/*`. Won is the surface for
    /// calls a model makes ABOUT its memory rather than calls an application makes WITH
    /// it. The old path still answers, and both share one rate-limit budget.
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
                status: 0,
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
    ///
    /// TypeScript and Python spell this as options on `revisions` itself. Rust takes
    /// positional arguments, so folding four of them into the counts call would make the
    /// cheap question look as expensive as the expensive one — and would have broken
    /// every existing `revisions(user_id)` call site.
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
    ///     // cursor would set `before` back to None, and the engine reads that as "no
    ///     // cursor" — page one, forever, without a single error.
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
    /// otherwise a person's name. Same cursor paging as `list_images`.
    ///
    /// `chunks` / `points_to_delete` report how many internal records a delete would
    /// actually remove — usually more than `returned`, and worth showing to whoever is
    /// about to confirm one.
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
    pub async fn list_all_memories(
        &self,
        user_id: impl Into<Option<&str>>,
    ) -> Result<Vec<serde_json::Value>, WosError> {
        let user_id = self.uid(user_id.into())?;
        let mut out: Vec<serde_json::Value> = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Backstop: the cursor-repeat guard catches a repeated cursor, but not a
        // server that mints a FRESH cursor every page forever — bound the walk.
        // Rust has no `for … else`, so the legitimate exits say so.
        let mut ended = false;
        for _ in 0..MAX_PAGES {
            let mut body = serde_json::json!({ "user_id": user_id, "limit": 100 });
            if let Some(c) = &cursor {
                body["cursor"] = serde_json::json!(c);
            }
            let page = self.post("/api/v1/memory/list", body).await?;
            if let Some(arr) = page.get("memories").and_then(|m| m.as_array()) {
                out.extend(arr.iter().cloned());
            }
            let next = page
                .get("next_cursor")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string());
            // Stop on the last page OR a server that repeats a cursor (else infinite loop).
            match next {
                Some(c) if seen.insert(c.clone()) => cursor = Some(c),
                _ => {
                    ended = true;
                    break;
                }
            }
        }
        if !ended {
            return Err(WosError::Api {
                status: 0,
                message: format!(
                    "stopped after {MAX_PAGES} pages — the store did not end. This is a \
truncated answer, not the whole store."
                ),
            });
        }
        Ok(out)
    }

    /// Every memory in a store — the cross-language name for [`Client::list_all_memories`].
    ///
    /// Python and TypeScript call this `export_memories` / `exportMemories`, and that
    /// is the name the published docs advertise, so a Rust reader following them hit a
    /// method that did not exist. Same behaviour, both names kept.
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
    /// store. Returns `{ "user_id", "status" }` (`status` is `"created"`/`"exists"`).
    pub async fn create_store(&self, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/memory/collection", serde_json::json!({ "user_id": user_id })).await
    }

    /// List your stores: `[{ "user_id", "created_at" }, ...]` (`default` first).
    pub async fn list_stores(&self) -> Result<Vec<serde_json::Value>, WosError> {
        let v = self.request(reqwest::Method::GET, "/api/v1/memory/collections", None, None).await?;
        Ok(v.get("collections").and_then(|c| c.as_array()).cloned().unwrap_or_default())
    }

    /// Delete a store and ALL its memories. Returns `{ "user_id", "status" }`.
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

    /// Unregister a person. Their memories stay; the name tag goes.
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
        //
        // Trimming matters as much as the emptiness test: "   " is not empty, so it used
        // to pass and travel as memory_id. A server that trims it back to nothing reads
        // the request as the whole-store form. `delete_all` below already trimmed; the
        // more dangerous path was the one that did not.
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
        // Destructive calls take the id directly instead of going through uid(), so the
        // collision warning never fired on the two that ERASE data.
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

    /// The key checks, in one place both request paths reach.
    ///
    /// They used to live inside `request_with_key`, which `request_bytes` does not go
    /// through — it builds its own request and sets the header directly. So
    /// `Client::new("").get_image(..)` still sent an empty header and failed at the
    /// network as the mystery 401 these checks exist to prevent, on the one route the
    /// README named without qualification.
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
        // as the same mystery 401. Python refuses it by name; this client did not.
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

    /// One request that answers with BYTES rather than JSON.
    ///
    /// Only `/memory/image` does this, and it is why it cannot go through `request`:
    /// that path parses the body as JSON and errors on anything else, so a JPEG would
    /// surface as a parse failure on a call that actually succeeded.
    ///
    /// Errors still arrive as JSON, so a non-2xx is decoded the same way as everywhere
    /// else and keeps `NotFound` / auth failures behaving identically.
    ///
    /// Retries 429 and connect-level failures, like every other call. A retry AFTER
    /// bytes have arrived would pay for the body twice; neither of these is that —
    /// a 429 carries no image, and a connect failure never reached the server.
    async fn request_bytes(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<(Vec<u8>, String), WosError> {
        self.check_api_key()?;
        let url = format!("{}{}", self.base_url, path);
        let attempts = self.retries.saturating_add(1);
        let deadline_at = self.deadline_at();
        let mut attempt: u32 = 0;
        let resp = loop {
            let sent = self
                .http
                .post(&url)
                .header("X-API-Key", &self.api_key)
                .header("X-WOS-Model", &self.model)
                .json(body)
                .timeout(self.attempt_budget(deadline_at)?)
                .send()
                .await;
            let r = match sent {
                Ok(r) => r,
                Err(e) => {
                    // A connect-level failure never reached the server, so re-sending
                    // cannot double-anything — and that includes a CONNECT timeout, which
                    // reqwest reports with both predicates true. Excluding every timeout
                    // here dropped exactly the case the JSON path retries. The ambiguous
                    // one is a timeout AFTER the request went out, and that arrives as
                    // is_timeout() without is_connect().
                    if e.is_connect() && attempt + 1 < attempts {
                        let delay = backoff(attempt, None);
                        tokio::time::sleep(sleep_within(delay, deadline_at, self.deadline)?).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(WosError::Network(e));
                }
            };
            // 429 carries no image and is refused before any processing, so retrying it
            // is always safe. This route had no loop at all while the module doc promised
            // "every call retries transient failures … 429 always" — measured in the
            // TypeScript client with maxRetries 4 against a 429 server: stats() sent five
            // requests, getImage() sent one. Both siblings fixed it; this one had not.
            if r.status().as_u16() == 429 && attempt + 1 < attempts {
                let ra = r.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
                let delay = backoff(attempt, ra.as_deref());
                tokio::time::sleep(sleep_within(delay, deadline_at, self.deadline)?).await;
                attempt += 1;
                continue;
            }
            break r;
        };
        // The quota this call just spent. rate_limit() promises the MOST RECENT call and
        // this route never wrote to it, so an image loop self-throttling on it read a
        // snapshot frozen at whatever JSON call came before — or None forever.
        if let Some(rl) = parse_rate_limit(resp.headers()) {
            if let Ok(mut g) = self.rl.lock() {
                *g = Some(rl);
            }
        }
        let status = resp.status();
        if status.is_redirection() {
            return Err(WosError::Api {
                status: status.as_u16(),
                message: "unexpected redirect — refused (the API key never follows a redirect). Check base_url: exact host, https://.".into(),
            });
        }
        if !status.is_success() {
            // Through the cap, not `resp.text()`. The SUCCESS body was streamed and
            // bounded at 64MB while the ERROR body on the same route was collected
            // whole — so a base_url that answers 500 and then streams indefinitely
            // could exhaust memory on the one route the module doc says is capped.
            let text = read_capped(resp).await.unwrap_or_default();
            let (message, request_id) = parse_error(&text);
            let message = match request_id {
                Some(id) => format!("{message} (request_id: {id})"),
                None => message,
            };
            return Err(WosError::Api { status: status.as_u16(), message });
        }
        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        let status = status.as_u16();
        let bytes = read_capped_bytes(resp).await?;
        if bytes.is_empty() {
            // An empty 200 would otherwise read as "here is your image" and write a
            // zero-byte file — indistinguishable from an image we lost.
            return Err(WosError::Api {
                status,
                message: "empty image body — the service returned no bytes".into(),
            });
        }
        Ok((bytes.to_vec(), ctype))
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
    /// first response instead of storing twice.
    async fn request_with_key(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
        query: Option<&[(&str, String)]>,
        idempotency_key: Option<&str>,
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
        if !valid_model(&self.model) {
            return Err(WosError::Api {
                status: 400,
                message: format!(
                    "invalid model name: {:?} (letters, digits, '.', '_', '-' only)",
                    self.model
                ),
            });
        }
        // Key hygiene (parity with the Python/TS SDKs): the key is trimmed at
        // construction, so any REMAINING whitespace is a paste error mid-key —
        // sent as-is it reads back as a mystery 401 (or fails header build).
        // Empty is checked here rather than at construction because `new` and
        // `with_base_url` are infallible by design. `Client::new(&env::var("KEY")
        // .unwrap_or_default())` with the variable unset otherwise produced a usable
        // client that sent an empty header and failed at the network as a 401 —
        // Python and TypeScript both refuse it at the call site that made the mistake.
        // `is_whitespace` is false for "", so this had to be its own check.
        self.check_api_key()?;
        let url = format!("{}{}", self.base_url, path);
        // `+ 1` overflowed for with_retries(u32::MAX): debug panicked, release wrapped to
        // 0, and `attempt + 1 < attempts` was then never true — "retry as hard as you can"
        // silently became "do not retry".
        let attempts = self.retries.saturating_add(1);
        let deadline_at = self.deadline_at();
        let mut attempt: u32 = 0;
        loop {
            let start = std::time::Instant::now();
            let mut req = self
                .http
                .request(method.clone(), &url)
                .timeout(self.attempt_budget(deadline_at)?)
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
                Err(e) => {
                    // Retry only when a retry cannot double-process a write:
                    //   · a connect-level failure — the request never reached the server
                    //   · an idempotent method — re-running it changes nothing
                    // Timeouts are excluded from BOTH: they are ambiguous (the write may
                    // have landed), and re-sending would bill it twice.
                    //
                    // The idempotent half was missing here. A GET whose body dropped
                    // mid-stream is not a connect error, so this client gave up where the
                    // TypeScript and Python clients recovered — the three are advertised
                    // as the same product with the same reliability, and a read that
                    // survives a blip in two of them must not fail in the third.
                    let safe = e.is_connect() || (method.is_idempotent() && !e.is_timeout());
                    if safe && attempt + 1 < attempts {
                        let delay = backoff(attempt, None);
                        log_debug!(
                            "{method} {path}: {} — retrying in {}ms (attempt {}/{attempts})",
                            if e.is_connect() { "connect error" } else { "transport error" },
                            delay.as_millis(),
                            attempt + 1
                        );
                        tokio::time::sleep(sleep_within(delay, deadline_at, self.deadline)?).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(WosError::Network(e));
                }
            };
            let status = resp.status();
            // 429 = rate-limited before processing → always safe. 502/503 are
            // ambiguous for a write (may be returned AFTER the backend processed +
            // billed it), so retry those only for idempotent methods — a retried
            // POST could double-store / double-bill.
            let retryable = status.as_u16() == 429
                || (matches!(status.as_u16(), 502 | 503) && method.is_idempotent());
            if retryable && attempt + 1 < attempts {
                let ra = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let delay = backoff(attempt, ra.as_deref());
                log_debug!(
                    "{method} {path} -> {} — retrying in {}ms (attempt {}/{attempts})",
                    status.as_u16(),
                    delay.as_millis(),
                    attempt + 1
                );
                tokio::time::sleep(sleep_within(delay, deadline_at, self.deadline)?).await;
                attempt += 1;
                continue;
            }
            if status.is_redirection() {
                return Err(WosError::Api {
                    status: status.as_u16(),
                    message: "unexpected redirect — refused (the API key never follows a redirect). Check base_url: exact host, https://.".into(),
                });
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
            if let Some(rl) = parse_rate_limit(resp.headers()) {
                if let Ok(mut g) = self.rl.lock() {
                    *g = Some(rl);
                }
            }
            // A drop WHILE READING the body is not a connect error — `send()` already
            // returned, so it surfaces here and nowhere else. Same rule as above: an
            // idempotent read can be re-run safely, a write cannot (the server may have
            // stored and billed it before the connection died). A timeout stays
            // unretried either way.
            let text = match read_capped(resp).await {
                Ok(t) => t,
                Err(e) => {
                    let retry_body = match &e {
                        WosError::Network(ne) => method.is_idempotent() && !ne.is_timeout(),
                        _ => false, // the size cap and friends are not transport failures
                    };
                    if retry_body && attempt + 1 < attempts {
                        let delay = backoff(attempt, None);
                        log_debug!(
                            "{method} {path}: body dropped — retrying in {}ms (attempt {}/{attempts})",
                            delay.as_millis(),
                            attempt + 1
                        );
                        tokio::time::sleep(sleep_within(delay, deadline_at, self.deadline)?).await;
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
            if !status.is_success() {
                let (message, request_id) = parse_error(&text);
                let message = match request_id {
                    Some(id) => format!("{message} (request_id: {id})"),
                    None => message,
                };
                return Err(WosError::Api { status: status.as_u16(), message });
            }
            // An empty body is only legal when the STATUS says there is no body, where
            // it reads as `{}`. An empty body on any other 2xx means a response went
            // missing on the way, and passing that off as success hides it. All three
            // SDKs apply this rule.
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
            // Enforce a JSON OBJECT (parity with the Python/TS SDKs): every WOS
            // endpoint returns one, so a body that parses to null / a number /
            // an array is a broken server, not an empty result to swallow.
            if !v.is_object() {
                return Err(WosError::Api {
                    status: status.as_u16(),
                    message: "expected a JSON object in the response".into(),
                });
            }
            let mut v = v;
            // Only when the body does not already carry it. These responses are
            // widening — a response may carry fields this client has never seen —
            // and a client that writes into the service's object is one
            // release away from overwriting a real answer with its own guess.
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

    // These assert the DESTINATION of a call, which is the thing that silently changed
    // between 2.2.27 (blank sent on, server mapped it to `default`) and 2.2.28 (blank
    // trimmed, so the call was redirected to the client's bound store). Either way the
    // caller never learned their tenant id was empty.

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
    /// `Instant + Duration` panics on overflow, so a deadline past the clock's range
    /// aborted the caller's task from inside the SDK on every call.
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

    /// `retries + 1` wrapped to 0 in release, and `attempt + 1 < attempts` was then never
    /// true — "retry as hard as you can" became "do not retry".
    #[test]
    fn the_largest_retry_count_still_retries() {
        let c = Client::new("wos-live-kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk").with_retries(u32::MAX);
        assert_eq!(c.retries.saturating_add(1), u32::MAX, "attempts must not wrap to 0");
    }

    /// with_user("") used to leave a blank default on the client, and every later call
    /// resolved to it — the builder walking around the guard the per-call path applies.
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
            // The per-call form was already guarded; it must stay guarded.
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
    fn mock_server_recording(responses: Vec<String>) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
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

    fn mock_server(responses: Vec<String>) -> (String, Arc<AtomicUsize>) {
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

    fn http(status: &str, headers: &str, body: &str) -> String {
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

    /// The count is refused out of range, not adjusted.
    ///
    /// `recall` has always been 5..=20 and the service refuses anything else.
    /// Search had no contract: this crate sent whatever it was given, the MCP
    /// server allowed 1..=60, and the service capped at 50 with no floor. Both
    /// ends and every method, because a guard that lives in one of five call
    /// sites is the shape of bug this crate has shipped before.
    /// `get_image` goes through `request_bytes`, not `request_with_key`, so the key
    /// checks that lived in the latter never ran for it — an empty or control-character
    /// key reached the header and failed at the network as the 401 those checks exist
    /// to replace. The README claimed the guard without naming a route.
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
                        assert_eq!(status, 0, "a refusal is local, not from the service");
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
        // `get` and `delete` refused a blank id; the others sent it and let the server
        // answer, which costs a round trip to learn the caller's own bug. `lineage` was
        // given the trim without the check, so it posted memory_id:"" while Python and
        // TypeScript raised locally — one surface answering the same call three ways.
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
    fn dropping_server(drops: usize, tail: String) -> (String, Arc<AtomicUsize>) {
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
        // A read that survives a network blip in the TypeScript and Python clients must
        // survive it here too. Without the retry the first attempt fails: the drop is not a
        // connect error, and only connect errors were retried.
        let (base, hits) = dropping_server(1, http("200 OK", "", r#"{"collections":[],"count":0}"#));
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base).with_retries(2);
        let got = mem.list_stores().await;
        assert!(got.is_ok(), "a dropped GET body should be retried, got {got:?}");
        assert_eq!(hits.load(Ordering::SeqCst), 2, "one drop + one good response");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_write_is_never_retried_when_the_body_drops_mid_stream() {
        // The other half, and the reason the rule is not simply "retry everything":
        // headers came back, so the server may already have stored AND billed the write.
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
        assert_eq!(backoff(0, Some("2")).as_millis(), 2000);
        assert_eq!(backoff(0, Some("999")).as_millis(), 30_000);
        let b = backoff(3, None).as_millis();
        assert!((4000..=4250).contains(&b), "got {b}");
        let cap = backoff(10, None).as_millis();
        assert!((8000..=8250).contains(&cap), "got {cap}");
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
        // delta-seconds form, capped at 30s.
        assert_eq!(backoff(0, Some("2")).as_millis(), 2000);
        assert_eq!(backoff(0, Some("999")).as_millis(), 30_000);
        // HTTP-date in the past → retry immediately (0), not exponential backoff.
        assert_eq!(backoff(3, Some("Wed, 21 Oct 2015 07:28:00 GMT")).as_millis(), 0);
        // A future HTTP-date → a positive, capped wait.
        let future = httpdate::fmt_http_date(std::time::SystemTime::now() + std::time::Duration::from_secs(5));
        let d = backoff(0, Some(&future)).as_millis();
        assert!(d > 0 && d <= 30_000, "got {d}");
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
        // the field's default. Before 2.2.17 this raised "invalid type: null,
        // expected a string" and dropped EVERY memory in the batch. Parity with the
        // Python (raw dict) and TS (structural cast) SDKs, which pass such rows through.
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
        // repeated in `memories`. Reading only `memories` meant an assistant
        // turn stored with add_turn vanished from search on Scroll 1.2 while the same
        // query returned it on others. Both fields come back, de-duplicated.
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
        // ONE malformed element must not fail the whole call
        // ("invalid type: null, expected struct Memory"), so a single bad element
        // destroyed every good memory in the batch — while Python/TypeScript
        // returned the valid ones. Skip the bad elements, keep the good records.
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
        // must yield [], not an "invalid type" error — parity with py/ts. search_self's
        // already coerced there; plain search / search_with lagged (errored on null).
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
        // A 502 on a write (POST) must NOT retry — the write may have landed.
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

    // ----- 2.2.14 -----

    #[tokio::test(flavor = "current_thread")]
    async fn whitespace_in_key_errors_before_sending() {
        // The key is trimmed at construction; REMAINING (inner) whitespace is a
        // paste error that would read back as a mystery 401 — fail it clearly,
        // before any request (parity with the Python/TS SDKs).
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
    async fn list_all_memories_stops_on_repeated_cursor() {
        // A server that repeats a cursor must not loop forever (only 2 requests).
        let (base, hits) = mock_server(vec![
            http("200 OK", "", "{\"memories\":[{\"id\":\"1\"}],\"next_cursor\":\"C\"}"),
            http("200 OK", "", "{\"memories\":[{\"id\":\"2\"}],\"next_cursor\":\"C\"}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let all = mem.list_all_memories("u").await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(hits.load(Ordering::SeqCst), 2); // stopped after the cursor repeated
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

    /// Standing still still has to stop: never hang forever on a server that keeps
    /// returning the same pair.
    #[tokio::test(flavor = "current_thread")]
    async fn all_images_stops_when_the_cursor_pair_repeats() {
        let page = "{\"images\":[{\"id\":\"x\"}],\"has_more\":true,\"next_before\":\"T\",\"next_skip_ids\":[\"x\"]}";
        let (base, hits) = mock_server(vec![http("200 OK", "", page), http("200 OK", "", page)]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        let all = mem.list_all_images("u", None).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    // An empty body was treated differently by each SDK: the same `204 No Content`
    // was a success (`{}`) in TypeScript and an "invalid JSON" error here. The three
    // SDKs ship as one surface, so they now agree in both directions.
    #[tokio::test(flavor = "current_thread")]
    async fn no_content_is_a_success_not_a_parse_error() {
        let (base, _) = mock_server(vec![http("204 No Content", "", "")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        assert_eq!(mem.delete("alice", "m1").await.unwrap(), serde_json::json!({}));
    }

    // The API has supported `Idempotency-Key` across the whole memory plane the entire
    // time, but no SDK could send one — there was no header path at all (measured
    // 2026-07-31). Pin that the header actually rides on the write.
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
        // Won is a separate address, not a rename: calls a model makes ABOUT its memory
        // live under /api/v1/won/*. The old /memory path still answers, so drifting back
        // fails nothing at runtime — it only makes the docs teach an address no client
        // calls. Pin it here so the three languages cannot disagree about it either.
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{\"revised\":3,\"total\":40}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        mem.revisions("alice").await.unwrap();
        let reqs = seen.lock().unwrap();
        assert!(reqs[0].contains("POST /api/v1/won/revisions"), "got {}", reqs[0]);
        assert!(!reqs[0].contains("/api/v1/memory/revisions"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_full_keeps_what_search_merges_away() {
        // `search` answers with one merged Vec, so the photos and the count of re-ask
        // passes actually run had nowhere to land. Both are billable, and both were
        // being discarded.
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
        // Of the three languages this is the only one that actually stops a typo.
        //   Python's **opts and TypeScript's [key: string]: unknown let `verfy` through;
        //   a struct cannot.
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
        // If the cursor loses its name the engine reads "no cursor", and the caller gets
        // **page one forever**, without a single error.
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
        // The three SDKs have to accept the SAME base64 on the same call. `base64` and
        // `openssl base64` wrap at 76 columns, and Python/TypeScript have stripped those
        // newlines since 2.2.31 while this client passed them straight through — so the
        // identical file worked in two languages and 400'd in the third.
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
            "",
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
        // A data: URL with nothing after the comma is an empty image, and the engine
        // answers "not a readable image" — a message that points at the file. Say it here.
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", "http://127.0.0.1:1");
        match mem
            .add_with("", "alice", serde_json::json!({}), serde_json::json!({"image": {"data": "data:image/png;base64,"}}))
            .await
        {
            Err(WosError::Api { status: 400, message }) => {
                assert!(message.contains("empty"), "got {message}")
            }
            other => panic!("expected a 400 about an empty image, got {other:?}"),
        }
        match mem
            .add_with("", "alice", serde_json::json!({}), serde_json::json!({"image": {"reference": "s3://x"}}))
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
        // `with_timeout` bounded one attempt, so nothing bounded the call: at the
        // defaults a single call can hold for 30s + backoff + 30s + backoff + 30s and
        // a handler awaiting it had no way to say how long it actually had. The
        // 5-second Retry-After below is the backoff this budget must cut short.
        let (base, _) = mock_server(vec![
            http("429 Too Many Requests", "Retry-After: 5\r\n", "{\"error\":\"rate limited\"}"),
            http("200 OK", "", "{\"memories\":[]}"),
        ]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(200));
        let t0 = std::time::Instant::now();
        match mem.search("q", "alice", 10).await {
            Err(WosError::Api { status: 0, message }) => {
                assert!(message.contains("exhausted"), "got {message}")
            }
            other => panic!("expected an exhausted budget, got {other:?}"),
        }
        // Under the budget, not merely under some larger number: the bound here was
        // 1.5s, which a run that slept out the remainder and then gave up passed just
        // as well as one that refused at once.
        assert!(t0.elapsed().as_secs_f64() < 0.15, "spent {:?} on a 200ms budget", t0.elapsed());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_clone_keeps_the_deadline() {
        // The same trap the transport fell into: a clone that quietly drops it works
        // right up to the call that needed the budget.
        let (base, _) = mock_server(vec![http(
            "429 Too Many Requests", "Retry-After: 5\r\n", "{}",
        )]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base)
            .with_deadline(std::time::Duration::from_millis(200))
            .with_model("tablet-2");
        assert!(mem.search("q", "alice", 10).await.is_err());
    }

    // ── recall's promise, kept ────────────────────────────────────────────
    #[tokio::test(flavor = "current_thread")]
    async fn recall_refuses_a_count_out_of_range_without_sending() {
        // `RecallOpts` said "out of range is refused, not clamped" and nothing
        // enforced it. `limit: 500` reached the engine and died there — a service
        // error for a mistake visible before opening a socket. The mock is handed a
        // response it must never get to serve.
        let (base, seen) = mock_server_recording(vec![http("200 OK", "", "{}")]);
        let mem = Client::with_base_url("wos-test-xxxxxxxxxx", &base);
        for bad in [0usize, 1, 4, 21, 500] {
            let opts = RecallOpts { limit: Some(bad), context_limit: None };
            match mem.recall_opts("q", "alice", &opts).await {
                Err(WosError::Api { status: 0, message }) => {
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
    // ── 2.2.25 ─────────────────────────────────────────────────────────────

    // The API lowercases a store id and rewrites anything outside [a-z0-9_] to '_', so
    // Alice.Smith · alice-smith · alice_smith are one store (measured against production
    // 2026-08-05). In an app with one store per end user, two people get merged.
    #[test]
    fn store_id_normalization_is_what_the_api_does() {
        assert_eq!(normalize_store_id("Alice.Smith"), "alice_smith");
        assert_eq!(normalize_store_id("alice-smith"), "alice_smith");
        assert_eq!(normalize_store_id("bob.lee@x.com"), "bob_lee_x_com");
        assert_eq!(normalize_store_id("bob-lee@x.com"), "bob_lee_x_com"); // ← collision
        assert_eq!(normalize_store_id("alice_smith"), "alice_smith"); // already normal form
        assert_eq!(normalize_store_id("na\u{ef}ve"), "na_ve"); // non-ASCII folds too
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

    /// The warning record is process-global and tests run in parallel. These three
    /// read that state directly, so serialize them; without this, entries written by
    /// another test bleed in and the result changes from run to run.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// This warning fires on email-shaped ids, and the pattern this SDK recommends
    /// (one store per end user) feeds it one per user. Unbounded, it grows for the
    /// life of the process.
    #[test]
    fn warned_ids_stay_bounded() {
        let _g = serial();
        _reset_warning_state();
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
        _reset_warning_state();
        for i in 0..(WARNED_STORE_IDS_MAX * 2) {
            warn_if_store_id_collapses(&format!("user.{i}@example.com"));
        }
        warn_if_store_id_collapses("zzz.late@example.com");
        let g = WARNED_STORE_IDS.get().unwrap().lock().unwrap();
        assert!(g.iter().any(|s| s == "zzz.late@example.com"), "late collision was not recorded");
        assert!(g.len() <= WARNED_STORE_IDS_MAX);
    }

    /// An id that does not fold is never recorded, so it cannot waste the cap.
    #[test]
    fn clean_ids_are_not_recorded() {
        let _g = serial();
        _reset_warning_state();
        for i in 0..100 {
            warn_if_store_id_collapses(&format!("user_{i}"));
        }
        let n = WARNED_STORE_IDS.get().map(|m| m.lock().unwrap().len()).unwrap_or(0);
        assert_eq!(n, 0, "an already-canonical id has no reason to be recorded");
    }
}

#[cfg(test)]
mod plain_http_host_tests {
    // `http://127.0.0.1:9@evil.example` reads as loopback to a naive check: the authority
    // was split on ':' before the userinfo was removed, so the check saw "127.0.0.1" and
    // stayed silent while the API key travelled in cleartext to evil.example. The host is
    // whatever follows the last '@'.
    fn host_of(base_url: &str) -> String {
        let lower = base_url.to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix("http://") else { return String::new() };
        let authority = rest.split('/').next().unwrap_or("");
        let hostport = authority.rsplit('@').next().unwrap_or("");
        if let Some(v6) = hostport.strip_prefix('[') {
            v6.split(']').next().unwrap_or("").to_string()
        } else {
            hostport.split(':').next().unwrap_or("").to_string()
        }
    }

    #[test]
    fn userinfo_cannot_impersonate_loopback() {
        assert_eq!(host_of("http://127.0.0.1:9@evil.example/x"), "evil.example");
        assert_eq!(host_of("http://localhost@evil.example"), "evil.example");
    }

    #[test]
    fn real_loopback_still_reads_as_loopback() {
        assert_eq!(host_of("http://127.0.0.1:8080/x"), "127.0.0.1");
        assert_eq!(host_of("http://localhost:3000"), "localhost");
        assert_eq!(host_of("http://[::1]:8080"), "::1");
    }
}
