use super::*;

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

/// One memory, as a call returns it.
#[derive(Debug, Deserialize)]
pub struct Memory {
    #[serde(default, deserialize_with = "null_to_default")]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "null_to_default")]
    pub content: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub category: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub similarity: f64,
    #[serde(default, deserialize_with = "null_to_default")]
    pub importance: f64,
    #[serde(default, deserialize_with = "null_to_default")]
    pub time_bucket: Option<String>,
    #[serde(default, deserialize_with = "null_to_default")]
    pub is_superseded: bool,
    #[serde(default, deserialize_with = "null_to_default")]
    pub superseded_by: Option<String>,
    #[serde(default, deserialize_with = "null_to_default")]
    pub created_at: Option<String>,
    #[serde(default, deserialize_with = "null_to_default")]
    pub event_date: Option<String>,
    #[serde(default, deserialize_with = "null_to_default")]
    pub speaker: Option<String>,
    /// The memory's time in the requested delivery form.
    #[serde(default, deserialize_with = "null_to_default")]
    pub time: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Memory {
    /// The writer's clock when it was stored, in whole minutes east of UTC; `None` when there is none or it is past 14 hours either side.
    pub fn tz_offset_minutes(&self) -> Option<i32> {
        let m = self.extra.get("tz_offset_minutes")?.as_f64()?;
        (m.fract() == 0.0 && m.abs() <= 840.0).then_some(m as i32)
    }
}

#[derive(Debug, Deserialize)]
#[non_exhaustive]
pub struct RecallResponse {
    #[serde(default)]
    pub short_term: serde_json::Value,
    #[serde(default)]
    pub long_term: serde_json::Value,
    #[serde(default)]
    pub context: serde_json::Value,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl RecallResponse {
    /// The recent conversation, oldest first.
    pub fn turns(&self) -> Vec<Turn> {
        records(&self.short_term, "turns")
            .into_iter()
            .filter_map(|t| serde_json::from_value(t).ok())
            .collect()
    }

    /// The long-term memories.
    pub fn memories(&self) -> Vec<Memory> {
        memories_from(self.long_term.get("memories"))
    }

    /// The conversation around the best memory, in order.
    pub fn context_rows(&self) -> Vec<String> {
        self.context
            .get("around_top_memory")
            .and_then(|v| v.as_array())
            .map(|rows| rows.iter().filter_map(|r| r.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }
}

/// One turn of the recent conversation.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Turn {
    #[serde(default, deserialize_with = "null_to_default")]
    pub role: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub content: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub timestamp: Option<String>,
    #[serde(default, deserialize_with = "null_to_default")]
    pub speaker: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Typed options for [`Client::search_opts`].
#[derive(Debug, Clone, Default)]
pub struct SearchOpts {
    /// Re-ask passes, 0-3.
    pub verify: Option<u8>,
    /// Image memories the answer may carry, 0-5 (default 1).
    pub max_images: Option<usize>,
}

/// Typed options for [`Client::recall_opts`] and [`Client::recall_rooms`].
#[derive(Debug, Clone, Default)]
pub struct RecallOpts {
    /// Long-term memories, 5-20 (default 10).
    pub limit: Option<usize>,
    /// Context around the best memory, 0-20 (default 10).
    pub context_limit: Option<usize>,
    pub room: Option<String>,
    /// Memories from the rooms, 1-20.
    pub room_limit: Option<usize>,
    /// Tokens of context around each memory from the rooms, 0-20000.
    pub room_expand_tokens: Option<u32>,
    pub long_term: Option<bool>,
    /// End long-term where today begins.
    pub long_term_cutoff: Option<bool>,
    pub today_start: Option<String>,
    pub now: Option<String>,
    /// Lines of the recent conversation, 0-500.
    pub short_term_limit: Option<usize>,
    /// Tokens of those lines, 0-8000; 0 sets no limit.
    pub short_term_budget_tokens: Option<u32>,
    /// Image memories, 0-5.
    pub max_images: Option<usize>,
    pub filters: Option<serde_json::Value>,
    pub speaker: Option<String>,
    pub include_superseded: Option<bool>,
}

/// Which rooms a look goes through: the last three days, each in three parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rooms {
    All,
    /// No rooms: long-term alone.
    None,
    Name(String),
    Names(Vec<String>),
}

impl Rooms {
    pub(crate) fn check(&self) -> Result<(), WosError> {
        let (ok, got) = match self {
            Rooms::All | Rooms::None => return Ok(()),
            Rooms::Name(n) => (is_room_name(n), serde_json::to_string(n).unwrap_or_default()),
            Rooms::Names(v) => (
                (1..=ROOMS_MAX).contains(&v.len()) && v.iter().all(|n| is_room_name(n)),
                serde_json::to_string(v).unwrap_or_default(),
            ),
        };
        if ok {
            return Ok(());
        }
        Err(refused(format!(
            "rooms must be \"all\" or room names like \"m0\" (this morning) or \"a1\" and \"n1\" \
             (yesterday afternoon and night), got {got}."
        )))
    }

    pub(crate) fn wire(&self) -> Option<serde_json::Value> {
        match self {
            Rooms::All => Some(serde_json::json!("all")),
            Rooms::None => Some(serde_json::json!(false)),
            Rooms::Name(n) => Some(serde_json::json!(n)),
            Rooms::Names(v) => Some(serde_json::json!(v)),
        }
    }
}

impl From<&str> for Rooms {
    fn from(s: &str) -> Self {
        if s == "all" { Rooms::All } else { Rooms::Name(s.to_string()) }
    }
}

impl From<String> for Rooms {
    fn from(s: String) -> Self {
        Rooms::from(s.as_str())
    }
}

impl<S: Into<String>> From<Vec<S>> for Rooms {
    fn from(v: Vec<S>) -> Self {
        Rooms::Names(v.into_iter().map(Into::into).collect())
    }
}

impl<const N: usize> From<[&str; N]> for Rooms {
    fn from(v: [&str; N]) -> Self {
        Rooms::Names(v.iter().map(|s| s.to_string()).collect())
    }
}

/// Per-call options for a write.
#[derive(Debug, Clone, Default)]
pub struct WriteOpts {
    /// The person's clock for this write, UTC offset in hours (default the client's).
    pub tz: Option<f64>,
    /// An open work this memory answers; the work then closes.
    pub work_id: Option<String>,
    /// Also put the line in the short-term window.
    pub window: Option<bool>,
    pub window_role: Option<String>,
    /// When the turn was said, RFC3339 with an offset.
    pub created_at: Option<String>,
    pub user_speaker: Option<String>,
    pub assistant_speaker: Option<String>,
}

/// Options for [`Client::around`]: offsets from the memory, which is 0.
#[derive(Debug, Clone, Default)]
pub struct AroundOpts {
    /// Default 3 before `center`.
    pub from: Option<i64>,
    /// Default 3 after `center`.
    pub to: Option<i64>,
    /// Default 0, the memory itself.
    pub center: Option<i64>,
    /// The `[first, last]` offsets you already have.
    pub held: Option<[i64; 2]>,
}

/// Options for [`Client::retrievals`].
#[derive(Debug, Clone, Default)]
pub struct RetrievalsOpts {
    /// Entries per page, 1-200 (default 50).
    pub limit: Option<usize>,
    pub before: Option<String>,
}

/// Where a working-memory step looks.
#[derive(Debug, Clone, Default)]
pub struct LookOpts {
    pub rooms: Option<Rooms>,
    /// The person's clock for this call, UTC offset in hours (default the client's).
    pub tz: Option<f64>,
    /// Photos each look may bring, 0-5 (default none).
    pub max_images: Option<usize>,
    /// Whether this work may bring the assistant's own lines (default on).
    pub own_words: Option<bool>,
    /// The diversity this work asks for, `"none"`, `"mmr"`, `"temporal"` or `"full"` (default unset: none is sent).
    pub diversity: Option<String>,
}

/// Options for [`Client::work_around`]: offsets from memory `[n]`.
#[derive(Debug, Clone, Default)]
pub struct WorkAroundOpts {
    /// Default 3 before `center`.
    pub from: Option<i64>,
    /// Default 3 after `center`.
    pub to: Option<i64>,
    /// Default 0, memory `[n]` itself.
    pub center: Option<i64>,
}

/// Options for [`Client::sleep_read`].
#[derive(Debug, Clone, Default)]
pub struct SleepReadOpts {
    pub tz: Option<f64>,
    /// 0 is today so far, 1 yesterday, up to 14.
    pub days_ago: Option<u32>,
    pub room: Option<String>,
    pub today_start: Option<String>,
    pub now: Option<String>,
    /// Tokens per page, 500-20000 (default 5000).
    pub budget_tokens: Option<u32>,
    pub cursor: Option<serde_json::Value>,
    pub include_superseded: Option<bool>,
    /// The `sleep_id` an earlier page answered with, to read on in the same sleep.
    pub sleep_id: Option<String>,
    pub since_last_sleep: Option<bool>,
    pub since: Option<String>,
}

/// One new version of one of the being's files, for [`Client::write_sleep_files`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SleepFileWrite {
    /// 0 is the core, 1 to 4 the boxes.
    pub slot: u32,
    /// The version it was read at.
    pub base_version: u64,
    pub title: String,
    pub body: String,
    pub evidence: Vec<String>,
    /// The day it is written on, "YYYY-MM-DD".
    pub day: Option<String>,
}

/// What [`Client::set_settings`] changes; only what is set is sent.
#[derive(Debug, Clone, Default)]
pub struct SettingsOpts {
    /// An IANA name such as `"Asia/Seoul"`.
    pub time_zone: Option<String>,
    pub default_speakers: Option<DefaultSpeakers>,
    /// Needed to change a time zone the store already has.
    pub confirmed_by_user: Option<bool>,
}

/// The store's default speakers.
/// Each side is `None` to leave it, `Some(None)` to clear it, or `Some(Some(name))` to set it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DefaultSpeakers {
    pub user: Option<Option<String>>,
    pub assistant: Option<Option<String>>,
}

/// Per-call options for [`Client::add_bulk_opts`].
#[derive(Debug, Clone, Default)]
pub struct BulkOpts {
    pub speaker: Option<String>,
    /// A conversation in parts, in place of `content`.
    pub segments: Option<Vec<Segment>>,
    pub timestamp: Option<String>,
}

/// One part of a conversation for [`Client::add_bulk_opts`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Segment {
    pub content: String,
    pub speaker: Option<String>,
    pub created_at: Option<String>,
}

#[derive(Debug)]
pub struct SelfSearch {
    pub memories: Vec<Memory>,
    /// The assistant's own words.
    pub self_memories: Vec<Memory>,
}

#[derive(Debug, Default)]
pub struct SearchFull {
    pub memories: Vec<Memory>,
    /// The assistant's own words.
    pub self_memories: Vec<Memory>,
    pub images: Vec<Memory>,
    pub verify_used: Option<u8>,
    pub raw: serde_json::Value,
}

#[cfg(test)]
mod tolerance_tests;
#[cfg(test)]
mod widening_tests;
