use super::*;

pub(crate) const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
pub(crate) const ANTHROPIC_VERSION: &str = "2023-06-01";
pub(crate) const ANTHROPIC_CACHE_MIN_TOKENS: u64 = 512;
pub(crate) const MODEL_RESPONSE_MAX_BYTES: usize = 32 * 1024 * 1024;

pub(crate) fn provider_http() -> HttpClient {
    let b = HttpClient::builder().connect_timeout(std::time::Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none());
    b.min_tls_version(reqwest::tls::Version::TLS_1_2).build().expect("wontopos: failed to build HTTP client")
}

pub(crate) fn check_model_key(raw: &str, what: &str) -> Result<String, WosError> {
    let key = js_trim(raw);
    if key.is_empty() {
        return Err(refused(format!("{what} is required: the model provider's key.")));
    }
    if key.chars().any(model_text_stray) {
        return Err(refused(format!("{what} contains whitespace or a control character: check for a stray newline.")));
    }
    if !key.is_ascii() {
        return Err(refused(format!("{what} contains a non-ASCII character: copy the key from a plain-text field.")));
    }
    if key.starts_with("wos-") {
        return Err(refused(format!("{what} starts with wos-, so it is a Wontopos key: a Wontopos key never goes to a model provider. Give the provider's own key.")));
    }
    Ok(key.to_string())
}

pub(crate) fn model_text_stray(c: char) -> bool {
    js_space(c) || matches!(c, '\0'..='\x1f' | '\x7f')
}

pub(crate) fn is_service_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    h == "wontopos.com" || h.ends_with(".wontopos.com")
}

pub(crate) fn host_key(host: &str) -> String {
    let dot = |c: char| if matches!(c, '\u{3002}' | '\u{ff0e}' | '\u{ff61}') { '.' } else { c };
    host.chars().map(dot).collect::<String>().to_lowercase().trim_end_matches('.').to_string()
}

pub(crate) fn check_model_name(m: &str, what: &str) -> Result<String, WosError> {
    let t = js_trim(m);
    if t.is_empty() || t.chars().any(model_text_stray) || m.encode_utf16().count() > 256 {
        return Err(refused(format!("{what} must be a model name, such as \"{ASK_MODEL}\".")));
    }
    Ok(t.to_string())
}

pub(crate) fn schema_text(v: &serde_json::Value) -> String {
    js_json(v)
}

/// How long Anthropic keeps a cached prompt prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheTtl {
    #[default]
    FiveMinutes,
    OneHour,
}

/// Options of [`anthropic_caller`].
#[derive(Clone, Default)]
pub struct AnthropicCallerOpts {
    pub api_key: String,
    /// The model it uses when the client names none.
    pub model: Option<String>,
    /// Default `https://api.anthropic.com`.
    pub base_url: Option<String>,
    pub cache_ttl: CacheTtl,
}

impl std::fmt::Debug for AnthropicCallerOpts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicCallerOpts")
            .field("api_key", &mask_key(&self.api_key))
            .field("model", &self.model)
            .field("base_url", &self.base_url.as_deref().map(redacted_base_url))
            .field("cache_ttl", &self.cache_ttl)
            .finish()
    }
}

/// The default caller: Anthropic's Messages API, from [`anthropic_caller`].
pub struct AnthropicCaller {
    pub(crate) key: String,
    pub(crate) base: String,
    pub(crate) model: Option<String>,
    pub(crate) ttl: CacheTtl,
    pub(crate) http: HttpClient,
}

impl std::fmt::Debug for AnthropicCaller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicCaller")
            .field("base_url", &self.base)
            .field("model", &self.model)
            .field("api_key", &mask_key(&self.key))
            .finish()
    }
}

/// The default caller: Anthropic's Messages API.
pub fn anthropic_caller(opts: AnthropicCallerOpts) -> Result<AnthropicCaller, WosError> {
    let key = check_model_key(&opts.api_key, "api_key")?;
    let base = provider_url(opts.base_url.as_deref().unwrap_or(ANTHROPIC_BASE_URL), None, true, GIVE_THE_ORIGIN)?;
    let model = opts.model.as_deref().map(|m| check_model_name(m, "anthropic_caller model")).transpose()?;
    Ok(AnthropicCaller { key, base, model, ttl: opts.cache_ttl, http: provider_http() })
}

impl ModelCaller for AnthropicCaller {
    fn call<'a>(&'a self, c: &'a ModelCall) -> BoxFuture<'a, Result<ModelReply, ModelCallError>> {
        Box::pin(async move {
            let url = format!("{}/v1/messages", self.base);
            let r = provider_post(&self.http, &url, &self.headers(), &anthropic_body(c, false, self.ttl), c.deadline, None).await?;
            Ok(anthropic_reply(&r))
        })
    }

    fn count_tokens<'a>(&'a self, c: &'a ModelCall) -> Option<BoxFuture<'a, Result<u64, ModelCallError>>> {
        Some(Box::pin(async move {
            let url = format!("{}/v1/messages/count_tokens", self.base);
            let r = provider_post(&self.http, &url, &self.headers(), &anthropic_body(c, true, self.ttl), c.deadline, None).await?;
            let n = r.get("input_tokens");
            n.and_then(serde_json::Value::as_u64)
                .or_else(|| n.and_then(serde_json::Value::as_f64).filter(|x| *x >= 0.0 && x.fract() == 0.0).map(|x| x as u64))
                .ok_or_else(|| ModelCallError::new(200, "count_tokens answered without input_tokens"))
        }))
    }

    fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    fn base_url(&self) -> Option<&str> {
        Some(&self.base)
    }
}

impl AnthropicCaller {
    fn headers(&self) -> Vec<(&'static str, String, bool)> {
        vec![("x-api-key", self.key.clone(), true), ("anthropic-version", ANTHROPIC_VERSION.to_string(), false)]
    }
}

async fn provider_post(
    http: &HttpClient, url: &str, headers: &[(&'static str, String, bool)], body: &serde_json::Value, deadline: std::time::Instant,
    reason_of: Option<fn(u16, &str) -> Option<&'static str>>,
) -> Result<serde_json::Map<String, serde_json::Value>, ModelCallError> {
    let left = deadline.saturating_duration_since(std::time::Instant::now());
    if left.is_zero() {
        return Err(ModelCallError::timeout("the call's time ran out before it was sent"));
    }
    let mut req = http.post(url).timeout(left).header(reqwest::header::CONTENT_TYPE, "application/json").body(schema_text(body).into_bytes());
    for (name, value, secret) in headers {
        let mut v =
            reqwest::header::HeaderValue::from_str(value).map_err(|_| ModelCallError::network(format!("the {name} header cannot be sent")))?;
        v.set_sensitive(*secret);
        req = req.header(*name, v);
    }
    let res = match req.send().await {
        Ok(r) => r,
        Err(e) if e.is_timeout() => return Err(ModelCallError::timeout("the provider did not answer in time")),
        Err(e) => return Err(ModelCallError::network(network_error(e).to_string())),
    };
    let status = res.status().as_u16();
    if (300..400).contains(&status) {
        return Err(ModelCallError::new(status, "redirect refused: the model key never follows a redirect"));
    }
    let head = res.headers().clone();
    let text = read_model_text(res).await?;
    if !(200..300).contains(&status) {
        let (message, request_id, kind, code) = provider_error(&text);
        let message = clean_text(&if message.is_empty() { format!("HTTP {status}") } else { message }, 1024);
        let kind = kind.or_else(|| code.clone());
        let header = |n: &str| head.get(n).and_then(|v| v.to_str().ok()).map(str::to_string);
        let request_id = request_id.or_else(|| header("request-id")).or_else(|| header("x-request-id"));
        let wait = header("retry-after-ms")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|ms| ms.is_finite() && *ms >= 0.0)
            .map(|ms| std::time::Duration::from_millis(ms.ceil() as u64))
            .or_else(|| header("retry-after").and_then(|v| retry_after_wait(&v)));
        let said = format!("{} {} {message}", kind.as_deref().unwrap_or(""), code.as_deref().unwrap_or(""));
        let reason = reason_of.and_then(|f| f(status, &said));
        let mut e = ModelCallError::new(status, message);
        e.error_type = kind.map(|t| clean_text(&t, 128));
        e.retry_after = wait.or((reason == Some("rate_limit")).then_some(std::time::Duration::from_secs(1)));
        e.request_id = request_id.map(|r| clean_text(&r, 256));
        e.reason = reason.map(str::to_string);
        return Err(e);
    }
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Object(o)) => Ok(o),
        Ok(_) => Err(ModelCallError::new(status, "the provider's answer is not a JSON object")),
        Err(_) => Err(ModelCallError::new(status, "the provider's answer is not JSON")),
    }
}

fn retry_after_wait(v: &str) -> Option<std::time::Duration> {
    let s = v.trim();
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        return Some(std::time::Duration::from_secs(s.parse::<u64>().unwrap_or(u64::MAX / 1000)));
    }
    let when = http_date(s)?;
    Some(when.duration_since(std::time::SystemTime::now()).unwrap_or_default())
}

fn provider_error(text: &str) -> (String, Option<String>, Option<String>, Option<String>) {
    let Ok(serde_json::Value::Object(data)) = serde_json::from_str::<serde_json::Value>(text) else {
        return (text.to_string(), None, None, None);
    };
    let s = |v: Option<&serde_json::Value>| v.and_then(|x| x.as_str()).map(str::to_string);
    match data.get("error") {
        Some(serde_json::Value::Object(err)) => {
            let kind = s(err.get("type"));
            let message = s(err.get("message")).or_else(|| kind.clone()).unwrap_or_else(|| text.to_string());
            (message, s(err.get("request_id")), kind, s(err.get("code")))
        }
        Some(serde_json::Value::String(e)) => (e.clone(), None, None, None),
        _ => (s(data.get("message")).unwrap_or_else(|| text.to_string()), None, None, None),
    }
}

async fn read_model_text(res: reqwest::Response) -> Result<String, ModelCallError> {
    let status = res.status().as_u16();
    if let Some(cl) = res.content_length() {
        if cl as usize > MODEL_RESPONSE_MAX_BYTES {
            return Err(ModelCallError::new(status, format!("the provider's answer is too large ({cl} bytes)")));
        }
    }
    use futures_util::StreamExt;
    let mut stream = res.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ModelCallError::network(network_error(e).to_string()))?;
        if buf.len() + chunk.len() > MODEL_RESPONSE_MAX_BYTES {
            return Err(ModelCallError::new(status, "the provider's answer is too large"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(buf.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&buf)).into_owned())
}

pub(crate) fn says_any(said: &str, words: &[&str]) -> bool {
    let s = said.to_lowercase();
    words.iter().any(|w| s.contains(w))
}

pub(crate) fn anthropic_body(c: &ModelCall, count: bool, ttl: CacheTtl) -> serde_json::Value {
    use serde_json::json;
    let parts: Vec<&String> = c.system.iter().filter(|t| !t.is_empty()).collect();
    let marked = !count && c.system_tokens.is_some_and(|n| n >= ANTHROPIC_CACHE_MIN_TOKENS);
    let system: Vec<serde_json::Value> = parts
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let mut b = json!({"type": "text", "text": text});
            if marked && i == parts.len() - 1 {
                b["cache_control"] = match ttl {
                    CacheTtl::OneHour => json!({"type": "ephemeral", "ttl": "1h"}),
                    CacheTtl::FiveMinutes => json!({"type": "ephemeral"}),
                };
            }
            b
        })
        .collect();
    let mut b = json!({"model": c.model, "system": system, "messages": [{"role": "user", "content": c.user}]});
    if !count {
        b["max_tokens"] = json!(c.max_tokens);
    }
    let mut oc = serde_json::Map::new();
    if let (Some(e), false) = (&c.effort, count) {
        oc.insert("effort".into(), json!(e));
    }
    if let Some(s) = &c.schema {
        oc.insert("format".into(), json!({"type": "json_schema", "schema": s}));
    }
    if !oc.is_empty() {
        b["output_config"] = serde_json::Value::Object(oc);
    }
    b
}

fn anthropic_reply(r: &serde_json::Map<String, serde_json::Value>) -> ModelReply {
    let blocks = r.get("content").and_then(|c| c.as_array()).into_iter().flatten();
    let text: String =
        blocks.filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text")).filter_map(|b| b.get("text").and_then(|t| t.as_str())).collect();
    let stop = match r.get("stop_reason") {
        None | Some(serde_json::Value::Null) => "end".to_string(),
        Some(serde_json::Value::String(s)) => match s.as_str() {
            "end_turn" | "stop_sequence" => "end".into(),
            "max_tokens" | "model_context_window_exceeded" => "max_tokens".into(),
            other => other.into(),
        },
        Some(other) => js_string(Some(other)),
    };
    ModelReply { text, stop, usage: r.get("usage").filter(|u| u.is_object()).cloned() }
}

/// How the router's JSON is asked of an OpenAI-compatible server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResponseFormat {
    #[default]
    JsonSchema,
    JsonObject,
    None,
}

impl ResponseFormat {
    const LADDER: [ResponseFormat; 3] = [ResponseFormat::JsonSchema, ResponseFormat::JsonObject, ResponseFormat::None];

    fn level(self) -> usize {
        Self::LADDER.iter().position(|f| *f == self).unwrap_or(0)
    }

    fn as_str(self) -> &'static str {
        match self {
            ResponseFormat::JsonSchema => "json_schema",
            ResponseFormat::JsonObject => "json_object",
            ResponseFormat::None => "none",
        }
    }
}

/// The name an OpenAI-compatible server gives the output budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxTokensField {
    MaxTokens,
    MaxCompletionTokens,
}

impl MaxTokensField {
    fn as_str(self) -> &'static str {
        match self {
            MaxTokensField::MaxTokens => "max_tokens",
            MaxTokensField::MaxCompletionTokens => "max_completion_tokens",
        }
    }
}

/// Options of [`openai_caller`].
#[derive(Clone, Default)]
pub struct OpenAiCallerOpts {
    /// The API's base URL; calls go to `{base_url}/chat/completions`.
    pub base_url: String,
    /// The model it uses when the client names none.
    pub model: String,
    /// Sent as `Authorization: Bearer`; leave it out for a server that takes none.
    pub api_key: Option<String>,
    /// How the router's JSON is asked for first (default `json_schema`).
    pub response_format: ResponseFormat,
    /// Default `max_completion_tokens` on api.openai.com, else `max_tokens`.
    pub max_tokens_field: Option<MaxTokensField>,
    /// Send a call's effort as `reasoning_effort` (default off).
    pub reasoning_effort: bool,
}

impl std::fmt::Debug for OpenAiCallerOpts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCallerOpts")
            .field("base_url", &redacted_base_url(&self.base_url))
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_deref().map(mask_key))
            .field("response_format", &self.response_format)
            .field("max_tokens_field", &self.max_tokens_field)
            .field("reasoning_effort", &self.reasoning_effort)
            .finish()
    }
}

/// A caller for any server that speaks OpenAI's chat completions, from [`openai_caller`].
pub struct OpenAiCaller {
    base: String,
    model: String,
    key: Option<String>,
    level: std::sync::atomic::AtomicUsize,
    pub(crate) max_field: MaxTokensField,
    effort: bool,
    http: HttpClient,
}

impl std::fmt::Debug for OpenAiCaller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCaller")
            .field("base_url", &self.base)
            .field("model", &self.model)
            .field("api_key", &self.key.as_deref().map_or_else(|| "none".to_string(), mask_key))
            .finish()
    }
}

/// A caller for any server that speaks OpenAI's chat completions.
pub fn openai_caller(opts: OpenAiCallerOpts) -> Result<OpenAiCaller, WosError> {
    let give = "the base URL without them, e.g. https://api.openai.com/v1";
    let base = provider_url(&opts.base_url, None, !matches!(opts.api_key.as_deref(), None | Some("")), give)?;
    let model = check_model_name(&opts.model, "openai_caller model")?;
    let key = opts.api_key.as_deref().filter(|k| !k.is_empty()).map(|k| check_model_key(k, "api_key")).transpose()?;
    let host = reqwest::Url::parse(&base).ok().and_then(|u| u.host_str().map(str::to_ascii_lowercase)).unwrap_or_default();
    let openai = host == "api.openai.com";
    let max_field = opts.max_tokens_field.unwrap_or(if openai { MaxTokensField::MaxCompletionTokens } else { MaxTokensField::MaxTokens });
    let level = std::sync::atomic::AtomicUsize::new(opts.response_format.level());
    Ok(OpenAiCaller { base, model, key, level, max_field, effort: opts.reasoning_effort, http: provider_http() })
}

const FORMAT_REFUSED: [u16; 3] = [400, 415, 422];

impl ModelCaller for OpenAiCaller {
    fn call<'a>(&'a self, c: &'a ModelCall) -> BoxFuture<'a, Result<ModelReply, ModelCallError>> {
        use std::sync::atomic::Ordering;
        Box::pin(async move {
            let url = format!("{}/chat/completions", self.base);
            let headers: Vec<(&'static str, String, bool)> =
                self.key.iter().map(|k| ("authorization", format!("Bearer {k}"), true)).collect();
            loop {
                let format = if c.schema.is_some() { ResponseFormat::LADDER[self.level.load(Ordering::Relaxed).min(2)] } else { ResponseFormat::None };
                let body = openai_body(c, format, self.max_field, self.effort);
                match provider_post(&self.http, &url, &headers, &body, c.deadline, Some(openai_reason)).await {
                    Ok(r) => return Ok(openai_reply(&r)),
                    Err(e) => {
                        let said = e.message.to_ascii_lowercase();
                        let refused = FORMAT_REFUSED.contains(&e.status)
                            && matches!(e.reason.as_deref(), None | Some("bad_request"))
                            && ["response_format", "json_schema", "json_object"].iter().any(|w| said.contains(w));
                        if c.schema.is_none() || format == ResponseFormat::None || !refused || std::time::Instant::now() >= c.deadline {
                            return Err(e);
                        }
                        let next = format.level() + 1;
                        self.level.fetch_max(next, Ordering::Relaxed);
                        let then = ResponseFormat::LADDER[next.min(2)].as_str();
                        log_debug!("openai_caller {}: response_format {} refused ({}); asking with {then}", self.base, format.as_str(), e.status);
                    }
                }
            }
        })
    }

    fn model(&self) -> Option<&str> {
        Some(&self.model)
    }

    fn base_url(&self) -> Option<&str> {
        Some(&self.base)
    }
}

pub(crate) const CONTEXT_WORDS: [&str; 5] =
    ["context_length", "context length", "maximum context", "too many tokens", "prompt is too long"];

fn openai_reason(status: u16, said: &str) -> Option<&'static str> {
    const CREDIT: [&str; 8] = [
        "insufficient_quota", "insufficient quota", "insufficient_balance", "insufficient balance", "arrearage",
        "exceeded your current quota", "credit", "billing",
    ];
    Some(if status == 401 || status == 403 {
        "auth"
    } else if status == 402 || says_any(said, &CREDIT) {
        "credit"
    } else if status == 404 {
        "bad_model"
    } else if status == 413 || says_any(said, &CONTEXT_WORDS) {
        "too_large"
    } else if status == 429 {
        "rate_limit"
    } else if status == 408 || status >= 500 {
        "overloaded"
    } else if matches!(status, 400 | 415 | 422) {
        "bad_request"
    } else {
        return None;
    })
}

pub(crate) fn openai_body(c: &ModelCall, format: ResponseFormat, max_field: MaxTokensField, effort: bool) -> serde_json::Value {
    use serde_json::json;
    let mut system = c.system.iter().filter(|t| !t.is_empty()).cloned().collect::<Vec<_>>().join("\n\n");
    if let (Some(schema), false) = (&c.schema, format == ResponseFormat::JsonSchema) {
        if !system.is_empty() {
            system.push_str("\n\n");
        }
        system.push_str(router_text::JSON_REPLY);
        system.push('\n');
        system.push_str(&schema_text(schema));
    }
    let mut messages = Vec::new();
    if !system.is_empty() {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": c.user}));
    let mut b = json!({"model": c.model, "messages": messages});
    b[max_field.as_str()] = json!(c.max_tokens);
    match (&c.schema, format) {
        (Some(schema), ResponseFormat::JsonSchema) => {
            b["response_format"] = json!({"type": "json_schema", "json_schema": {"name": "decision", "strict": true, "schema": schema}})
        }
        (Some(_), ResponseFormat::JsonObject) => b["response_format"] = json!({"type": "json_object"}),
        _ => {}
    }
    if let (true, Some(e)) = (effort, &c.effort) {
        b["reasoning_effort"] = json!(if e == "xhigh" || e == "max" { "high" } else { e.as_str() });
    }
    b
}

fn openai_reply(r: &serde_json::Map<String, serde_json::Value>) -> ModelReply {
    let null = serde_json::Value::Null;
    let choice = r.get("choices").and_then(|c| c.as_array()).and_then(|c| c.first()).unwrap_or(&null);
    let msg = choice.get("message").unwrap_or(&null);
    let text = match msg.get("content") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(parts)) => parts.iter().map(|p| p.get("text").and_then(|t| t.as_str()).unwrap_or("")).collect(),
        _ => String::new(),
    };
    let text = strip_think(&text);
    let refusal = msg.get("refusal").and_then(|v| v.as_str()).is_some_and(|s| !js_trim(s).is_empty());
    let stop = match choice.get("finish_reason").and_then(|f| f.as_str()) {
        _ if refusal => "refusal",
        Some("length") => "max_tokens",
        Some("content_filter") => "refusal",
        _ => "end",
    };
    ModelReply { text, stop: stop.into(), usage: r.get("usage").filter(|u| u.is_object()).cloned() }
}

fn strip_think(text: &str) -> String {
    let after = js_trim_start(text).strip_prefix("<think>").and_then(|a| a.find("</think>").map(|end| &a[end + "</think>".len()..]));
    after.map_or(text, js_trim_start).to_string()
}
