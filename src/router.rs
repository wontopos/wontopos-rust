use super::*;

pub(crate) const ASK_MODEL: &str = "claude-haiku-5-5";
pub(crate) const ROUTER_MAX_TOKENS: u32 = 2_000;
pub(crate) const SKILL_MAX_TOKENS: u32 = 7_000;
pub(crate) const ROUTER_PROMPT_MAX_TOKENS: u64 = 95_000;
pub(crate) const ROUTER_STABLE_MAX_TOKENS: u64 = 80_000;
pub(crate) const FRAME_TOKENS: u64 = 64;
pub(crate) const EXCHANGE_MAX_CHARS: usize = 2_000;
pub(crate) const EXCHANGE_ANSWER_CHARS: usize = 300;
pub(crate) const SPEAKER_MAX_BYTES: usize = 256;
pub(crate) const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
pub(crate) const ROUTER_MAPS_MAX: usize = 10_000;
pub(crate) const TOKEN_COUNTS_MAX: usize = 256;
pub(crate) const SKILLS_EXPORT_VERSION: u64 = 1;
pub(crate) const MAX_TIMER_MS: u64 = 2_147_483_647;
pub(crate) const SKILL_FIELDS: [&str; 11] =
    ["name", "description", "kind", "input", "instructions", "max_tokens", "effort", "model", "caller", "confirm", "metadata"];
pub(crate) const SKILL_OPTIONAL: [&str; 7] = ["instructions", "max_tokens", "effort", "model", "caller", "confirm", "metadata"];

pub(crate) fn is_skill_name(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=32).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

/// Why an ask did not go through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AskReason {
    NotConfigured,
    NoTimeZone,
    NoSkills,
    Limit,
    TooLarge,
    Credit,
    SpendLimit,
    Auth,
    RateLimit,
    Overloaded,
    Timeout,
    Cancelled,
    Network,
    BadModel,
    BadRequest,
    Refusal,
    Truncated,
    ToolError,
    BadChoice,
    NeedsConfirmation,
    Failed,
}

impl AskReason {
    /// Every reason.
    pub const ALL: [AskReason; 21] = {
        use AskReason::*;
        [
            NotConfigured, NoTimeZone, NoSkills, Limit, TooLarge, Credit, SpendLimit, Auth, RateLimit, Overloaded, Timeout,
            Cancelled, Network, BadModel, BadRequest, Refusal, Truncated, ToolError, BadChoice, NeedsConfirmation, Failed,
        ]
    };

    /// The reason as it is written, such as `"rate_limit"`.
    pub fn as_str(self) -> &'static str {
        [
            "not_configured", "no_time_zone", "no_skills", "limit", "too_large", "credit", "spend_limit", "auth",
            "rate_limit", "overloaded", "timeout", "cancelled", "network", "bad_model", "bad_request", "refusal",
            "truncated", "tool_error", "bad_choice", "needs_confirmation", "failed",
        ][self as usize]
    }

    /// What the model is told when an ask ends for this reason.
    pub fn text(self) -> &'static str {
        router_text::failed(self)
    }

    fn from_provider(s: &str) -> Option<AskReason> {
        use AskReason::*;
        [Credit, SpendLimit, Auth, RateLimit, Overloaded, BadModel, BadRequest, TooLarge].into_iter().find(|r| r.as_str() == s)
    }
}

impl std::fmt::Display for AskReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a skill runs: the skill model follows its instructions, or the app's function runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillKind {
    #[default]
    Llm,
    Code,
}

/// A skill: one thing the router can choose to do for the model that asks.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct SkillDefinition {
    /// 1-32 of `[a-z0-9_]`, starting with a letter.
    pub name: String,
    /// What the router reads to choose it.
    pub description: String,
    pub kind: SkillKind,
    /// JSON Schema of the input the router fills in.
    pub input: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// `Llm` only: the most the skill model writes (default 7000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `Llm` only: the name of a caller in [`RouterOptions::callers`] that runs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller: Option<String>,
    /// Run only after a person agrees, through [`Client::run_skill`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<bool>,
    /// Anything the app keeps with the skill; never sent to a model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
}

/// What a code skill's function gets beside its input.
#[derive(Debug, Clone)]
pub struct SkillContext {
    pub store: String,
    pub request: String,
    /// What the router added for the skill, or `""`.
    pub note: String,
    pub ask_id: String,
    pub speaker: Option<String>,
    /// When the ask's time runs out.
    pub deadline: std::time::Instant,
}

/// What a code skill's function returns when it fails.
pub type SkillError = Box<dyn std::error::Error + Send + Sync>;

/// A code skill's function: the checked input in, the result out.
pub type SkillHandler =
    std::sync::Arc<dyn Fn(serde_json::Value, SkillContext) -> BoxFuture<'static, Result<serde_json::Value, SkillError>> + Send + Sync>;

/// A [`SkillHandler`] from an async function or closure.
pub fn skill_handler<F, Fut>(f: F) -> SkillHandler
where
    F: Fn(serde_json::Value, SkillContext) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<serde_json::Value, SkillError>> + Send + 'static,
{
    std::sync::Arc::new(move |input, ctx| Box::pin(f(input, ctx)))
}

/// The router's limits.
#[derive(Debug, Clone, Copy, Default)]
pub struct RouterLimits {
    /// Asks per answer, the same store and `turn` (default 3).
    pub per_answer: Option<u32>,
    /// Asks per store within `window`, for asks without a `turn` (default 3).
    pub per_window: Option<u32>,
    /// Default one minute.
    pub window: Option<std::time::Duration>,
    /// Time the asks of one answer may take together (default 75 seconds).
    pub time: Option<std::time::Duration>,
    /// Asks per store per day (default 200).
    pub per_day: Option<u32>,
}

/// The router behind [`Client::ask`]: its callers, models, prompt, skill functions and limits.
#[derive(Clone, Default)]
pub struct RouterOptions {
    /// The Anthropic key of the router's default caller, and of the skills' too unless `skill_key` is set.
    pub key: Option<String>,
    /// A separate Anthropic key for LLM skills (default `key`).
    pub skill_key: Option<String>,
    /// The router's model (default the caller's own, else `claude-haiku-5-5`).
    pub router_model: Option<String>,
    /// The router's caller (default the Anthropic caller with `key`).
    pub router_caller: Option<std::sync::Arc<dyn ModelCaller>>,
    /// The model of LLM skills (default the skill caller's own, else `claude-haiku-5-5`).
    pub skill_model: Option<String>,
    /// The caller of LLM skills (default the Anthropic caller with `skill_key`, else `key`).
    pub skill_caller: Option<std::sync::Arc<dyn ModelCaller>>,
    pub callers: std::collections::HashMap<String, std::sync::Arc<dyn ModelCaller>>,
    /// The router's output budget, 2000 (the default) to 128000.
    pub max_tokens: Option<u32>,
    pub prompt: Option<String>,
    pub name: Option<String>,
    /// The person's IANA time zone; default the client's `tz`, and `ask` refuses without either.
    pub time_zone: Option<String>,
    pub handlers: std::collections::HashMap<String, SkillHandler>,
    pub limits: RouterLimits,
    /// The API origin of the callers made from `key` and `skill_key` (default `https://api.anthropic.com`).
    pub provider_base_url: Option<String>,
}

impl std::fmt::Debug for RouterOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names: Vec<&String> = self.handlers.keys().collect();
        names.sort();
        let mut callers: Vec<&String> = self.callers.keys().collect();
        callers.sort();
        f.debug_struct("RouterOptions")
            .field("key", &self.key.as_deref().map(mask_key))
            .field("skill_key", &self.skill_key.as_deref().map(mask_key))
            .field("router_model", &self.router_model)
            .field("router_caller", &self.router_caller.is_some())
            .field("skill_model", &self.skill_model)
            .field("skill_caller", &self.skill_caller.is_some())
            .field("callers", &callers)
            .field("max_tokens", &self.max_tokens)
            .field("prompt", &self.prompt.is_some())
            .field("name", &self.name)
            .field("time_zone", &self.time_zone)
            .field("handlers", &names)
            .field("limits", &self.limits)
            .field("provider_base_url", &self.provider_base_url.as_deref().map(redacted_base_url))
            .finish()
    }
}

/// Whether an ask reads the router's last exchanges and keeps its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum History {
    #[default]
    Use,
    None,
}

/// Per-call options for [`Client::ask`] and [`Client::router_tools`].
#[derive(Debug, Clone, Default)]
pub struct AskOpts {
    /// Your id for the answer this ask belongs to; asks with the same turn share its limits.
    pub turn: Option<String>,
    pub speaker: Option<String>,
    pub history: History,
}

/// Per-call options for [`Client::run_skill`].
#[derive(Debug, Clone, Default)]
pub struct RunSkillOpts {
    pub request: Option<String>,
    pub note: Option<String>,
    pub speaker: Option<String>,
    /// The `ask_id` of the `NeedsConfirmation` answer.
    pub ask_id: Option<String>,
}

/// One side of [`AskHistory`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryStep {
    Ok,
    Failed,
    Off,
    Skipped,
}

/// Whether `ask` read its history and kept its exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct AskHistory {
    pub read: HistoryStep,
    pub pushed: HistoryStep,
}

/// The provider's usage numbers for the router call and the skill calls, summed.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct AskUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub router: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill: Option<serde_json::Value>,
}

/// What the router did: picked a skill, said what the skills can do, or said it cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AskAction {
    Skill,
    Explain,
    Cannot,
}

impl AskAction {
    fn as_str(self) -> &'static str {
        match self {
            AskAction::Skill => "skill",
            AskAction::Explain => "explain",
            AskAction::Cannot => "cannot",
        }
    }
}

/// What [`Client::ask`] returns; give the model `text`, or [`ask_for_model`]'s answer.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AskResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<AskAction>,
    /// For the model: the skill's result, the router's words, or why not.
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<AskReason>,
    /// The input the router gave the skill, for [`Client::run_skill`] after a yes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    /// What failed, in words for you, never for the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub store: String,
    pub ask_id: String,
    pub history: AskHistory,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<AskUsage>,
}

/// What the model that asked reads, from [`ask_for_model`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ModelAnswer {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<AskAction>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

/// What `ask` gives the model that asked.
pub fn ask_for_model(r: &AskResult) -> ModelAnswer {
    ModelAnswer {
        ok: r.ok,
        action: r.action,
        text: r.text.clone(),
        skill: r.skill.clone().filter(|s| !s.is_empty()),
        truncated: r.truncated.then_some(true),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AskReadiness {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<AskReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// One exchange kept on a store's router channel.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RouterExchange {
    pub request: String,
    pub answer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl RouterExchange {
    fn from_value(v: &serde_json::Value) -> Option<Self> {
        let o = v.as_object()?;
        let request = o.get("request")?.as_str()?.to_string();
        let answer = o.get("answer")?.as_str()?.to_string();
        let text = |k: &str| o.get(k).and_then(|s| s.as_str()).map(str::to_string);
        let extra = o
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "request" | "answer" | "speaker" | "at"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Some(RouterExchange { request, answer, speaker: text("speaker"), at: text("at"), extra })
    }
}

/// The `ask` tool for one store, from [`Client::router_tools`].
#[derive(Debug)]
pub struct RouterTools {
    client: Client,
    store: String,
    opts: AskOpts,
}

impl RouterTools {
    /// The tool, in the model API's own format.
    pub fn definitions(&self, format: ToolFormat) -> Vec<serde_json::Value> {
        use serde_json::json;
        let schema = json!({
            "type": "object",
            "properties": {"request": {"type": "string", "description": router_text::TOOL_REQUEST}},
            "required": ["request"]
        });
        vec![match format {
            ToolFormat::Anthropic => json!({"name": "ask", "description": router_text::TOOL, "input_schema": schema}),
            ToolFormat::OpenAi => json!({
                "type": "function",
                "function": {"name": "ask", "description": router_text::TOOL, "parameters": schema}
            }),
        }]
    }

    /// Answer one call of `ask` with the JSON text the model reads.
    pub async fn run(&self, name: &str, input: &serde_json::Value) -> Result<String, WosError> {
        if name != "ask" {
            return Err(refused(format!("not the ask tool: {}", js_quote(name))));
        }
        let request = match input.get("request") {
            Some(serde_json::Value::String(s)) if !js_trim(s).is_empty() => s.clone(),
            _ => return Ok(model_refusal(router_text::NO_REQUEST)),
        };
        let r = self.client.ask(&request, Some(self.store.as_str()), &self.opts).await?;
        Ok(serde_json::to_string(&ask_for_model(&r)).unwrap_or_default())
    }

    /// [`RouterTools::run`] with the arguments as a JSON string, as OpenAI sends them.
    pub async fn run_arguments(&self, name: &str, arguments: &str) -> Result<String, WosError> {
        if name != "ask" {
            return Err(refused(format!("not the ask tool: {}", js_quote(name))));
        }
        let Some(input) = arguments_of(arguments) else { return Ok(model_refusal(router_text::NOT_JSON)) };
        self.run(name, &input).await
    }
}

fn model_refusal(text: &str) -> String {
    serde_json::to_string(&ModelAnswer { ok: false, action: None, text: text.into(), skill: None, truncated: None })
        .unwrap_or_default()
}

fn model_failure(e: &ModelCallError) -> Failure {
    let status = e.status;
    let kind = e.error_type.as_deref().unwrap_or("");
    let msg = clean_text(&e.message, 512);
    let detail = if status == 0 {
        msg.clone()
    } else if kind.is_empty() {
        format!("{status}: {msg}")
    } else {
        format!("{status} {kind}: {msg}")
    };
    let wait = e.retry_after;
    if let Some(r) = e.reason.as_deref().and_then(AskReason::from_provider) {
        return Failure { reason: r, detail: Some(detail), retry_after: if r == AskReason::RateLimit { wait } else { None } };
    }
    let reason = if status == 0 {
        if e.reason.as_deref() == Some("timeout") { AskReason::Timeout } else { AskReason::Network }
    } else if kind == "billing_error" || status == 402 || (status == 400 && says_any(&msg, &["credit balance"])) {
        AskReason::Credit
    } else if status == 400 && says_any(&msg, &["usage limit", "spend limit", "spending limit", "spend cap"]) {
        AskReason::SpendLimit
    } else if status == 401 || status == 403 || kind == "authentication_error" || kind == "permission_error" {
        AskReason::Auth
    } else if status == 404 {
        AskReason::BadModel
    } else if status == 413 {
        AskReason::TooLarge
    } else if status == 429 {
        return match wait {
            None => Failure::new(AskReason::SpendLimit, format!("{detail} (no Retry-After)")),
            Some(w) => Failure { reason: AskReason::RateLimit, detail: Some(detail), retry_after: Some(w) },
        };
    } else if status == 408 || status == 529 || status >= 500 || kind == "overloaded_error" {
        AskReason::Overloaded
    } else if status == 400 && says_any(&msg, &CONTEXT_WORDS) {
        AskReason::TooLarge
    } else if status == 400 {
        AskReason::BadRequest
    } else {
        AskReason::Failed
    };
    Failure::new(reason, detail)
}

#[derive(Debug, Clone)]
struct Failure {
    reason: AskReason,
    detail: Option<String>,
    retry_after: Option<std::time::Duration>,
}

impl Failure {
    fn new(reason: AskReason, detail: impl Into<String>) -> Self {
        Failure { reason, detail: Some(detail.into()), retry_after: None }
    }
}

#[derive(Clone)]
struct Bounded<K, V> {
    map: std::collections::HashMap<K, (u64, V)>,
    order: std::collections::BTreeMap<u64, K>,
    next: u64,
    max: usize,
}

impl<K: std::hash::Hash + Eq + Clone, V> Bounded<K, V> {
    fn new(max: usize) -> Self {
        Bounded { map: Default::default(), order: Default::default(), next: 0, max }
    }

    fn get(&self, k: &K) -> Option<&V> {
        self.map.get(k).map(|(_, v)| v)
    }

    fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        self.map.get_mut(k).map(|(_, v)| v)
    }

    fn set(&mut self, k: K, v: V) {
        if let Some((seq, _)) = self.map.remove(&k) {
            self.order.remove(&seq);
        }
        let seq = self.next;
        self.next += 1;
        self.order.insert(seq, k.clone());
        self.map.insert(k, (seq, v));
        while self.map.len() > self.max {
            let Some((_, oldest)) = self.order.pop_first() else { break };
            self.map.remove(&oldest);
        }
    }

    fn set_fresh(&mut self, k: K, v: V, stale: impl Fn(&V) -> bool) {
        if self.map.len() >= self.max && !self.map.contains_key(&k) {
            let old: Vec<(u64, K)> =
                self.map.iter().filter(|(_, (_, v))| stale(v)).map(|(key, (seq, _))| (*seq, key.clone())).collect();
            for (seq, key) in old {
                self.order.remove(&seq);
                self.map.remove(&key);
            }
        }
        self.set(k, v);
    }
}

#[derive(Default, Clone)]
struct SkillRegistry {
    stores: std::collections::HashMap<String, std::collections::BTreeMap<String, SkillDefinition>>,
}

impl SkillRegistry {
    fn list(&self, store: &str) -> Vec<SkillDefinition> {
        self.stores.get(&normalize_store_id(store)).map(|s| s.values().cloned().collect()).unwrap_or_default()
    }

    fn get(&self, store: &str, name: &str) -> Option<SkillDefinition> {
        self.stores.get(&normalize_store_id(store)).and_then(|s| s.get(name)).cloned()
    }

    fn has(&self, store: &str, name: &str) -> bool {
        self.stores.get(&normalize_store_id(store)).is_some_and(|s| s.contains_key(name))
    }

    fn add(&mut self, store: &str, defs: Vec<SkillDefinition>) {
        let s = self.stores.entry(normalize_store_id(store)).or_default();
        for d in defs {
            s.insert(d.name.clone(), d);
        }
    }

    fn remove(&mut self, store: &str, name: &str) -> bool {
        let k = normalize_store_id(store);
        let Some(s) = self.stores.get_mut(&k) else { return false };
        if s.remove(name).is_none() {
            return false;
        }
        if s.is_empty() {
            self.stores.remove(&k);
        }
        true
    }
}

#[derive(Debug, Clone, Copy)]
struct Limits {
    per_answer: u32,
    per_window: u32,
    window: std::time::Duration,
    time: std::time::Duration,
    per_day: u32,
}

#[derive(Clone)]
struct Counts {
    answers: Bounded<String, (u32, std::time::Duration, String)>,
    windows: Bounded<String, Vec<std::time::Instant>>,
    days: Bounded<String, (String, u32)>,
}

pub(crate) struct RouterState {
    skills: std::sync::Mutex<SkillRegistry>,
    handlers: std::collections::HashMap<String, SkillHandler>,
    router: Option<std::sync::Arc<dyn ModelCaller>>,
    router_model: String,
    skill: Option<std::sync::Arc<dyn ModelCaller>>,
    skill_model: String,
    callers: std::collections::HashMap<String, std::sync::Arc<dyn ModelCaller>>,
    max_tokens: u32,
    prompt: String,
    name: Option<String>,
    time_zone: Option<(String, chrono_tz::Tz)>,
    limits: Limits,
    tokens: std::sync::Mutex<Bounded<(u64, usize), u64>>,
    counts: std::sync::Mutex<Counts>,
}

impl RouterState {
    pub(crate) fn empty() -> Self {
        RouterState {
            skills: Default::default(),
            handlers: Default::default(),
            router: None,
            router_model: ASK_MODEL.into(),
            skill: None,
            skill_model: ASK_MODEL.into(),
            callers: Default::default(),
            max_tokens: ROUTER_MAX_TOKENS,
            prompt: router_text::PROMPT.into(),
            name: None,
            time_zone: None,
            limits: Limits {
                per_answer: 3,
                per_window: 3,
                window: std::time::Duration::from_secs(60),
                time: std::time::Duration::from_secs(75),
                per_day: 200,
            },
            tokens: std::sync::Mutex::new(Bounded::new(TOKEN_COUNTS_MAX)),
            counts: std::sync::Mutex::new(Counts {
                answers: Bounded::new(ROUTER_MAPS_MAX),
                windows: Bounded::new(ROUTER_MAPS_MAX),
                days: Bounded::new(ROUTER_MAPS_MAX),
            }),
        }
    }

    fn build(o: RouterOptions, wontopos_base: &str, wontopos_key: &str) -> Result<Self, WosError> {
        type Caller = std::sync::Arc<dyn ModelCaller>;
        let mut st = RouterState::empty();
        let provider = match &o.provider_base_url {
            None => ANTHROPIC_BASE_URL.to_string(),
            Some(b) => provider_url(b, Some(wontopos_base), false, GIVE_THE_ORIGIN)?,
        };
        let anthropic = |k: &str, what: &str| -> Result<Caller, WosError> {
            let key = check_model_key(k, what)?;
            if key == js_trim(wontopos_key) {
                return Err(refused(format!(
                    "{what} is this client's Wontopos key: a Wontopos key never goes to a model provider. Give the provider's own key."
                )));
            }
            Ok(std::sync::Arc::new(AnthropicCaller {
                key,
                base: provider_url(&provider, Some(wontopos_base), true, GIVE_THE_ORIGIN)?,
                model: None,
                ttl: CacheTtl::FiveMinutes,
                http: provider_http(),
            }))
        };
        let own = |c: &Caller, what: &str| -> Result<Caller, WosError> {
            if let Some(b) = c.base_url() {
                provider_base(b, Some(wontopos_base))?;
            }
            if let Some(m) = c.model() {
                check_model_name(m, &format!("{what} model"))?;
            }
            Ok(c.clone())
        };
        let by_key = o.key.as_deref().map(|k| anthropic(k, "RouterOptions.key")).transpose()?;
        st.router = match &o.router_caller {
            Some(c) => Some(own(c, "RouterOptions.router_caller")?),
            None => by_key.clone(),
        };
        st.skill = match (&o.skill_caller, &o.skill_key) {
            (Some(c), _) => Some(own(c, "RouterOptions.skill_caller")?),
            (None, Some(k)) => Some(anthropic(k, "RouterOptions.skill_key")?),
            (None, None) => by_key,
        };
        let pick = |given: &Option<String>, caller: &Option<Caller>, what: &str| -> Result<String, WosError> {
            let m = given.clone().or_else(|| caller.as_ref().and_then(|c| c.model().map(str::to_string)));
            check_model_name(m.as_deref().unwrap_or(ASK_MODEL), what)
        };
        st.router_model = pick(&o.router_model, &st.router, "RouterOptions.router_model")?;
        st.skill_model = pick(&o.skill_model, &st.skill, "RouterOptions.skill_model")?;
        for (name, c) in &o.callers {
            if !is_skill_name(name) {
                return Err(refused(format!(
                    "RouterOptions.callers: {} is not a name (1-32 of [a-z0-9_], a letter first).",
                    js_quote(name)
                )));
            }
            st.callers.insert(name.clone(), own(c, &format!("RouterOptions.callers[{}]", js_quote(name)))?);
        }
        if let Some(n) = o.max_tokens {
            st.max_tokens = check_whole(u64::from(n), "RouterOptions.max_tokens", u64::from(ROUTER_MAX_TOKENS), u64::from(MODEL_MAX_TOKENS))? as u32;
        }
        if let Some(p) = &o.prompt {
            if js_trim(p).is_empty() {
                return Err(refused("RouterOptions.prompt must be the whole router prompt: a non-blank string."));
            }
            st.prompt = p.clone();
        }
        if let Some(n) = &o.name {
            if js_trim(n).is_empty() {
                return Err(refused("RouterOptions.name must be a non-blank string."));
            }
            st.name = Some(js_trim(n).to_string());
        }
        st.time_zone = o.time_zone.as_deref().map(check_time_zone).transpose()?;
        for (name, f) in &o.handlers {
            if !is_skill_name(name) {
                return Err(refused(format!(
                    "RouterOptions.handlers: {} is not a skill name (1-32 of [a-z0-9_], a letter first).",
                    js_quote(name)
                )));
            }
            st.handlers.insert(name.clone(), f.clone());
        }
        let l = o.limits;
        let ms = |d: Option<std::time::Duration>, what: &str, default: std::time::Duration| match d {
            None => Ok(default),
            Some(d) => {
                let n = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
                check_whole(n, what, 1, MAX_TIMER_MS).map(std::time::Duration::from_millis)
            }
        };
        let whole = |n: Option<u32>, what: &str, max: u64, default: u32| match n {
            None => Ok(default),
            Some(n) => check_whole(u64::from(n), what, 1, max).map(|n| n as u32),
        };
        st.limits = Limits {
            per_answer: whole(l.per_answer, "RouterOptions.limits.per_answer", 1_000, 3)?,
            per_window: whole(l.per_window, "RouterOptions.limits.per_window", 1_000, 3)?,
            window: ms(l.window, "RouterOptions.limits.window (ms)", std::time::Duration::from_secs(60))?,
            time: ms(l.time, "RouterOptions.limits.time (ms)", std::time::Duration::from_secs(75))?,
            per_day: whole(l.per_day, "RouterOptions.limits.per_day", 1_000_000, 200)?,
        };
        Ok(st)
    }

    async fn stable_tokens(&self, caller: &dyn ModelCaller, c: &ModelCall, run: &AskClock) -> (u64, bool) {
        let key = stable_key(c);
        if let Some(n) = lock(&self.tokens).get(&key).copied() {
            return (n, true);
        }
        match count_tokens(caller, c, run).await {
            None => (call_bytes(c), false),
            Some(n) => {
                lock(&self.tokens).set(key, n);
                (n, true)
            }
        }
    }

    fn reserve(self: &std::sync::Arc<Self>, store: &str, turn: Option<&str>, day: &str, now: std::time::Instant) -> Result<Reservation, String> {
        let l = self.limits;
        let mut c = lock(&self.counts);
        let today = match c.days.get(&store.to_string()) {
            Some((d, n)) if d == day => *n,
            _ => 0,
        };
        if today >= l.per_day {
            return Err(format!("this store has asked {} times today", l.per_day));
        }
        let mut budget = l.time;
        let mut answer = None;
        if let Some(t) = turn {
            let k = format!("{store}\u{0}{t}");
            let (count, spent) = c.answers.get(&k).map_or((0, std::time::Duration::ZERO), |a| (a.0, a.1));
            if count >= l.per_answer {
                return Err(format!("this answer has asked {} times", l.per_answer));
            }
            budget = l.time.saturating_sub(spent);
            if budget.is_zero() {
                return Err(format!("this answer has used its {} ms", l.time.as_millis()));
            }
            c.answers.set_fresh(k.clone(), (count + 1, spent, day.to_string()), |a| a.2.as_str() < day);
            answer = Some(k);
        } else {
            let w: Vec<std::time::Instant> = c
                .windows
                .get(&store.to_string())
                .map(|w| w.iter().copied().filter(|t| now.saturating_duration_since(*t) < l.window).collect())
                .unwrap_or_default();
            if w.len() >= l.per_window as usize {
                return Err(format!("this store has asked {} times in {} ms", l.per_window, l.window.as_millis()));
            }
            let mut w = w;
            w.push(now);
            c.windows.set_fresh(store.to_string(), w, |ts| ts.iter().all(|t| now.saturating_duration_since(*t) >= l.window));
        }
        c.days.set_fresh(store.to_string(), (day.to_string(), today + 1), |d| d.0.as_str() < day);
        Ok(Reservation { state: self.clone(), answer, budget, started: now })
    }
}

struct Reservation {
    state: std::sync::Arc<RouterState>,
    answer: Option<String>,
    budget: std::time::Duration,
    started: std::time::Instant,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(k) = &self.answer {
            let spent = self.started.elapsed();
            if let Some(a) = lock(&self.state.counts).answers.get_mut(k) {
                a.1 += spent;
            }
        }
    }
}

struct AskClock {
    ends: tokio::time::Instant,
}

impl AskClock {
    fn new(budget: std::time::Duration) -> Self {
        AskClock { ends: tokio::time::Instant::now() + budget }
    }

    fn left(&self) -> std::time::Duration {
        self.ends.saturating_duration_since(tokio::time::Instant::now())
    }

    fn expired(&self) -> bool {
        self.left().is_zero()
    }

    fn deadline(&self) -> std::time::Instant {
        self.ends.into_std()
    }

    async fn within<F: std::future::Future>(&self, f: F) -> Option<F::Output> {
        tokio::time::timeout_at(self.ends, f).await.ok()
    }
}

fn timed_out() -> Failure {
    Failure::new(AskReason::Timeout, "the ask's time ran out")
}

#[derive(Debug, Clone)]
enum RouterClock {
    Zone { label: String, tz: chrono_tz::Tz },
    Offset { label: String, minutes: i32 },
}

impl RouterClock {
    fn label(&self) -> &str {
        match self {
            RouterClock::Zone { label, .. } | RouterClock::Offset { label, .. } => label,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClockParts {
    date: String,
    weekday: &'static str,
    time: String,
    year: i64,
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

fn clock_parts(clock: &RouterClock, ms: i64) -> ClockParts {
    match clock {
        RouterClock::Zone { tz, .. } => {
            use chrono::{Datelike, TimeZone, Timelike};
            match tz.timestamp_millis_opt(ms).single() {
                Some(t) => ClockParts {
                    date: format!("{:04}-{:02}-{:02}", t.year(), t.month(), t.day()),
                    weekday: WEEKDAYS[t.weekday().num_days_from_sunday() as usize],
                    time: format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second()),
                    year: i64::from(t.year()),
                },
                None => offset_parts(ms, 0),
            }
        }
        RouterClock::Offset { minutes, .. } => offset_parts(ms, *minutes),
    }
}

fn offset_parts(ms: i64, minutes: i32) -> ClockParts {
    let t = ms.saturating_add(i64::from(minutes) * 60_000);
    let days = t.div_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    let secs = t.rem_euclid(86_400_000) / 1000;
    ClockParts {
        date: format!("{y:04}-{m:02}-{d:02}"),
        weekday: WEEKDAYS[(days + 4).rem_euclid(7) as usize],
        time: format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60),
        year: y,
    }
}

fn in_years(year: i64) -> bool {
    (1..=9999).contains(&year)
}

fn check_time_zone(zone: &str) -> Result<(String, chrono_tz::Tz), WosError> {
    let z = js_trim(zone);
    if z.is_empty() {
        return Err(refused("RouterOptions.time_zone must be an IANA time zone name such as \"Asia/Seoul\"."));
    }
    let found = z.parse::<chrono_tz::Tz>().ok().or_else(|| {
        chrono_tz::TZ_VARIANTS.iter().copied().find(|t| t.name().eq_ignore_ascii_case(z))
    });
    found.map(|tz| (z.to_string(), tz)).ok_or_else(|| {
        refused(format!(
            "RouterOptions.time_zone {} is not a time zone this client knows: use an IANA name such as \"Asia/Seoul\".",
            js_quote(zone)
        ))
    })
}

fn router_turn(exchanges: &[RouterExchange], clock: &RouterClock, now: i64, name: Option<&str>, request: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if exchanges.is_empty() {
        parts.push(router_text::NO_EARLIER.into());
    } else {
        let mut lines = vec![router_text::EARLIER.to_string()];
        for e in exchanges {
            let when = e.at.as_deref().and_then(epoch_ms).filter(|&at| in_years(offset_parts(at, 0).year))
                .map(|at| clock_parts(clock, at))
                .filter(|p| in_years(p.year))
                .map_or_else(|| "?".into(), |p| format!("{} {}", p.date, p.time));
            lines.push(format!("[{when}] request: {}\nanswer: {}", kept_in(&e.request), kept_in(&e.answer)));
        }
        parts.push(lines.join("\n"));
    }
    let n = clock_parts(clock, now);
    parts.push(format!("{} {} ({}) {} {}", router_text::NOW, n.date, n.weekday, n.time, clock.label()));
    if let Some(name) = name {
        parts.push(format!("{} {}", router_text::NAME, kept_in(name)));
    }
    parts.push(format!("{}\n{request}", router_text::REQUEST));
    parts.join("\n\n")
}

fn skill_list(skills: &[SkillDefinition]) -> String {
    let mut lines = vec![router_text::SKILLS.to_string()];
    for s in skills {
        let shown = js_json(&s.input).replace('\u{85}', "\\u0085").replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029");
        lines.push(format!("- {}: {}\n  {} {}", s.name, kept_in(&s.description), router_text::SKILL_INPUT, shown));
    }
    lines.join("\n")
}

fn router_schema(names: &[&str]) -> serde_json::Value {
    let mut skills = vec![""];
    skills.extend_from_slice(names);
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {"type": "string", "enum": ["skill", "explain", "cannot"]},
            "skill": {"type": "string", "enum": skills},
            "input": {"type": "string"},
            "note": {"type": "string"},
            "text": {"type": "string"}
        },
        "required": ["action", "skill", "input", "note", "text"],
        "additionalProperties": false
    })
}

struct Decided {
    action: AskAction,
    skill: String,
    input: String,
    note: String,
    text: String,
}

fn parse_decision(text: &str) -> Option<Decided> {
    let d = first_json_object(text)?;
    let action = match d.get("action").and_then(|a| a.as_str()) {
        Some("skill") => AskAction::Skill,
        Some("explain") => AskAction::Explain,
        Some("cannot") => AskAction::Cannot,
        _ => return None,
    };
    let s = |k: &str| d.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let input = match d.get("input") {
        Some(v @ serde_json::Value::Object(_)) => v.to_string(),
        _ => s("input"),
    };
    Some(Decided { action, skill: s("skill"), input, note: s("note"), text: s("text") })
}

enum Decision {
    Words(AskAction, String),
    Skill { def: Box<SkillDefinition>, input: serde_json::Value, note: String },
}

fn skill_turn(request: &str, note: &str, input: &serde_json::Value) -> String {
    let mut parts = Vec::new();
    if !js_trim(request).is_empty() {
        parts.push(format!("{}\n{}", router_text::FOR_SKILL_REQUEST, kept_in(request)));
    }
    if !note.is_empty() {
        parts.push(format!("{}\n{}", router_text::FOR_SKILL_NOTE, kept_in(note)));
    }
    parts.push(format!("{}\n{}", router_text::FOR_SKILL_INPUT, js_json(input)));
    parts.join("\n\n")
}

fn skill_text(value: &serde_json::Value) -> String {
    let text = match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => js_json(other),
    };
    if js_trim(&text).is_empty() { router_text::EMPTY.into() } else { text }
}

fn exchange_answer(r: &AskResult) -> String {
    use router_text::{KEPT_CUT_OFF, KEPT_FAILED, KEPT_SKILL, KEPT_WAITING};
    let skill = r.skill.as_deref().unwrap_or("undefined");
    let head = if r.reason == Some(AskReason::NeedsConfirmation) {
        format!("[{KEPT_SKILL} {skill}: {KEPT_WAITING}]")
    } else if r.action == Some(AskAction::Skill) {
        if r.ok {
            let cut = if r.truncated { format!(", {KEPT_CUT_OFF}") } else { String::new() };
            format!("[{KEPT_SKILL} {skill}{cut}]")
        } else {
            let reason = r.reason.map_or("undefined", AskReason::as_str);
            format!("[{KEPT_SKILL} {skill}: {KEPT_FAILED}, {reason}]")
        }
    } else {
        format!("[{}]", r.action.map_or("undefined", AskAction::as_str))
    };
    let body = if r.ok { cut_chars(js_trim(&r.text), EXCHANGE_ANSWER_CHARS) } else { String::new() };
    if body.is_empty() { head } else { format!("{head} {body}") }
}

fn cut_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn call_bytes(c: &ModelCall) -> u64 {
    let schema = c.schema.as_ref().map_or(0, |s| s.to_string().len());
    FRAME_TOKENS + (c.user.len() + c.system.iter().map(String::len).sum::<usize>() + schema) as u64
}

fn stable_key(c: &ModelCall) -> (u64, usize) {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let schema = c.schema.as_ref().map(|s| s.to_string()).unwrap_or_default();
    (c.model.as_str(), &c.system, schema.as_str()).hash(&mut h);
    let len = c.model.len() + c.system.iter().map(String::len).sum::<usize>() + schema.len();
    (h.finish(), len)
}

async fn count_tokens(caller: &dyn ModelCaller, c: &ModelCall, run: &AskClock) -> Option<u64> {
    run.within(caller.count_tokens(c)?).await?.ok()
}

fn sum_usage(into: &mut serde_json::Map<String, serde_json::Value>, from: &serde_json::Map<String, serde_json::Value>) {
    for (k, v) in from {
        match v {
            serde_json::Value::Number(n) => {
                let add = n.as_f64().unwrap_or(0.0);
                let sum = into.get(k).and_then(|x| x.as_f64()).map_or(add, |x| x + add);
                let sum = if sum.fract() == 0.0 && sum.abs() < 9e15 {
                    serde_json::json!(sum as i64)
                } else {
                    serde_json::json!(sum)
                };
                into.insert(k.clone(), sum);
            }
            serde_json::Value::Object(o) => {
                let mut inner = match into.remove(k) {
                    Some(serde_json::Value::Object(m)) => m,
                    _ => serde_json::Map::new(),
                };
                sum_usage(&mut inner, o);
                into.insert(k.clone(), serde_json::Value::Object(inner));
            }
            other => {
                into.entry(k.clone()).or_insert_with(|| other.clone());
            }
        }
    }
}

fn first_json_object(text: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    if let Ok(serde_json::Value::Object(o)) = serde_json::from_str::<serde_json::Value>(text) {
        return Some(o);
    }
    let starts: Vec<usize> = text.match_indices('{').map(|(i, _)| i).take(64).collect();
    let ends = brace_ends(text.as_bytes(), &starts);
    for i in starts {
        if let Some(&end) = ends.get(&i) {
            if let Ok(serde_json::Value::Object(o)) = serde_json::from_str::<serde_json::Value>(&text[i..=end]) {
                return Some(o);
            }
        }
    }
    None
}

#[derive(Clone, Copy, PartialEq)]
enum ScanState {
    Out,
    In,
    Esc,
}

struct BraceScans {
    depth: i64,
    open: Vec<(i64, usize)>,
    esc: usize,
}

fn join_scans(into: &mut Vec<(ScanState, BraceScans)>, state: ScanState, g: BraceScans) {
    match into.iter_mut().find(|(s, _)| *s == state) {
        None => into.push((state, g)),
        Some((_, h)) => {
            let shift = h.depth - g.depth;
            h.open.extend(g.open.into_iter().map(|(t, s)| (t + shift, s)));
            h.open.sort_by_key(|&(t, _)| t);
        }
    }
}

fn brace_ends(b: &[u8], starts: &[usize]) -> std::collections::HashMap<usize, usize> {
    let mut ends = std::collections::HashMap::new();
    let Some(&first) = starts.first() else { return ends };
    let mut pending: std::collections::HashSet<usize> = starts.iter().copied().collect();
    let mut groups: Vec<(ScanState, BraceScans)> = Vec::new();
    for (i, &c) in b.iter().enumerate().skip(first) {
        if !matches!(c, b'{' | b'}' | b'"' | b'\\') {
            continue;
        }
        if let Some(k) = groups.iter().position(|(s, g)| *s == ScanState::Esc && g.esc + 1 < i) {
            let (_, g) = groups.remove(k);
            join_scans(&mut groups, ScanState::In, g);
        }
        if c == b'{' && pending.remove(&i) {
            if !groups.iter().any(|(s, _)| *s == ScanState::Out) {
                groups.push((ScanState::Out, BraceScans { depth: 0, open: Vec::new(), esc: 0 }));
            }
            if let Some((_, out)) = groups.iter_mut().find(|(s, _)| *s == ScanState::Out) {
                out.open.push((out.depth, i));
            }
        }
        let mut moved: Vec<(ScanState, BraceScans)> = Vec::new();
        for (state, mut g) in groups.drain(..) {
            let next = match state {
                ScanState::Out => match c {
                    b'{' => {
                        g.depth += 1;
                        state
                    }
                    b'}' => {
                        g.depth -= 1;
                        while g.open.last().is_some_and(|&(t, _)| t == g.depth) {
                            if let Some((_, s)) = g.open.pop() {
                                ends.insert(s, i);
                            }
                        }
                        state
                    }
                    b'"' => ScanState::In,
                    _ => state,
                },
                ScanState::In => match c {
                    b'\\' => {
                        g.esc = i;
                        ScanState::Esc
                    }
                    b'"' => ScanState::Out,
                    _ => state,
                },
                ScanState::Esc => ScanState::In,
            };
            if !g.open.is_empty() {
                join_scans(&mut moved, next, g);
            }
        }
        groups = moved;
        if groups.is_empty() && pending.is_empty() {
            break;
        }
    }
    ends
}

fn new_ask_id() -> String {
    format!("ask-{}", &new_idempotency_key()[4..28])
}

fn ask_turn(turn: Option<&str>) -> Result<Option<String>, WosError> {
    let Some(t) = turn else { return Ok(None) };
    if js_trim(t).is_empty() || t.encode_utf16().count() > 256 {
        return Err(refused(
            "turn must be your id for the answer this ask belongs to: a non-blank string of 256 characters at most.",
        ));
    }
    Ok(Some(js_trim(t).to_string()))
}

fn ask_speaker(speaker: Option<&str>) -> Result<Option<String>, WosError> {
    let Some(s) = speaker else { return Ok(None) };
    let s = js_trim(s);
    if s.is_empty() {
        return Err(refused("speaker must be a non-blank name: leave it out for the store's own exchanges."));
    }
    if s.len() > SPEAKER_MAX_BYTES {
        return Err(refused(format!("speaker is at most {SPEAKER_MAX_BYTES} bytes.")));
    }
    one_line_speaker(s, "speaker")?;
    Ok(Some(s.to_string()))
}

struct AskRun {
    store: String,
    ask_id: String,
    history: AskHistory,
    usage: AskUsage,
}

impl AskRun {
    fn result(&self, ok: bool, text: String) -> AskResult {
        AskResult {
            ok,
            action: None,
            text,
            skill: None,
            truncated: false,
            reason: None,
            input: None,
            detail: None,
            store: self.store.clone(),
            ask_id: self.ask_id.clone(),
            history: self.history,
            usage: (self.usage.router.is_some() || self.usage.skill.is_some()).then(|| self.usage.clone()),
        }
    }

    fn refuse(&self, f: Failure) -> AskResult {
        let mut r = self.result(false, f.reason.text().into());
        r.reason = Some(f.reason);
        r.detail = f.detail;
        r
    }

    fn skill_result(&self, ran: Result<(String, bool), Failure>, skill: &str, input: serde_json::Value) -> AskResult {
        let mut r = match ran {
            Err(f) => self.refuse(f),
            Ok((text, truncated)) => AskResult { truncated, ..self.result(true, text) },
        };
        r.action = Some(AskAction::Skill);
        r.skill = Some(skill.to_string());
        r.input = Some(input);
        r
    }
}

fn edit_within(a: &str, b: &str, max: usize) -> bool {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > max {
        return false;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut diag = prev[0];
        prev[0] = i;
        for j in 1..=b.len() {
            let tmp = prev[j];
            prev[j] = (prev[j] + 1).min(prev[j - 1] + 1).min(diag + usize::from(a[i - 1] != b[j - 1]));
            diag = tmp;
        }
    }
    prev[b.len()] <= max
}

fn check_skill(def: &serde_json::Value, st: &RouterState) -> Result<SkillDefinition, String> {
    let Some(o) = def.as_object() else {
        return Err("a skill is an object: { name, description, kind, input, ... }.".into());
    };
    for k in o.keys() {
        if !SKILL_FIELDS.contains(&k.as_str()) {
            let near = SKILL_FIELDS.iter().find(|n| edit_within(k, n, 2));
            let hint = near.map_or(String::new(), |n| format!(" (did you mean {}?)", js_quote(n)));
            return Err(format!(
                "skill field {} is not one this SDK knows{hint}: {}.",
                js_quote(k),
                SKILL_FIELDS.join(", ")
            ));
        }
    }
    let set: serde_json::Map<String, serde_json::Value> =
        o.iter().filter(|(k, v)| !(v.is_null() && SKILL_OPTIONAL.contains(&k.as_str()))).map(|(k, v)| (k.clone(), v.clone())).collect();
    let o = &set;
    let name = match o.get("name") {
        Some(serde_json::Value::String(n)) if is_skill_name(n) => n.clone(),
        other => {
            return Err(format!(
                "skill name must be 1-32 of [a-z0-9_], starting with a letter: got {}.",
                other.map_or("undefined".into(), js_json)
            ))
        }
    };
    if o.get("description").and_then(|d| d.as_str()).is_none_or(|d| js_trim(d).is_empty()) {
        return Err(format!("{name}: description is required. It is what the router reads to choose the skill."));
    }
    let kind = match o.get("kind").and_then(|k| k.as_str()) {
        Some("llm") => SkillKind::Llm,
        Some("code") => SkillKind::Code,
        _ => return Err(format!("{name}: kind must be \"llm\" or \"code\".")),
    };
    let input = o.get("input").unwrap_or(&serde_json::Value::Null);
    check_schema(input, "input", 0)?;
    if input.get("type").and_then(|t| t.as_str()) != Some("object") {
        return Err(format!("{name}: input must describe an object: its schema needs type: \"object\"."));
    }
    if o.get("confirm").is_some_and(|c| !c.is_boolean()) {
        return Err(format!("{name}: confirm must be true or false."));
    }
    if o.get("metadata").is_some_and(|m| !m.is_object()) {
        return Err(format!("{name}: metadata must be an object."));
    }
    let mut kept = o.clone();
    if kind == SkillKind::Code {
        for k in ["instructions", "max_tokens", "effort", "model", "caller"] {
            if o.contains_key(k) {
                return Err(format!("{name}: {k} is for an llm skill; a code skill runs your function."));
            }
        }
        if !st.handlers.contains_key(&name) {
            return Err(format!("{name}: a code skill needs its function, RouterOptions.handlers[{}].", js_quote(&name)));
        }
    } else {
        if o.get("instructions").and_then(|i| i.as_str()).is_none_or(|i| js_trim(i).is_empty()) {
            return Err(format!("{name}: instructions are required: what the skill model is told to do."));
        }
        if let Some(m) = o.get("max_tokens") {
            let Some(n) = m.as_f64().filter(|n| n.fract() == 0.0 && (1.0..=f64::from(MODEL_MAX_TOKENS)).contains(n)) else {
                return Err(format!(
                    "{name}: max_tokens must be a whole number from 1 to {MODEL_MAX_TOKENS}, got {}.",
                    js_string(Some(m))
                ));
            };
            kept.insert("max_tokens".into(), serde_json::json!(n as u32));
        }
        if o.get("effort").is_some_and(|e| !e.as_str().is_some_and(|e| EFFORTS.contains(&e))) {
            return Err(format!("{name}: effort must be one of {}.", EFFORTS.join(", ")));
        }
        if let Some(m) = o.get("model") {
            let model = m.as_str().ok_or_else(|| format!("{name}: model must be a model name, such as \"{ASK_MODEL}\"."));
            let model = check_model_name(model?, &format!("{name}: model")).map_err(|e| api_message(&e))?;
            kept.insert("model".into(), serde_json::json!(model));
        }
        if let Some(c) = o.get("caller") {
            if !c.as_str().is_some_and(|c| st.callers.contains_key(c)) {
                let mut names: Vec<&str> = st.callers.keys().map(String::as_str).collect();
                names.sort();
                let have = if names.is_empty() { String::new() } else { format!(" ({})", names.join(", ")) };
                return Err(format!("{name}: caller {c} is not one of RouterOptions.callers{have}."));
            }
        }
    }
    let kept = serde_json::Value::Object(kept);
    if nests_deeper(&kept, SKILL_MAX_DEPTH) {
        return Err(format!("{name}: the skill nests deeper than {SKILL_MAX_DEPTH} levels."));
    }
    serde_json::from_value(kept)
        .map_err(|e| format!("{name}: the skill cannot be kept as JSON ({e})."))
}

const SKILL_MAX_DEPTH: usize = 100;

fn nests_deeper(v: &serde_json::Value, levels: usize) -> bool {
    match v {
        serde_json::Value::Object(o) => levels == 0 || o.values().any(|x| nests_deeper(x, levels - 1)),
        serde_json::Value::Array(a) => levels == 0 || a.iter().any(|x| nests_deeper(x, levels - 1)),
        _ => false,
    }
}

fn already_has(store: &str, name: &str, then: &str) -> WosError {
    refused(format!("store {} already has a skill named {}{then}", js_quote(store), js_quote(name)))
}

fn api_message(e: &WosError) -> String {
    match e {
        WosError::Api { message, .. } => message.clone(),
        other => other.to_string(),
    }
}

const SCHEMA_TYPES: [&str; 7] = ["object", "array", "string", "number", "integer", "boolean", "null"];
const SCHEMA_CHECKED: [&str; 16] = [
    "type", "properties", "required", "additionalProperties", "items", "enum", "const", "minLength", "maxLength",
    "minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum", "minItems", "maxItems", "anyOf",
];
const SCHEMA_NOTES: [&str; 6] = ["description", "title", "default", "examples", "$comment", "format"];

fn check_schema(s: &serde_json::Value, at: &str, depth: usize) -> Result<(), String> {
    use serde_json::Value;
    if depth > 32 {
        return Err(format!("{at}: the schema nests deeper than 32 levels."));
    }
    let Some(o) = s.as_object() else { return Err(format!("{at}: a schema is an object.")) };
    let count = |v: &Value, k: &str| {
        if v.as_f64().is_some_and(|n| n >= 0.0 && n.fract() == 0.0) {
            Ok(())
        } else {
            Err(format!("{at}: {k} must be a whole number of 0 or more."))
        }
    };
    for (k, v) in o {
        if SCHEMA_NOTES.contains(&k.as_str()) {
            continue;
        }
        if !SCHEMA_CHECKED.contains(&k.as_str()) {
            return Err(format!(
                "{at}: {} is not a keyword this SDK checks, so an input could pass that the skill does not expect. It checks {}.",
                js_quote(k),
                SCHEMA_CHECKED.join(", ")
            ));
        }
        match k.as_str() {
            "type" => {
                let ts: Vec<&Value> = match v {
                    Value::Array(a) => a.iter().collect(),
                    other => vec![other],
                };
                if ts.is_empty() || !ts.iter().all(|t| t.as_str().is_some_and(|t| SCHEMA_TYPES.contains(&t))) {
                    return Err(format!("{at}: type must be one of {}, or a list of them.", SCHEMA_TYPES.join(", ")));
                }
            }
            "properties" => {
                let Some(props) = v.as_object() else {
                    return Err(format!("{at}: properties must be an object of schemas."));
                };
                for (p, sub) in props {
                    check_schema(sub, &format!("{at}.{p}"), depth + 1)?;
                }
            }
            "required" if !v.as_array().is_some_and(|a| a.iter().all(Value::is_string)) => {
                return Err(format!("{at}: required must be a list of property names."));
            }
            "additionalProperties" if !v.is_boolean() => check_schema(v, &format!("{at}.*"), depth + 1)?,
            "items" => check_schema(v, &format!("{at}[]"), depth + 1)?,
            "enum" if v.as_array().is_none_or(|a| a.is_empty()) => {
                return Err(format!("{at}: enum must be a non-empty list."));
            }
            "anyOf" => {
                let Some(subs) = v.as_array().filter(|a| !a.is_empty()) else {
                    return Err(format!("{at}: anyOf must be a non-empty list of schemas."));
                };
                for (i, sub) in subs.iter().enumerate() {
                    check_schema(sub, &format!("{at}.anyOf[{i}]"), depth + 1)?;
                }
            }
            "minLength" | "maxLength" | "minItems" | "maxItems" => count(v, k)?,
            "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" if !v.is_number() => {
                return Err(format!("{at}: {k} must be a number."));
            }
            _ => {}
        }
    }
    Ok(())
}

fn input_error(v: &serde_json::Value, s: &serde_json::Value, at: &str) -> Option<String> {
    use serde_json::Value;
    let num = |k: &str| s.get(k).and_then(Value::as_f64);
    if let Some(t) = s.get("type") {
        let ts: Vec<&str> = match t {
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            other => other.as_str().into_iter().collect(),
        };
        if !ts.iter().any(|t| is_type(v, t)) {
            return Some(format!("{at} must be {}", ts.join(" or ")));
        }
    }
    if let Some(Value::Array(options)) = s.get("enum") {
        if !options.iter().any(|e| same_json(e, v)) {
            return Some(format!("{at} must be one of {}", Value::Array(options.clone())));
        }
    }
    if let Some(c) = s.get("const") {
        if !same_json(c, v) {
            return Some(format!("{at} must be {c}"));
        }
    }
    if let Value::String(text) = v {
        let n = text.chars().count() as f64;
        if let Some(min) = num("minLength").filter(|m| n < *m) {
            return Some(format!("{at} must be at least {} characters", js_num(min)));
        }
        if let Some(max) = num("maxLength").filter(|m| n > *m) {
            return Some(format!("{at} must be at most {} characters", js_num(max)));
        }
    }
    if let Some(x) = v.as_f64() {
        if let Some(m) = num("minimum").filter(|m| x < *m) {
            return Some(format!("{at} must be at least {}", js_num(m)));
        }
        if let Some(m) = num("maximum").filter(|m| x > *m) {
            return Some(format!("{at} must be at most {}", js_num(m)));
        }
        if let Some(m) = num("exclusiveMinimum").filter(|m| x <= *m) {
            return Some(format!("{at} must be over {}", js_num(m)));
        }
        if let Some(m) = num("exclusiveMaximum").filter(|m| x >= *m) {
            return Some(format!("{at} must be under {}", js_num(m)));
        }
    }
    if let Value::Array(items) = v {
        let n = items.len() as f64;
        if let Some(m) = num("minItems").filter(|m| n < *m) {
            return Some(format!("{at} must hold at least {} items", js_num(m)));
        }
        if let Some(m) = num("maxItems").filter(|m| n > *m) {
            return Some(format!("{at} must hold at most {} items", js_num(m)));
        }
        if let Some(each) = s.get("items").filter(|i| i.is_object()) {
            if let Some(e) = items.iter().enumerate().find_map(|(i, x)| input_error(x, each, &format!("{at}[{i}]"))) {
                return Some(e);
            }
        }
    }
    if let Value::Object(fields) = v {
        let empty = serde_json::Map::new();
        let props = s.get("properties").and_then(Value::as_object).unwrap_or(&empty);
        if let Some(Value::Array(required)) = s.get("required") {
            if let Some(r) = required.iter().filter_map(Value::as_str).find(|r| !fields.contains_key(*r)) {
                return Some(format!("{at}.{r} is required"));
            }
        }
        let extra = s.get("additionalProperties");
        for (k, x) in fields {
            let prop = props.get(k).filter(|p| p.is_object());
            if prop.is_none() && extra == Some(&Value::Bool(false)) {
                return Some(format!("{at}.{k} is not a field this skill takes"));
            }
            if let Some(e) = prop.or(extra.filter(|e| e.is_object())).and_then(|sub| input_error(x, sub, &format!("{at}.{k}"))) {
                return Some(e);
            }
        }
    }
    if let Some(Value::Array(any)) = s.get("anyOf") {
        if !any.iter().any(|sub| sub.is_object() && input_error(v, sub, at).is_none()) {
            return Some(format!("{at} fits none of the shapes it may take"));
        }
    }
    None
}

fn is_type(v: &serde_json::Value, t: &str) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "integer" => v.as_f64().is_some_and(|n| n.fract() == 0.0),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => false,
    }
}

fn same_json(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_json(p, q)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, p)| y.get(k).is_some_and(|q| same_json(p, q)))
        }
        _ => a == b,
    }
}

impl Client {

    /// A copy of this client with a router behind [`Client::ask`].
    pub fn with_router(&self, opts: RouterOptions) -> Result<Self, WosError> {
        let state = RouterState::build(opts, &self.base_url, &self.api_key)?;
        let skills = lock(&self.router.skills).clone();
        let mut stores: Vec<&String> = skills.stores.keys().collect();
        stores.sort();
        for store in stores {
            for def in skills.stores[store].values() {
                let raw = serde_json::to_value(def).map_err(|e| refused(format!("the skill cannot be kept as JSON ({e}).")))?;
                check_skill(&raw, &state).map_err(refused)?;
            }
        }
        *lock(&state.skills) = skills;
        *lock(&state.counts) = lock(&self.router.counts).clone();
        let mut c = self.clone_with(None, None);
        c.router = std::sync::Arc::new(state);
        Ok(c)
    }

    /// Add a skill to a store, kept in this process. A name the store already has is refused.
    pub async fn add_skill(&self, def: SkillDefinition, user_id: impl Into<Option<&str>>) -> Result<SkillDefinition, WosError> {
        let store = self.uid(user_id.into())?;
        let raw = serde_json::to_value(&def).map_err(|e| refused(format!("the skill cannot be kept as JSON ({e}).")))?;
        let d = check_skill(&raw, &self.router).map_err(refused)?;
        let mut skills = lock(&self.router.skills);
        if skills.has(&store, &d.name) {
            return Err(already_has(&store, &d.name, ": remove it first to replace it."));
        }
        skills.add(&store, vec![d.clone()]);
        Ok(d)
    }

    /// A store's skills, by name.
    pub async fn list_skills(&self, user_id: impl Into<Option<&str>>) -> Result<Vec<SkillDefinition>, WosError> {
        let store = self.uid(user_id.into())?;
        Ok(lock(&self.router.skills).list(&store))
    }

    /// Remove a skill from a store; false when it had none of that name.
    pub async fn remove_skill(&self, name: &str, user_id: impl Into<Option<&str>>) -> Result<bool, WosError> {
        let store = self.uid(user_id.into())?;
        let name = check_id(name, "name")?;
        Ok(lock(&self.router.skills).remove(&store, &name))
    }

    /// Copy a skill from one store to others.
    pub async fn copy_skill(&self, name: &str, from: &str, to: &[&str]) -> Result<Vec<String>, WosError> {
        let skill = check_id(name, "name")?;
        if js_trim(from).is_empty() {
            return Err(refused("from is required: the store to copy from. A copy never falls back to the default store."));
        }
        if to.is_empty() {
            return Err(refused("to is required: a store, or a list of stores."));
        }
        for t in to {
            if js_trim(t).is_empty() {
                return Err(refused(format!("to holds {}, which is not a store id.", js_quote(t))));
            }
            warn_if_store_id_collapses(t);
        }
        warn_if_store_id_collapses(from);
        let mut skills = lock(&self.router.skills);
        let Some(src) = skills.get(from, &skill) else {
            return Err(refused(format!("store {} has no skill named {}.", js_quote(from), js_quote(&skill))));
        };
        let from_key = normalize_store_id(from);
        let mut seen = std::collections::HashSet::from([from_key.clone()]);
        for t in to {
            let k = normalize_store_id(t);
            if !seen.insert(k.clone()) {
                let why = if k == from_key { "the store copied from" } else { "in the list twice" };
                return Err(refused(format!("{} is {why}; nothing was copied.", js_quote(t))));
            }
            if skills.has(t, &src.name) {
                return Err(already_has(t, &src.name, "; nothing was copied."));
            }
        }
        for t in to {
            skills.add(t, vec![src.clone()]);
        }
        Ok(to.iter().map(|t| t.to_string()).collect())
    }

    /// A store's skills as JSON text, for [`Client::import_skills`].
    pub async fn export_skills(&self, user_id: impl Into<Option<&str>>) -> Result<String, WosError> {
        #[derive(serde::Serialize)]
        struct Export<'a> {
            wontopos_skills: u64,
            store: &'a str,
            skills: Vec<SkillDefinition>,
        }
        let store = self.uid(user_id.into())?;
        let skills = lock(&self.router.skills).list(&store);
        serde_json::to_string(&Export { wontopos_skills: SKILLS_EXPORT_VERSION, store: &store, skills })
            .map_err(|e| refused(format!("the skills cannot be written as JSON ({e}).")))
    }

    /// Add the skills of an [`Client::export_skills`] text to a store, all or none.
    pub async fn import_skills(&self, data: &str, user_id: impl Into<Option<&str>>) -> Result<Vec<String>, WosError> {
        let parsed: serde_json::Value =
            serde_json::from_str(data).map_err(|e| refused(format!("the skills are not JSON: {e}")))?;
        let version = parsed.get("wontopos_skills").and_then(serde_json::Value::as_f64);
        let Some(list) = parsed.get("skills").and_then(serde_json::Value::as_array).filter(|_| version == Some(1.0)) else {
            return Err(refused(format!(
                "not an export_skills text: expected {{\"wontopos_skills\": {SKILLS_EXPORT_VERSION}, \"skills\": [...]}}."
            )));
        };
        let store = self.uid(user_id.into())?;
        let defs = list
            .iter()
            .enumerate()
            .map(|(i, s)| check_skill(s, &self.router).map_err(|e| refused(format!("skills[{i}]: {e} Nothing was imported."))))
            .collect::<Result<Vec<_>, _>>()?;
        let mut skills = lock(&self.router.skills);
        let mut names = std::collections::HashSet::new();
        for d in &defs {
            if !names.insert(d.name.as_str()) {
                return Err(refused(format!("{} is in the skills twice; nothing was imported.", js_quote(&d.name))));
            }
            if skills.has(&store, &d.name) {
                return Err(already_has(&store, &d.name, "; nothing was imported."));
            }
        }
        let added = defs.iter().map(|d| d.name.clone()).collect();
        skills.add(&store, defs);
        Ok(added)
    }

    /// Ask the router for something outside memory.
    /// Only a mistake in the arguments is an `Err`; any other failure comes back as `ok: false`.
    pub async fn ask(&self, request: &str, user_id: impl Into<Option<&str>>, opts: &AskOpts) -> Result<AskResult, WosError> {
        if js_trim(request).is_empty() {
            return Err(refused("request is required: what the model asks for, in its own words."));
        }
        let turn = ask_turn(opts.turn.as_deref())?;
        let speaker = ask_speaker(opts.speaker.as_deref())?;
        let use_history = opts.history == History::Use;
        let store = self.uid(user_id.into())?;
        let st = &self.router;
        let step = if use_history { HistoryStep::Skipped } else { HistoryStep::Off };
        let mut run_state = AskRun {
            store: store.clone(),
            ask_id: new_ask_id(),
            history: AskHistory { read: step, pushed: step },
            usage: AskUsage::default(),
        };
        if let Some(f) = self.ask_precheck(&store).await {
            return Ok(run_state.refuse(f));
        }
        let Some(clock) = self.router_clock() else {
            return Ok(run_state.refuse(Failure::new(AskReason::NoTimeZone, "the client has no time zone")));
        };
        let started = std::time::Instant::now();
        let day = clock_parts(&clock, now_ms()).date;
        let lim = match st.reserve(&normalize_store_id(&store), turn.as_deref(), &day, started) {
            Ok(l) => l,
            Err(full) => return Ok(run_state.refuse(Failure::new(AskReason::Limit, full))),
        };
        let run = AskClock::new(lim.budget);
        let mut exchanges = Vec::new();
        let mut no_channel = false;
        if use_history {
            match run.within(self.channel_read(&store, speaker.as_deref(), Some(&run))).await {
                Some(Ok(e)) => {
                    exchanges = e;
                    run_state.history.read = HistoryStep::Ok;
                }
                Some(Err(WosError::Api { status: 404 | 501, .. })) => {
                    no_channel = true;
                    run_state.history.read = HistoryStep::Skipped;
                }
                _ => run_state.history.read = HistoryStep::Failed,
            }
        }
        if run.expired() {
            return Ok(run_state.refuse(timed_out()));
        }
        let decided = self.router_decide(&store, request, exchanges, &clock, &run, &mut run_state.usage).await;
        let mut result = match decided {
            Err(f) => return Ok(run_state.refuse(f)),
            Ok(Decision::Words(action, text)) => {
                let mut r = run_state.result(true, text);
                r.action = Some(action);
                r
            }
            Ok(Decision::Skill { def, input, note }) => {
                let ran = if def.confirm == Some(true) {
                    Err(Failure { reason: AskReason::NeedsConfirmation, detail: None, retry_after: None })
                } else {
                    let ctx = SkillContext {
                        store: store.clone(),
                        request: request.to_string(),
                        note,
                        ask_id: run_state.ask_id.clone(),
                        speaker: speaker.clone(),
                        deadline: run.deadline(),
                    };
                    self.run_skill_def(&def, input.clone(), ctx, &run, &mut run_state.usage).await
                };
                run_state.skill_result(ran, &def.name, input)
            }
        };
        if use_history {
            result.history.pushed = if no_channel || run.expired() {
                HistoryStep::Skipped
            } else {
                let answer = exchange_answer(&result);
                match run.within(self.channel_push(&store, speaker.as_deref(), request, &answer, &run)).await {
                    Some(Ok(())) => HistoryStep::Ok,
                    _ => HistoryStep::Failed,
                }
            };
        }
        drop(lim);
        Ok(result)
    }

    /// Whether [`Client::ask`] can run for this store now.
    pub async fn can_ask(&self, user_id: impl Into<Option<&str>>) -> Result<AskReadiness, WosError> {
        let store = self.uid(user_id.into())?;
        Ok(match self.ask_precheck(&store).await {
            None => AskReadiness { ok: true, reason: None, text: None, detail: None },
            Some(f) => AskReadiness { ok: false, reason: Some(f.reason), text: Some(f.reason.text().into()), detail: f.detail },
        })
    }

    /// Run a skill the router chose, once a person agreed to it; never offer this to a model.
    pub async fn run_skill(
        &self, name: &str, input: &serde_json::Value, user_id: impl Into<Option<&str>>, opts: &RunSkillOpts,
    ) -> Result<AskResult, WosError> {
        let store = self.uid(user_id.into())?;
        let st = &self.router;
        let skill = check_id(name, "name")?;
        let def = lock(&st.skills).get(&store, &skill);
        let Some(def) = def else {
            return Err(refused(format!("store {} has no skill named {}.", js_quote(&store), js_quote(name))));
        };
        if !input.is_object() {
            return Err(refused("input must be an object: the input the skill takes."));
        }
        if let Some(bad) = input_error(input, &def.input, "input") {
            return Err(refused(format!("{}: {bad}.", def.name)));
        }
        let speaker = ask_speaker(opts.speaker.as_deref())?;
        let ask_id = opts.ask_id.as_deref().map_or_else(|| Ok(new_ask_id()), |a| check_id(a, "ask_id"))?;
        let run_state = AskRun {
            store: store.clone(),
            ask_id: ask_id.clone(),
            history: AskHistory { read: HistoryStep::Off, pushed: HistoryStep::Off },
            usage: AskUsage::default(),
        };
        let run = AskClock::new(st.limits.time);
        let ctx = SkillContext {
            store,
            request: opts.request.clone().unwrap_or_default(),
            note: opts.note.clone().unwrap_or_default(),
            ask_id,
            speaker,
            deadline: run.deadline(),
        };
        let mut usage = AskUsage::default();
        let ran = self.run_skill_def(&def, input.clone(), ctx, &run, &mut usage).await;
        let mut r = run_state.skill_result(ran, &def.name, input.clone());
        r.usage = usage.skill.is_some().then_some(usage);
        Ok(r)
    }

    /// The router's last exchanges with a store, oldest first.
    pub async fn router_history(&self, user_id: impl Into<Option<&str>>, speaker: Option<&str>) -> Result<Vec<RouterExchange>, WosError> {
        let store = self.uid(user_id.into())?;
        let speaker = ask_speaker(speaker)?;
        match self.channel_read(&store, speaker.as_deref(), None).await {
            Err(WosError::Api { status: 404 | 501, .. }) => Ok(Vec::new()),
            r => r,
        }
    }

    /// The `ask` tool for a model that calls tools.
    pub fn router_tools<'a>(&self, user_id: impl Into<Option<&'a str>>, opts: &AskOpts) -> Result<RouterTools, WosError> {
        ask_turn(opts.turn.as_deref())?;
        ask_speaker(opts.speaker.as_deref())?;
        Ok(RouterTools { store: self.uid(user_id.into())?, client: self.clone_with(None, None), opts: opts.clone() })
    }

    async fn ask_precheck(&self, store: &str) -> Option<Failure> {
        let st = &self.router;
        if st.router.is_none() {
            return Some(Failure::new(
                AskReason::NotConfigured,
                "pass RouterOptions.key (the model provider's key) or RouterOptions.router_caller to with_router",
            ));
        }
        if self.router_clock().is_none() {
            return Some(Failure::new(
                AskReason::NoTimeZone,
                "the client has no time zone: set RouterOptions.time_zone to an IANA name such as \"Asia/Seoul\", \
                 or with_tz to the UTC offset in hours, such as 9",
            ));
        }
        if lock(&st.skills).list(store).is_empty() {
            return Some(Failure::new(
                AskReason::NoSkills,
                format!("store {} has no skills: add one with add_skill", js_quote(store)),
            ));
        }
        None
    }

    fn router_clock(&self) -> Option<RouterClock> {
        if let Some((label, tz)) = &self.router.time_zone {
            return Some(RouterClock::Zone { label: label.clone(), tz: *tz });
        }
        let minutes = tz_minutes(self.tz?).ok()?;
        Some(RouterClock::Offset { label: utc_label(f64::from(minutes)), minutes })
    }

    async fn channel_read(&self, store: &str, speaker: Option<&str>, run: Option<&AskClock>) -> Result<Vec<RouterExchange>, WosError> {
        let mut body = serde_json::json!({"user_id": store, "channel": "router"});
        if let Some(s) = speaker {
            body["speaker"] = serde_json::json!(s);
        }
        let path = "/api/v1/memory/channel/read";
        let r = match run {
            Some(run) => self.with_deadline(run.left().max(std::time::Duration::from_millis(1))).post(path, body).await?,
            None => self.post(path, body).await?,
        };
        Ok(r.get("exchanges")
            .and_then(serde_json::Value::as_array)
            .map(|a| a.iter().filter_map(RouterExchange::from_value).collect())
            .unwrap_or_default())
    }

    async fn channel_push(&self, store: &str, speaker: Option<&str>, request: &str, answer: &str, run: &AskClock) -> Result<(), WosError> {
        let mut body = serde_json::json!({
            "user_id": store,
            "channel": "router",
            "request": cut_chars(request, EXCHANGE_MAX_CHARS),
            "answer": cut_chars(answer, EXCHANGE_MAX_CHARS),
        });
        if let Some(s) = speaker {
            body["speaker"] = serde_json::json!(s);
        }
        let c = self.with_deadline(run.left().max(std::time::Duration::from_millis(1)));
        c.post("/api/v1/memory/channel/push", body).await.map(|_| ())
    }

    async fn router_decide(
        &self, store: &str, request: &str, exchanges: Vec<RouterExchange>, clock: &RouterClock, run: &AskClock, usage: &mut AskUsage,
    ) -> Result<Decision, Failure> {
        let st = &self.router;
        let Some(caller) = st.router.clone() else {
            return Err(Failure::new(AskReason::NotConfigured, "no router caller"));
        };
        let skills = lock(&st.skills).list(store);
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        let base = ModelCall {
            purpose: CallPurpose::Router,
            model: st.router_model.clone(),
            system: vec![st.prompt.clone(), skill_list(&skills)],
            user: "-".into(),
            max_tokens: st.max_tokens,
            effort: Some("low".into()),
            schema: Some(router_schema(&names)),
            system_tokens: None,
            deadline: run.deadline(),
        };
        let (stable, counted) = st.stable_tokens(&*caller, &base, run).await;
        if run.expired() {
            return Err(timed_out());
        }
        if stable > ROUTER_STABLE_MAX_TOKENS {
            return Err(Failure::new(
                AskReason::TooLarge,
                format!("the router prompt and this store's skills come to {stable} tokens, over {ROUTER_STABLE_MAX_TOKENS}"),
            ));
        }
        let mut held: &[RouterExchange] = &exchanges;
        let call = loop {
            let turn = router_turn(held, clock, now_ms(), st.name.as_deref(), request);
            let bytes = turn.len() as u64;
            let call = ModelCall { user: turn, system_tokens: counted.then_some(stable), ..base.clone() };
            if stable + bytes + FRAME_TOKENS <= ROUTER_PROMPT_MAX_TOKENS {
                break call;
            }
            let exact = count_tokens(&*caller, &call, run).await;
            if run.expired() {
                return Err(timed_out());
            }
            if exact.is_some_and(|n| n <= ROUTER_PROMPT_MAX_TOKENS) {
                break call;
            }
            if held.is_empty() {
                return Err(Failure::new(
                    AskReason::TooLarge,
                    format!("the request does not fit the router's {ROUTER_PROMPT_MAX_TOKENS} tokens"),
                ));
            }
            held = &held[1..];
        };
        let reply = self.call_model(&*caller, &call, run, &mut usage.router).await?;
        if reply.stop == "refusal" {
            return Err(Failure::new(AskReason::Refusal, "the router declined"));
        }
        if reply.stop == "max_tokens" {
            return Err(Failure::new(AskReason::Truncated, "the router's answer was cut off before it was whole"));
        }
        let Some(d) = parse_decision(&reply.text) else {
            return Err(Failure::new(AskReason::BadChoice, "the router's answer holds no decision"));
        };
        if d.action != AskAction::Skill {
            let said = js_trim(&d.text);
            let text = if !said.is_empty() {
                said.to_string()
            } else if d.action == AskAction::Explain {
                format!("{} {}", router_text::EXPLAIN_FALLBACK, names.join(", "))
            } else {
                router_text::CANNOT.to_string()
            };
            return Ok(Decision::Words(d.action, text));
        }
        let def = lock(&st.skills).get(store, &d.skill);
        let Some(def) = def else {
            return Err(Failure::new(
                AskReason::BadChoice,
                format!("the router chose {}, which this store has no skill of", js_quote(&d.skill)),
            ));
        };
        let input: serde_json::Value = if js_trim(&d.input).is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&d.input)
                .map_err(|_| Failure::new(AskReason::BadChoice, format!("the router's input for {} is not JSON", def.name)))?
        };
        if !input.is_object() {
            return Err(Failure::new(AskReason::BadChoice, format!("the router's input for {} is not an object", def.name)));
        }
        if let Some(bad) = input_error(&input, &def.input, "input") {
            return Err(Failure::new(AskReason::BadChoice, format!("the router's input for {}: {bad}", def.name)));
        }
        Ok(Decision::Skill { def: Box::new(def), input, note: js_trim(&d.note).to_string() })
    }

    async fn run_skill_def(
        &self, def: &SkillDefinition, input: serde_json::Value, ctx: SkillContext, run: &AskClock, usage: &mut AskUsage,
    ) -> Result<(String, bool), Failure> {
        let st = &self.router;
        if def.kind == SkillKind::Code {
            let Some(f) = st.handlers.get(&def.name).cloned() else {
                return Err(Failure::new(
                    AskReason::ToolError,
                    format!("no function is registered for {} (RouterOptions.handlers)", def.name),
                ));
            };
            return match run.within(f(input, ctx)).await {
                None => Err(timed_out()),
                Some(Ok(value)) => Ok((skill_text(&value), false)),
                Some(Err(e)) if !run.expired() => Err(Failure::new(
                    AskReason::ToolError,
                    format!("{} returned an error: {}", def.name, clean_text(&e.to_string(), 512)),
                )),
                Some(Err(_)) => Err(timed_out()),
            };
        }
        let Some(caller) = def.caller.as_ref().map_or_else(|| st.skill.clone(), |n| st.callers.get(n).cloned()) else {
            return Err(Failure::new(
                AskReason::NotConfigured,
                match &def.caller {
                    Some(n) => format!("{} names the caller {}, which RouterOptions.callers does not have", def.name, js_quote(n)),
                    None => "an llm skill needs RouterOptions.key, skill_key or skill_caller".into(),
                },
            ));
        };
        let model = def
            .model
            .clone()
            .or_else(|| def.caller.as_ref().and_then(|_| caller.model().map(|m| js_trim(m).to_string())))
            .unwrap_or_else(|| st.skill_model.clone());
        let call = ModelCall {
            purpose: CallPurpose::Skill,
            model,
            system: vec![def.instructions.clone().unwrap_or_default()],
            user: skill_turn(&ctx.request, &ctx.note, &input),
            max_tokens: def.max_tokens.unwrap_or(SKILL_MAX_TOKENS),
            effort: def.effort.clone(),
            schema: None,
            system_tokens: None,
            deadline: run.deadline(),
        };
        let got = self.call_model(&*caller, &call, run, &mut usage.skill).await?;
        if got.stop == "refusal" {
            return Err(Failure::new(AskReason::Refusal, "the skill model declined"));
        }
        if js_trim(&got.text).is_empty() {
            return Err(if got.stop == "max_tokens" {
                Failure::new(AskReason::Truncated, "the skill model ran out of tokens before it wrote anything")
            } else {
                Failure::new(AskReason::Failed, "the skill model answered with no text")
            });
        }
        let cut = got.stop == "max_tokens";
        Ok((got.text, cut))
    }

    async fn call_model(
        &self, caller: &dyn ModelCaller, call: &ModelCall, run: &AskClock, usage: &mut Option<serde_json::Value>,
    ) -> Result<ModelReply, Failure> {
        let mut attempt = 0;
        loop {
            if run.expired() {
                return Err(timed_out());
            }
            let e = match run.within(caller.call(call)).await {
                None => return Err(timed_out()),
                Some(Ok(reply)) => {
                    let reply = reply_shape(reply);
                    if let Some(serde_json::Value::Object(u)) = &reply.usage {
                        let mut into = match usage.take() {
                            Some(serde_json::Value::Object(m)) => m,
                            _ => serde_json::Map::new(),
                        };
                        sum_usage(&mut into, u);
                        *usage = Some(serde_json::Value::Object(into));
                    }
                    return Ok(reply);
                }
                Some(Err(e)) => e,
            };
            if run.expired() {
                return Err(timed_out());
            }
            let f = model_failure(&e);
            if let (AskReason::RateLimit, 0, Some(wait)) = (f.reason, attempt, f.retry_after) {
                if wait < run.left() {
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                    continue;
                }
            }
            return Err(f);
        }
    }
}

/// What [`Client::import_skill_md`] sets beside what a SKILL.md carries.
#[derive(Debug, Clone, Default)]
pub struct SkillMdOptions {
    /// Default: the frontmatter's, with hyphens as `_`.
    pub name: Option<String>,
    /// Default: one `task` string.
    pub input: Option<serde_json::Value>,
    pub model: Option<String>,
    pub caller: Option<String>,
    pub max_tokens: Option<u32>,
    pub effort: Option<String>,
    pub confirm: Option<bool>,
}

const SKILL_MD_NOT_RUN: &str =
    "Only SKILL.md's text was taken. Scripts and other files in the skill's folder, or named in its text, are not run, read or sent.";

fn skill_md_input() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {"task": {"type": "string", "description": router_text::SKILL_MD_TASK}},
        "required": ["task"]
    })
}

#[derive(Debug, Clone, PartialEq)]
struct SkillMd {
    name: String,
    description: String,
    rest: serde_json::Map<String, serde_json::Value>,
    body: String,
}

fn md_lines(text: &str) -> Vec<String> {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    t.replace("\r\n", "\n").replace('\r', "\n").split('\n').map(str::to_string).collect()
}

fn parse_skill_md(text: &str) -> Result<SkillMd, String> {
    let lines = md_lines(text);
    if js_trim(&lines[0]) != "---" {
        return Err(
            "SKILL.md must begin with YAML frontmatter: a --- line, name: and description:, and another --- line.".into(),
        );
    }
    let Some(close) = lines.iter().skip(1).position(|l| js_trim(l) == "---").map(|i| i + 1) else {
        return Err("SKILL.md: the frontmatter has no closing --- line.".into());
    };
    let mut meta = parse_frontmatter(&lines[1..close], 2)?;
    let mut take = |k: &str| meta.iter().position(|(key, _)| key == k).map(|i| meta.remove(i).1);
    let name = take("name");
    let description = take("description");
    let Some(name) = name else { return Err("SKILL.md: the frontmatter has no name.".into()) };
    let name = match &name {
        serde_json::Value::String(n) if n.encode_utf16().count() <= 64 && is_md_name(n.as_str()) => n.clone(),
        other => {
            return Err(format!(
                "SKILL.md: name must be lowercase letters, digits and hyphens, at most 64 characters, got {other}."
            ))
        }
    };
    let Some(description) = description else {
        return Err("SKILL.md: the frontmatter has no description. It is what the router reads to choose the skill.".into());
    };
    let description = match description {
        serde_json::Value::String(d) if !js_trim(&d).is_empty() => d,
        _ => return Err("SKILL.md: description must be text, and not blank.".into()),
    };
    let units = description.encode_utf16().count();
    if units > 1024 {
        return Err(format!("SKILL.md: description is at most 1024 characters, got {units}."));
    }
    let body = js_trim(&lines[close + 1..].join("\n")).to_string();
    if body.is_empty() {
        return Err("SKILL.md: there are no instructions after the frontmatter.".into());
    }
    let rest = meta.into_iter().collect();
    Ok(SkillMd { name, description: js_trim(&description).to_string(), rest, body })
}

fn is_md_name(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()))
}

fn key_value(line: &str, trailing: bool) -> Option<(String, Option<String>)> {
    let key_end = line
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_alphanumeric() || c == '_' || (i > 0 && c == '-')))
        .map_or(line.len(), |(i, _)| i);
    if key_end == 0 {
        return None;
    }
    let key = &line[..key_end];
    let after_key = line[key_end..].trim_start_matches([' ', '\t']);
    let after = after_key.strip_prefix(':')?;
    if after.is_empty() {
        return Some((key.to_string(), None));
    }
    let value = after.trim_start_matches([' ', '\t']);
    if value.len() == after.len() {
        return None;
    }
    if value.contains(['\u{2028}', '\u{2029}', '\r', '\n']) {
        return None;
    }
    let value = if trailing { value.trim_end_matches([' ', '\t']) } else { value };
    Some((key.to_string(), Some(value.to_string())))
}

fn js_indent(l: &str) -> usize {
    l.chars().take_while(|c| js_space(*c)).count()
}

fn parse_frontmatter(lines: &[String], first_line: usize) -> Result<Vec<(String, serde_json::Value)>, String> {
    let mut out: Vec<(String, serde_json::Value)> = Vec::new();
    let place = |i: usize| format!("SKILL.md frontmatter, line {}", first_line + i);
    let indented = |l: &str| l.chars().next().is_some_and(js_space);
    let blank = |l: &str| js_trim(l).is_empty();
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        if blank(line) || js_trim_start(line).starts_with('#') {
            i += 1;
            continue;
        }
        if indented(line) {
            return Err(format!("{}: an indented line with no key above it.", place(i)));
        }
        let Some((key, value)) = key_value(line, true) else {
            let shown = utf16_prefix(line, 80);
            return Err(format!("{}: expected \"key: value\", got {}.", place(i), js_quote(&shown)));
        };
        if out.iter().any(|(k, _)| *k == key) {
            return Err(format!("{}: {key} is given twice.", place(i)));
        }
        let mut j = i + 1;
        while j < lines.len() && (blank(&lines[j]) || indented(&lines[j])) {
            j += 1;
        }
        let mut block: Vec<&str> = lines[i + 1..j].iter().map(String::as_str).collect();
        while block.last().is_some_and(|l| blank(l)) {
            block.pop();
        }
        let v = front_value(value.as_deref().unwrap_or(""), &block, &place(i))?;
        out.push((key, v));
        i = j;
    }
    Ok(out)
}

fn utf16_prefix(s: &str, n: usize) -> String {
    String::from_utf16_lossy(&s.encode_utf16().take(n).collect::<Vec<u16>>())
}

fn front_value(raw: &str, block: &[&str], place: &str) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    let v = js_trim(raw);
    let is_block = matches!(v, "|" | ">" | "|+" | "|-" | ">+" | ">-");
    if is_block {
        let filled: Vec<&&str> = block.iter().filter(|l| !js_trim(l).is_empty()).collect();
        if filled.is_empty() {
            return Ok(Value::String(String::new()));
        }
        let indent = filled.iter().map(|l| js_indent(l)).min().unwrap_or(0);
        let ls: Vec<String> =
            block.iter().map(|l| l.chars().skip(indent).collect::<String>().trim_end_matches(js_space).to_string()).collect();
        let joined = ls.join("\n");
        if v.starts_with('|') {
            return Ok(Value::String(js_trim(&joined).to_string()));
        }
        let mut paragraphs: Vec<String> = Vec::new();
        let mut rest = joined.as_str();
        while let Some(at) = rest.find("\n\n") {
            paragraphs.push(rest[..at].replace('\n', " "));
            rest = rest[at..].trim_start_matches('\n');
        }
        paragraphs.push(rest.replace('\n', " "));
        return Ok(Value::String(js_trim(&paragraphs.join("\n")).to_string()));
    }
    if v.is_empty() {
        let items: Vec<&str> = block.iter().map(|l| js_trim(l)).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
        if items.is_empty() {
            return Ok(Value::String(String::new()));
        }
        if items.iter().all(|l| *l == "-" || l.starts_with("- ")) {
            return items.iter().map(|l| scalar_of(js_trim(&l[1..]), place, 0)).collect::<Result<Vec<_>, _>>().map(Value::Array);
        }
        let mut nested = serde_json::Map::new();
        for l in items {
            let Some((k, val)) = key_value(l, false) else {
                let shown = utf16_prefix(l, 80);
                return Err(format!("{place}: expected \"key: value\" under it, got {}.", js_quote(&shown)));
            };
            nested.insert(k, scalar_of(js_trim(val.as_deref().unwrap_or("")), place, 0)?);
        }
        return Ok(Value::Object(nested));
    }
    if !block.is_empty() {
        if v.starts_with(['"', '\'', '[', '{']) {
            return Err(format!("{place}: a quoted value cannot go on over the indented lines below it."));
        }
        let words: Vec<&str> = std::iter::once(v).chain(block.iter().map(|l| js_trim(l))).filter(|s| !s.is_empty()).collect();
        return Ok(Value::String(words.join(" ")));
    }
    scalar_of(v, place, 0)
}

fn quoted<'a>(v: &'a str, place: &str) -> Result<&'a str, String> {
    let b = v.as_bytes();
    let q = b[0];
    let mut i = 1;
    let mut end = None;
    while i < b.len() {
        if q == b'"' && b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == q {
            if q == b'\'' && b.get(i + 1) == Some(&b'\'') {
                i += 2;
                continue;
            }
            end = Some(i);
            break;
        }
        i += 1;
    }
    let kind = if q == b'"' { "double" } else { "single" };
    let end = end.ok_or_else(|| format!("{place}: a {kind}-quoted value has no closing quote."))?;
    let rest = &v[end + 1..];
    if !rest.is_empty() && !(rest.chars().next().is_some_and(js_space) && js_trim(rest).starts_with('#')) {
        return Err(format!("{place}: only a comment can follow the closing quote."));
    }
    Ok(&v[..=end])
}

const FRONT_LIST_DEPTH: usize = 32;

fn scalar_of(v: &str, place: &str, depth: usize) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    if v.starts_with('"') {
        let s = quoted(v, place)?;
        return serde_json::from_str::<Value>(s).map_err(|_| format!("{place}: the double-quoted value is not valid."));
    }
    if v.starts_with('\'') {
        let s = quoted(v, place)?;
        return Ok(Value::String(s[1..s.len() - 1].replace("''", "'")));
    }
    if v.starts_with('[') && v.ends_with(']') {
        if depth >= FRONT_LIST_DEPTH {
            return Err(format!("{place}: a list nests deeper than {FRONT_LIST_DEPTH} levels."));
        }
        return v[1..v.len() - 1]
            .split(',')
            .map(js_trim)
            .filter(|x| !x.is_empty())
            .map(|x| scalar_of(x, place, depth + 1))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array);
    }
    let chars: Vec<(usize, char)> = v.char_indices().collect();
    let cut = chars.windows(2).find(|w| js_space(w[0].1) && w[1].1 == '#').map(|w| w[0].0);
    Ok(Value::String(js_trim(cut.map_or(v, |c| &v[..c])).to_string()))
}

impl Client {
    /// Add an LLM skill from an Agent Skills `SKILL.md`.
    pub async fn import_skill_md(&self, user_id: impl Into<Option<&str>>, text: &str, opts: &SkillMdOptions) -> Result<SkillDefinition, WosError> {
        let md = parse_skill_md(text).map_err(refused)?;
        let name = opts.name.clone().unwrap_or_else(|| md.name.replace('-', "_"));
        if opts.name.is_none() && !is_skill_name(&name) {
            return Err(refused(format!(
                "SKILL.md: the name {} makes the skill name {}, which is not 1-32 of [a-z0-9_] starting with a \
                 letter: pass another as SkillMdOptions.name.",
                js_quote(&md.name),
                js_quote(&name)
            )));
        }
        let mut metadata = serde_json::Map::new();
        metadata.insert("source".into(), "SKILL.md".into());
        metadata.insert("skill_md_name".into(), md.name.clone().into());
        if !md.rest.is_empty() {
            metadata.insert("frontmatter".into(), serde_json::Value::Object(md.rest.clone()));
        }
        metadata.insert("not_run".into(), SKILL_MD_NOT_RUN.into());
        let def = SkillDefinition {
            name,
            description: md.description,
            kind: SkillKind::Llm,
            input: opts.input.clone().unwrap_or_else(skill_md_input),
            instructions: Some(md.body),
            max_tokens: opts.max_tokens,
            effort: opts.effort.clone(),
            model: opts.model.clone(),
            caller: opts.caller.clone(),
            confirm: opts.confirm,
            metadata: Some(metadata),
        };
        self.add_skill(def, user_id).await
    }
}

#[cfg(test)]
mod router_tests;
