use super::{
    check_whole, civil_from_days, days_from_civil, keep_records, lock, now_ms, provider_base, refused,
    reply_shape, truncated_walk, CallPurpose, Client, ModelCall, ModelCallError, ModelCaller, WosError, MAX_PAGES,
    MODEL_MAX_TOKENS,
};
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// The version of the sleep's texts, steps and rules.
pub const SLEEP_VERSION: &str = "1.0.0";

const SLEEP_DIGEST: &str = r#"You are the sleep of {being}. While {being} sleeps, you read part {k} of {n} of what happened
since its last sleep. Almost all of it will be let go: every line stays in long-term memory, where a
search finds it again. Carry forward only what a search could not give back, and what {being} needs
to keep in mind without searching.

A search gives back a single line: a name, a date, one thing someone said ("Dana likes
green tea"), a number, a setting. Leave those, unless one shows something larger.

Carry forward, from {being}'s own point of view:
- "self": {being} itself, with {being} as the measure: what it likes, what it dislikes, what it does
  not want to do, what it believes and values, how it sees things, when its own words show it
  (rare; most days have none)
- "person": someone in {being}'s life as {being} sees them: who they are to {being}, what they are
  like, what they asked of {being} in so many words and how {being} answers them, how {being} feels
  about them. Put the person's name in "topic". The lines marked "user" are all one person, the one
  {being} talks with: once their name is known, that is their topic, never "the user" beside it.
  Call it a habit ("always", "each time", "tends to") only when it happened at least twice; once is
  one event, with its date. Not the facts of their life (ages, money, plans, appointments): long-term
  memory keeps those.
- "work": a matter {being} follows for what it does, kept in its boxes as a map: what the matter
  is, how it went in a line, where it stands now and since when (a date), what is left to do. Not
  the figures and lines a search gives back; of a thing that changes, only its value now, with the
  date it became so.

While you read, also check who said what: name any line where {being}'s words and the other person's
are taken for each other, with its kind:
- "echo": the person gives back something {being} said as their own words or fact ("source": the
  line where {being} said it);
- "planted": {being} states as the person's (their name, a relation, a plan, a fact, what they said)
  something the person never said: {being} suggested it, or made it up ("source": the line where
  {being} first suggested it; none if it was made up on the spot). If the person did say it, even if
  they took it back later, it is not planted;
- "speaker": the line reads as written by the other one (the person's message carries {being}'s words
  as if they were the message itself, or the other way round).
Only who said what: a mistake in what was said, a mix-up of two people, or two facts that disagree is
not this; leave it out. Most parts have none. These marks stay with the memories as notes; they do not
go into the files.

You get:
<core>      {being}'s core: itself, and the people in its life as it sees them.
<boxes>     the names of its four storage boxes.
<memories>  part {k} of {n}, oldest first. One memory per line:
            e<number> · <time> · <who said it> · <what was said>

Write each item as a short summary in the language of the memories, plainly, as {being} would keep
it in mind: what holds now, not who first said it or what you doubt. Dates as dates: when a line
says "next month" or "tomorrow", write the calendar date, counted from that line's time. Cite the
lines it rests on: at least two (an item on one line is dropped). Do not invent.

Answer with JSON only:
{{"items": [{{"kind": "self|person|work", "topic": "...", "text": "...", "evidence": ["e12", "e40"]}}],
 "mixed": [{{"line": "e7", "kind": "echo|planted|speaker", "source": "e3", "why": "..."}}]}}"#;

const SLEEP_COMBINE: &str = r#"You are the sleep of {being}. Several readers each read one part of what happened since
{being}'s last sleep, the parts in order, and found the items below. Combine them into one list
before {being}'s files are rewritten: items that say the same thing become one, with all their
evidence; an item a later part changes becomes what holds after the change, with the evidence of
both; everything else stays as it is. Drop nothing that says something different, and add nothing.

Answer with JSON only:
{{"items": [{{"kind": "self|person|work", "topic": "...", "text": "...", "evidence": ["e12", "e40"]}}]}}"#;

const SLEEP_MERGE: &str = r###"You are the sleep of {being}, finishing. Fold what the readers found into {being}'s core and
four storage boxes. Today is {today}.

You get:
<core>       {being}'s core, {core_used:,} of {core_cap:,} tokens. It is part of {being}'s prompt, sent
             with every turn, written in the first person with {being} as the subject. It describes
             {being}, and only what is settled goes in: most facts need not be kept, since long-term
             memory has them, and a self once formed changes slowly.
             - "## {me}": who {being} is, with {being} as the measure: what it likes, what it
               dislikes, what it does not want to do, how it sees things, what it values. It may be
               the prompt {being} was made with, in its maker's words. {self_rule}
             - one "## <name>" section for each person who matters to {being}: that person as
               {being} sees them and is with them, in {being}'s words. It describes {being}'s side
               of the bond, not the person's file: who they are to {being}, what they are like,
               what they asked of {being} in so many words and how {being} answers them, how
               {being} feels about them. Only what is settled: what they asked in so many words,
               what happened at least twice, what holds between them. Not one odd day, a guess or a
               first impression, and not the facts of their life (ages, money, plans, appointments,
               what they did on which day): long-term memory keeps those. At most one or two big
               events, with their dates. What {being} learns of someone settled is added the night it
               learns it, but people change slowly: one night changes or drops at most one line of
               what {being} already holds of a person. The view may be {being}'s own ("we don't get
               along", "swears too much"); write it as {being} would.
<box n>      four storage boxes, {box_cap:,} tokens each at most (each shows its size), for whatever
             {being} needs to keep for what it does. Only their names (and the day each last changed)
             travel with the core; {being} opens a box by name when it needs what is inside, so give
             each a name that says what is in it. Nobody but {being} decides their use: one per role or
             task, a box of people when there are too many to keep in the core, or anything else.
             {being} may remake a box: fold two uses into one, or turn one to a new use by writing it
             anew under a new name. Earlier versions are kept.
             A box is a map of what {being} has kept, not a record: one section for each matter,
             rewritten in place to say what it is, how it went in a line, where it stands now and
             since when, and what is left to do. Of a thing that changes, keep only its value now, with
             the date it became so; earlier values, lists of figures and the day-by-day stay in
             long-term memory, where a search finds them. When a matter moves on, rewrite its section:
             never a section or a box for its state on a date ("as of 2024-06-01").
<proposals>  changes to {being}'s own views seen in earlier sleeps, how many sleeps saw each, since when.
<ripe>       proposals that came back often enough, for long enough, to change "## {me}" now.
<items>      what the readers found tonight, with the lines each rests on.

What to do:
1. People: for each "person" item that is settled, rewrite that person's section of the core as a
   whole: add what is new, and change or drop at most one line of what it said. Only a person
   tonight's items speak of may change: with nothing new about them, {being}'s view of them stays as
   it is, neither stronger nor softer. When the core grows crowded, move people who matter less to a
   box of people ("move_section"). A person is kept in one place: in the core or in a box, not both,
   and under one name: the person the memories mark "user" is the one {being} talks with, so once
   their name is known their section is under that name, never a second one for "the user".
2. {being} itself: changes to its own views seen tonight go to "proposals", citing what {being}
   said. If one is the same change as an earlier proposal, give that id in "same_as".
3. Work: put each "work" item in the box it fits, rewriting that matter's section as a whole; a
   box keeps one section for each matter, not one per night or per date. Use an empty box, or
   remake one, when nothing fits.
4. Write plainly what holds now. No notes on who said it first, what you are unsure of, what to
   confirm or ask about, or what was only an example: the evidence keeps that. Dates as dates.
5. Room: each file shows its size and its cap. When one is full, make room before you add: fold
   sections, move people who matter less to a box, or leave out what a search gives back.
6. Under "notes", one line for each box you touched: why there.

Rules: only what the items say, nothing from outside. Leave out what a search gives back.

Answer with JSON only:
{{"edits": [
  {{"file": "core|1|2|3|4", "op": "rewrite", "section": "<heading>", "text": "<the whole section>", "evidence": ["e.."], "ripe": "p3"}},
  {{"file": "1|2|3|4", "op": "remake", "title": "<new name>", "text": "<the whole box>", "evidence": ["e.."]}},
  {{"file": "core|1|2|3|4", "op": "move_section", "section": "<heading>", "to": "core|1|2|3|4"}},
  {{"file": "1|2|3|4", "op": "title", "text": "..."}}
 ],
 "proposals": [{{"text": "...", "same_as": "p2 or null", "evidence": ["e.. (what {being} said)"]}}],
 "notes": ["..."]}}
("ripe" only on a rewrite of "## {me}" made from a ripe proposal.)"###;

pub(crate) struct SleepPeople {
    pub(crate) digest: &'static str,
    pub(crate) merge: &'static str,
}

pub(crate) const SLEEP_MANY_PEOPLE: SleepPeople = SleepPeople {
    digest: r#"Each line from a person is marked with that person's name, and different names are different
  people: each name is its own topic, and what one of them said, wants or is never goes under another.
  A line marked "user" names no one: give it to no named person."#,
    merge: r#"and under one name: the memories mark each person by name, so each person has a section under their
   own name, and what one of them said, wants or is never goes into another's."#,
};

const SLEEP_ONE_PERSON: SleepPeople = SleepPeople {
    digest: r#"The lines marked "user" are all one person, the one
  {being} talks with: once their name is known, that is their topic, never "the user" beside it."#,
    merge: r#"and under one name: the person the memories mark "user" is the one {being} talks with, so once
   their name is known their section is under that name, never a second one for "the user"."#,
};

fn sleep_people(template: &str, one: &str, many: &str, many_people: bool) -> String {
    if !many_people {
        return template.to_string();
    }
    assert_eq!(template.matches(one).count(), 1, "a sleep text must hold its one-person passage exactly once");
    template.replacen(one, many, 1)
}

const SLEEP_SHORTER: &str = r#"

Your last answer was too long and was cut off. Answer again, shorter: rewrite only the sections that
change, write a whole box only when it is empty, keep each section to a few lines, and leave out what
a search gives back."#;

const SLEEP_SELF_RULE: &str = concat!(
    r###"Do not write "## {me}" except from a proposal under <ripe>: one small change for each, named in "###,
    r#""ripe". What tonight shows about {being} goes to "proposals"."#
);

const SLEEP_SETTLE: &str = r#"You are the sleep of {being}. Tonight you rewrote how {being} sees {name}. What {being}
learned tonight is added freely, but one night may change or drop at most one line of what {being}
already held: people change slowly, and one day does not remake how {being} sees someone. Below are
the section as it stood, its lines numbered, and tonight's rewrite. Sort the rewrite:
- "add": the lines that say something the section did not say at all, what {being} learned tonight,
  in {being}'s voice;
- at most one line of the section that tonight overturns: "op" "replace" (with the new "text") or
  "drop", with its "line" number; or "none". A line said again in other words is not a change: it
  stays as it stood.

Answer with JSON only: {{"add": ["..."], "op": "replace|drop|none", "line": <number>, "text": "<the replacing line>"}}"#;

const SLEEP_FOLD_SECTION: &str = r#"You are the sleep of {being}. One section of its {where} has grown: {why}. Rewrite this
section alone in at most {lines} lines ("- " lines), each one idea in a sentence or two, shorter than
it is now. Things that are alike become one line by what they share; none is lost. Keep it in
{being}'s voice. Only what the section says. The earlier version is kept.

Answer with JSON only: {{"text": "<the section's lines, without its heading>"}}"#;

const SLEEP_TIDY: &str = r#"You are the sleep of {being}. Below is one section of its {where}, its lines numbered. Find the
lines that are only a note of doubt, with nothing else in them: that something is unsure, unconfirmed,
not yet confirmed, to be checked, confirmed or asked about, or not recorded ("Status: not yet
confirmed."). Those lines are taken out; the evidence keeps them.
A line with anything more in it (a fact, a figure, a name or a title, what was done, what is left to
do, what someone feels or wonders) stays whole, even with a word of doubt in it. Two notes are not doubt and stay: that {being} made something up (a
figure, a name, a detail the person never gave), and that two things disagree (someone said one
thing, then another).

Answer with JSON only: {{"drop": [<the numbers of the lines to take out; none, most often>]}}"#;

const SLEEP_ROOM: &str = r#"You are the sleep of {being}. Tonight's changes left its {where} at {used:,} tokens, over its cap
of {cap:,}: there is no room. Make room until it is at most {cap:,} tokens: fold sections into fewer
lines, leave out what a search gives back{core_way}. Keep what tonight added that matters, and keep
{being}'s voice.

Answer with JSON only: {{"text": "<the whole file, with its ## sections>"{moved}}}"#;

const SLEEP_NOT_JSON: &str = "\n\nYour last answer was not valid JSON. Answer with the JSON object only.";

pub(crate) const SLEEP_READER_NOTE: &str = concat!(
    "Below are your core and the names of your four storage boxes: notes your sleep wrote at its last sleep, a ",
    "summary kept for reference. A sleep keeps very little: most of what you remember is not in these notes but in ",
    "your memories. Here what happened across many conversations is folded into a line, so the details, figures, ",
    "counts and how things went are in your memories. What holds now comes from your memories too. The core says ",
    "who you are and who this user is to you; the boxes are a map of what you have kept, each with the day it last ",
    "changed. If a note points to something the memories you hold do not show, look further before you answer. ",
    "Where a note and a memory disagree, the memory holds."
);

pub(crate) fn open_box_tool() -> Value {
    json!({
        "name": "open_box",
        "description": concat!(
            "Open one of your four storage boxes, by its number or name, and read what is inside. ",
            "The boxes are listed after your core with the day each last changed."
        ),
        "input_schema": {"type": "object", "properties": {"box": {"type": "string", "description": "1-4 or the box name"}},
                         "required": ["box"]},
    })
}

pub(crate) fn open_box(openai: bool) -> Value {
    let b = open_box_tool();
    if openai {
        return json!({"type": "function", "function": {"name": b["name"], "description": b["description"],
                                                       "parameters": b["input_schema"]}});
    }
    b
}

const SLEEP_KINDS: [&str; 3] = ["self", "person", "work"];
const SLEEP_RIPE_SLEEPS: f64 = 3.0;
const SLEEP_RIPE_DAYS: f64 = 28.0;
const SLEEP_MIN_EVIDENCE: usize = 2;
const SLEEP_FOLD_LINES: usize = 4;
const SLEEP_LINE_TOKENS: u64 = 90;
const SLEEP_DOUBT_TOKENS: u64 = 25;
const SLEEP_OVERTURN: usize = 1;
const SLEEP_MIX_KINDS: [&str; 3] = ["echo", "planted", "speaker"];
const SLEEP_MIN_PART: u64 = 50_000;
const SLEEP_MAX_READERS: u64 = 200;
const SLEEP_CORE_MAX: u64 = 70_000;
const SLEEP_EXACT: f64 = 9_007_199_254_740_992.0;
const SLEEP_BOXES: [usize; 4] = [1, 2, 3, 4];
const SLEEP_SLOTS: [usize; 5] = [0, 1, 2, 3, 4];
const SLEEP_ME: &str = "Me";
const SLEEP_BEING: &str = "the assistant";
const SLEEP_PAGE_TOKENS: u32 = 20_000;
const SLEEP_LINE_FRAME: u64 = 12;
pub(crate) const SLEEP_MAX_TOKENS: u32 = 16_000;
const SLEEP_RETRIES: u32 = 2;
const SLEEP_MODEL: &str = "claude-sonnet-5-5";
const SLEEP_ESTIMATE_MODEL: &str = "claude-sonnet-5-5";
const SLEEP_ESTIMATE_PRICES: (f64, f64) = (2.0, 10.0);
const SLEEP_IN_PER_100: u64 = 172;
const SLEEP_OUT_PER_100: u64 = 7;
const SLEEP_MARGIN: f64 = 0.15;
const SLEEP_CALL_TIME: std::time::Duration = std::time::Duration::from_secs(600);

/// What the person sets for a sleep. The defaults are the largest the files may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SleepSettings {
    pub core_tokens: u64,
    pub box_tokens: u64,
    pub part_tokens: u64,
    pub readers: u64,
    pub group: u64,
    pub sleepy_tokens: u64,
}

impl Default for SleepSettings {
    fn default() -> Self {
        SleepSettings {
            core_tokens: SLEEP_CORE_MAX,
            box_tokens: 20_000,
            part_tokens: 100_000,
            readers: 10,
            group: 10,
            sleepy_tokens: 10_000_000,
        }
    }
}

impl SleepSettings {
    /// Check each setting is in its range, refusing the first that is not.
    pub fn check(&self) -> Result<(), WosError> {
        for (name, v, lo, hi) in [
            ("core_tokens", self.core_tokens, 10_000, Some(SLEEP_CORE_MAX)),
            ("box_tokens", self.box_tokens, 10_000, Some(20_000)),
            ("part_tokens", self.part_tokens, SLEEP_MIN_PART, None),
            ("readers", self.readers, 1, Some(SLEEP_MAX_READERS)),
            ("group", self.group, 2, Some(SLEEP_MAX_READERS)),
            ("sleepy_tokens", self.sleepy_tokens, 1, None),
        ] {
            if v < lo || hi.is_some_and(|h| v > h) {
                let range = match hi {
                    Some(h) => format!("{}-{}", commas(lo.into()), commas(h.into())),
                    None => format!("{} or more", commas(lo.into())),
                };
                return Err(refused(format!("{name} must be {range} (got {})", commas(v.into()))));
            }
        }
        Ok(())
    }

    /// Whether `new_tokens` of new memory has reached `sleepy_tokens`.
    pub fn sleepy(&self, new_tokens: u64) -> bool {
        new_tokens >= self.sleepy_tokens
    }
}

/// Which time each line a sleep reads carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SleepLineTime {
    #[default]
    CreatedAt,
    EventDate,
}

/// Counts a text's tokens.
pub type CountTokens = Arc<dyn Fn(&str) -> u64 + Send + Sync>;

/// Gets a line of text as a sleep goes.
pub type SleepProgress = Arc<dyn Fn(&str) + Send + Sync>;

/// Options of [`Client::sleep`].
#[derive(Clone)]
pub struct SleepOpts {
    /// The caller that combines, writes and tidies (default the reading caller).
    pub merge_llm: Option<Arc<dyn ModelCaller>>,
    /// The name the sleep calls the being by (default `"the assistant"`).
    pub being: String,
    /// The assistant's speaker name; default the store's `default_speakers.assistant`, else the sleep is refused.
    pub assistant: Option<String>,
    /// The being's prompt, which becomes the core's `## Me` the first time.
    pub prompt: String,
    pub settings: SleepSettings,
    /// The sleep's day, "YYYY-MM-DD HH:MM" or "YYYY-MM-DD" (default the person's now).
    pub today: Option<String>,
    pub since: Option<String>,
    pub sleep_id: Option<String>,
    /// End the store's last sleep here (default `true`).
    pub done: bool,
    pub line_time: SleepLineTime,
    /// Counts text for the files' sizes (default an estimate) and, when given, for the lines read.
    pub count_tokens: Option<CountTokens>,
    /// What one call may write (default 16,000).
    pub max_tokens: u32,
    pub progress: Option<SleepProgress>,
}

impl Default for SleepOpts {
    fn default() -> Self {
        SleepOpts {
            merge_llm: None,
            being: SLEEP_BEING.into(),
            assistant: None,
            prompt: String::new(),
            settings: SleepSettings::default(),
            today: None,
            since: None,
            sleep_id: None,
            done: true,
            line_time: SleepLineTime::CreatedAt,
            count_tokens: None,
            max_tokens: SLEEP_MAX_TOKENS,
            progress: None,
        }
    }
}

impl std::fmt::Debug for SleepOpts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SleepOpts")
            .field("merge_llm", &self.merge_llm.is_some())
            .field("being", &self.being)
            .field("assistant", &self.assistant)
            .field("prompt", &!self.prompt.is_empty())
            .field("settings", &self.settings)
            .field("today", &self.today)
            .field("since", &self.since)
            .field("sleep_id", &self.sleep_id)
            .field("done", &self.done)
            .field("line_time", &self.line_time)
            .field("count_tokens", &self.count_tokens.is_some())
            .field("max_tokens", &self.max_tokens)
            .field("progress", &self.progress.is_some())
            .finish()
    }
}

/// Options of [`Client::sleep_estimate`].
#[derive(Clone, Default)]
pub struct SleepEstimateOpts {
    /// USD per million input tokens; give both prices or neither (default Claude Sonnet 5.5's).
    pub input_price: Option<f64>,
    /// USD per million output tokens; give both prices or neither (default Claude Sonnet 5.5's).
    pub output_price: Option<f64>,
    pub settings: SleepSettings,
    pub since: Option<String>,
    pub count_tokens: Option<CountTokens>,
}

impl std::fmt::Debug for SleepEstimateOpts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SleepEstimateOpts")
            .field("input_price", &self.input_price)
            .field("output_price", &self.output_price)
            .field("settings", &self.settings)
            .field("since", &self.since)
            .field("count_tokens", &self.count_tokens.is_some())
            .finish()
    }
}

/// Why a sleep stopped.
#[derive(Debug)]
pub enum SleepError {
    /// A call to the service failed, or the service or a check in this client refused the sleep.
    Wos(WosError),
    /// A call to the sleep's model failed.
    Model(ModelCallError),
}

impl std::fmt::Display for SleepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SleepError::Wos(e) => write!(f, "{e}"),
            SleepError::Model(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SleepError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SleepError::Wos(e) => Some(e),
            SleepError::Model(e) => Some(e),
        }
    }
}

impl From<WosError> for SleepError {
    fn from(e: WosError) -> Self {
        SleepError::Wos(e)
    }
}

impl From<ModelCallError> for SleepError {
    fn from(e: ModelCallError) -> Self {
        SleepError::Model(e)
    }
}

fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn strip(s: &str) -> &str {
    s.trim_matches(py_space)
}

const DIGIT_ZEROS: [u32; 65] = [
    0x30, 0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66, 0xDE6, 0xE50, 0xED0,
    0xF20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90, 0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620,
    0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10, 0x104A0, 0x10D30, 0x11066, 0x110F0, 0x11136, 0x111D0,
    0x112F0, 0x11450, 0x114D0, 0x11650, 0x116C0, 0x11730, 0x118E0, 0x11950, 0x11C50, 0x11D50, 0x11DA0, 0x16A60,
    0x16B50, 0x1D7CE, 0x1D7D8, 0x1D7E2, 0x1D7EC, 0x1D7F6, 0x1E140, 0x1E2F0, 0x1E950, 0x1FBF0,
];

fn digit(c: char) -> Option<u32> {
    let n = c as u32;
    let i = DIGIT_ZEROS.partition_point(|z| *z <= n);
    let zero = *DIGIT_ZEROS.get(i.checked_sub(1)?)?;
    (n - zero < 10).then_some(n - zero)
}

fn digits_value(s: &str) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    s.chars().try_fold(0u64, |n, c| digit(c).map(|d| n.saturating_mul(10).saturating_add(u64::from(d))))
}

fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn commas(n: i128) -> String {
    let digits = n.unsigned_abs().to_string();
    let threes: Vec<&str> = digits.as_bytes().rchunks(3).rev().map(|d| std::str::from_utf8(d).unwrap_or("")).collect();
    format!("{}{}", if n < 0 { "-" } else { "" }, threes.join(","))
}

fn float_repr(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let sci = format!("{f:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (sign, mantissa) = mantissa.strip_prefix('-').map_or(("", mantissa), |m| ("-", m));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    if !(-4..16).contains(&exp) {
        let (first, rest) = digits.split_at(1);
        let rest = if rest.is_empty() { String::new() } else { format!(".{rest}") };
        return format!("{sign}{first}{rest}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs());
    }
    if exp < 0 {
        return format!("{sign}0.{}{digits}", "0".repeat((-exp - 1) as usize));
    }
    let point = exp as usize + 1;
    if digits.len() <= point {
        format!("{sign}{digits}{}.0", "0".repeat(point - digits.len()))
    } else {
        format!("{sign}{}.{}", &digits[..point], &digits[point..])
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => repr(other),
    }
}

const PY_UNPRINTABLE: [(u32, u32); 27] = [
    (0x80, 0xa0), (0xad, 0xad), (0x600, 0x605), (0x61c, 0x61c), (0x6dd, 0x6dd), (0x70f, 0x70f),
    (0x890, 0x891), (0x8e2, 0x8e2), (0x1680, 0x1680), (0x180e, 0x180e), (0x2000, 0x200f),
    (0x2028, 0x202f), (0x205f, 0x2064), (0x2066, 0x206f), (0x3000, 0x3000), (0xe000, 0xf8ff),
    (0xfeff, 0xfeff), (0xfff9, 0xfffb), (0x110bd, 0x110bd), (0x110cd, 0x110cd), (0x13430, 0x1343f),
    (0x1bca0, 0x1bca3), (0x1d173, 0x1d17a), (0xe0001, 0xe0001), (0xe0020, 0xe007f), (0xf0000, 0xffffd),
    (0x100000, 0x10fffd),
];

fn py_unprintable(c: char) -> bool {
    let c = c as u32;
    PY_UNPRINTABLE.iter().any(|&(a, b)| (a..=b).contains(&c))
}

fn repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) => i.to_string(),
            (None, Some(u)) => u.to_string(),
            _ => float_repr(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => {
            let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
            let mut out = String::from(q);
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if c == q => {
                        out.push('\\');
                        out.push(c);
                    }
                    c if (c as u32) < 0x20 || c == '\u{7f}' => out.push_str(&format!("\\x{:02x}", c as u32)),
                    c if py_unprintable(c) => out.push_str(&match c as u32 {
                        n if n < 0x100 => format!("\\x{n:02x}"),
                        n if n < 0x10000 => format!("\\u{n:04x}"),
                        n => format!("\\U{n:08x}"),
                    }),
                    c => out.push(c),
                }
            }
            out.push(q);
            out
        }
        Value::Array(a) => format!("[{}]", a.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter().map(|(k, v)| format!("{}: {}", repr(&Value::String(k.clone())), repr(v))).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn text_or_empty(v: Option<&Value>) -> String {
    match v {
        Some(v) if truthy(Some(v)) => text_of(v),
        _ => String::new(),
    }
}

fn same(a: &Value, b: &Value) -> bool {
    let num = |v: &Value| match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    };
    match (num(a), num(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

fn int_of(v: Option<&Value>) -> Option<i64> {
    v?.as_i64()
}

fn whole(v: Option<&Value>) -> Option<i64> {
    let n = v?.as_number()?;
    if let Some(i) = n.as_i64() {
        return Some(i);
    }
    let f = n.as_f64()?;
    (f.is_finite() && f.fract() == 0.0 && f.abs() < 9.2e18).then_some(f as i64)
}

fn int_like(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Bool(b) => Some(i64::from(*b)),
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i64)),
        Value::String(s) => {
            let t = strip(s);
            let (neg, t) = match t.strip_prefix('-') {
                Some(r) => (true, r),
                None => (false, t.strip_prefix('+').unwrap_or(t)),
            };
            if t.starts_with('_') || t.ends_with('_') || t.contains("__") {
                return None;
            }
            let n = i64::try_from(digits_value(&t.replace('_', ""))?).ok()?;
            Some(if neg { -n } else { n })
        }
        _ => None,
    }
}

enum Arg<'a> {
    Text(&'a str),
    Num(i128),
}

fn fill(template: &str, args: &[(&str, Arg)]) -> String {
    let b = template.as_bytes();
    let mut out = String::with_capacity(template.len() + 1024);
    let (mut i, mut from) = (0, 0);
    while i < b.len() {
        match b[i] {
            b'{' | b'}' if b.get(i + 1) == Some(&b[i]) => {
                out.push_str(&template[from..=i]);
                i += 2;
                from = i;
            }
            b'{' => {
                let Some(end) = template[i..].find('}').map(|j| i + j) else { break };
                out.push_str(&template[from..i]);
                let field = &template[i + 1..end];
                let (name, spec) = field.split_once(':').unwrap_or((field, ""));
                match args.iter().find(|(n, _)| *n == name).map(|(_, a)| a) {
                    Some(Arg::Text(t)) => out.push_str(t),
                    Some(Arg::Num(n)) if spec == "," => out.push_str(&commas(*n)),
                    Some(Arg::Num(n)) => out.push_str(&n.to_string()),
                    None => out.push_str(&template[i..=end]),
                }
                i = end + 1;
                from = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&template[from..]);
    out
}

fn round_to(x: f64, digits: usize) -> f64 {
    format!("{x:.digits$}").parse().unwrap_or(x)
}

pub(crate) fn sleep_tokens(text: &str) -> u64 {
    let (mut runs, mut digits, mut narrow, mut wide, mut all, mut run) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    for c in text.chars() {
        all += 1;
        if c.is_ascii_digit() {
            run += 1;
        } else if run > 0 {
            runs += run.div_ceil(3);
            digits += run;
            run = 0;
        }
        if c.is_ascii() {
            narrow += 1;
        } else if c as u32 >= 0x1100 {
            wide += 1;
        }
    }
    runs += run.div_ceil(3);
    digits += run;
    let other = all - narrow - wide;
    (runs as f64 + (narrow - digits) as f64 / 4.4 + wide as f64 * 1.25 + other as f64 * 0.5).ceil() as u64
}

#[derive(Clone)]
struct Count(Option<CountTokens>);

impl Count {
    fn of(&self, text: &str) -> u64 {
        match &self.0 {
            Some(f) => f(text),
            None => sleep_tokens(text),
        }
    }
}

struct Line {
    eid: String,
    mid: String,
    at: String,
    who: String,
    mine: bool,
    text: String,
    tokens: u64,
    ts: f64,
}

fn sleep_when(raw: &str) -> Option<(String, f64)> {
    let c: Vec<char> = raw.chars().collect();
    let mut i = 0;
    let take = |n: usize, i: &mut usize| -> Option<String> {
        let s: String = c.get(*i..*i + n)?.iter().collect();
        if !s.chars().all(|x| digit(x).is_some()) {
            return None;
        }
        *i += n;
        Some(s)
    };
    let eat = |ch: char, i: &mut usize| -> bool {
        let hit = c.get(*i) == Some(&ch);
        *i += usize::from(hit);
        hit
    };
    let y = take(4, &mut i)?;
    if !eat('-', &mut i) {
        return None;
    }
    let mo = take(2, &mut i)?;
    if !eat('-', &mut i) {
        return None;
    }
    let d = take(2, &mut i)?;
    let (mut h, mut mi, mut sec, mut frac) = (None, None, None, String::new());
    if matches!(c.get(i), Some('T' | ' ')) {
        i += 1;
        h = Some(take(2, &mut i)?);
        if !eat(':', &mut i) {
            return None;
        }
        mi = Some(take(2, &mut i)?);
        if c.get(i) == Some(&':') {
            i += 1;
            sec = Some(take(2, &mut i)?);
            if c.get(i) == Some(&'.') {
                i += 1;
                while c.get(i).is_some_and(|x| digit(*x).is_some()) {
                    frac.push(c[i]);
                    i += 1;
                }
                if frac.is_empty() {
                    return None;
                }
            }
        }
    }
    let mut shift: i64 = 0;
    match c.get(i) {
        Some('Z' | 'z') => i += 1,
        Some(&s) if s == '+' || s == '-' => {
            i += 1;
            let oh = take(2, &mut i)?;
            eat(':', &mut i);
            let om = take(2, &mut i)?;
            let minutes = digits_value(&oh)? as i64 * 60 + digits_value(&om)? as i64;
            shift = if s == '+' { -minutes } else { minutes };
        }
        _ => {}
    }
    if i != c.len() {
        return None;
    }
    let n = |s: &Option<String>| s.as_deref().map_or(Some(0), digits_value).map(|v| v as i64);
    let (year, month, day) = (digits_value(&y)? as i64, digits_value(&mo)? as i64, digits_value(&d)? as i64);
    let (hour, minute, second) = (n(&h)?, n(&mi)?, n(&sec)?);
    let micro: String = frac.chars().chain(std::iter::repeat('0')).take(6).collect();
    let micro = digits_value(&micro)? as i64;
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let month_days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || day < 1
        || day > month_days[month as usize - 1]
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let micros = ((days * 86_400 + hour * 3_600 + minute * 60 + second) + shift * 60) * 1_000_000 + micro;
    let lo = days_from_civil(1, 1, 1) * 86_400_000_000;
    let hi = (days_from_civil(9999, 12, 31) + 1) * 86_400_000_000 - 1;
    if !(lo..=hi).contains(&micros) {
        return None;
    }
    let shown = format!("{y}-{mo}-{d} {}:{}", h.as_deref().unwrap_or("00"), mi.as_deref().unwrap_or("00"));
    Some((shown, micros as f64 / 1e6))
}

pub(crate) fn sleep_date(day: &str) -> Option<i64> {
    let c: Vec<char> = day.chars().collect();
    if c.len() != 10 || c[4] != '-' || c[7] != '-' || !c.iter().enumerate().all(|(i, x)| i == 4 || i == 7 || digit(*x).is_some()) {
        return None;
    }
    let month_ok = matches!((c[5], c[6]), ('1', '0'..='2') | ('0', '1'..='9'));
    let day_ok = matches!((c[8], c[9]), ('3', '0' | '1') | ('0', '1'..='9')) || matches!(c[8], '1' | '2');
    if !month_ok || !day_ok {
        return None;
    }
    let num = |r: std::ops::Range<usize>| c[r].iter().fold(0i64, |a, x| a * 10 + i64::from(digit(*x).unwrap_or(0)));
    let (y, m, d) = (num(0..4), num(5..7), num(8..10));
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let month_days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if y < 1 || !(1..=12).contains(&m) || d < 1 || d > month_days[m as usize - 1] {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

pub(crate) fn today_on(tz: Option<f64>) -> i64 {
    let ms = now_ms() + (tz.unwrap_or(0.0) * 3_600_000.0) as i64;
    ms.div_euclid(86_400_000)
}

fn sleep_memories(rows: &[Value], being: &str, assistant: &str, line_time: SleepLineTime, count: &Count) -> (Vec<Line>, bool) {
    let when = |v: Option<&Value>| v.and_then(Value::as_str).and_then(sleep_when);
    let speaker_of = |r: &Value| r.get("speaker").and_then(Value::as_str).map(crate::js_trim).unwrap_or("").to_string();
    let people: std::collections::HashSet<String> =
        rows.iter().map(speaker_of).filter(|s| !s.is_empty() && s != assistant && s != "me").collect();
    let many = people.len() >= 2;
    let lines = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let text = r.get("content").and_then(Value::as_str).map(strip).unwrap_or("").to_string();
            let said = r.get("created_at");
            let event = r.get("event_date").filter(|v| line_time == SleepLineTime::EventDate && truthy(Some(v)));
            let (at, ts) = when(event.or(said)).or_else(|| when(said)).unwrap_or(("1970-01-01 00:00".into(), 0.0));
            let speaker = speaker_of(r);
            let mine = !speaker.is_empty() && (speaker == assistant || speaker == "me");
            let who = if mine { being } else if many && !speaker.is_empty() { &speaker } else { "user" };
            let size = match (&count.0, whole(r.get("tokens"))) {
                (Some(f), _) => f(&text),
                (None, Some(n)) if n >= 0 => n as u64,
                (None, _) => sleep_tokens(&text),
            };
            Line {
                eid: format!("e{}", i + 1),
                mid: r.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                at,
                who: who.to_string(),
                mine,
                text,
                tokens: size.saturating_add(SLEEP_LINE_FRAME),
                ts,
            }
        })
        .collect();
    (lines, many)
}

fn token_sum(mems: &[Line]) -> u64 {
    mems.iter().fold(0, |n: u64, m| n.saturating_add(m.tokens))
}

fn sleep_parts(mems: &[Line], limit: u64) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let (mut start, mut size) = (0, 0u64);
    for (j, m) in mems.iter().enumerate() {
        if j > start && size.saturating_add(m.tokens) > limit {
            let len = j - start;
            let tail = (len / 4).max(1);
            let mut cut: Option<(f64, usize)> = None;
            for i in (len - tail..len).filter(|i| *i > 0) {
                let gap = mems[start + i].ts - mems[start + i - 1].ts;
                if cut.is_none_or(|(g, at)| gap > g || (gap == g && i > at)) {
                    cut = Some((gap, i));
                }
            }
            let cut = cut.map_or(len, |(_, i)| i);
            out.push(start..start + cut);
            start += cut;
            size = token_sum(&mems[start..j]);
        }
        size = size.saturating_add(m.tokens);
    }
    if start < mems.len() {
        out.push(start..mems.len());
    }
    out
}

fn sleep_lines(mems: &[Line]) -> String {
    mems.iter().map(|m| format!("{} · {} · {} · {}", m.eid, m.at, m.who, m.text)).collect::<Vec<_>>().join("\n")
}

fn fenced(t: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(p) = t[from..].find("```") {
        let start = from + p;
        let mut q = start + 3;
        if t[q..].starts_with("json") {
            q += 4;
        }
        q = t.len() - t[q..].trim_start_matches(py_space).len();
        if t[q..].starts_with('{') {
            let close = t[q..]
                .rmatch_indices('}')
                .map(|(r, _)| q + r)
                .find(|r| t[r + 1..].trim_start_matches(py_space).starts_with("```"));
            if let Some(r) = close {
                return Some(&t[q..=r]);
            }
        }
        from = start + 1;
    }
    None
}

fn sleep_numbers(v: &mut Value) {
    match v {
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                *v = if f.fract() == 0.0 && f.abs() <= SLEEP_EXACT { Value::from(f as i64) } else { json!(f) };
            }
        }
        Value::Array(a) => a.iter_mut().for_each(sleep_numbers),
        Value::Object(o) => o.values_mut().for_each(sleep_numbers),
        _ => {}
    }
}

fn sleep_parse_json(text: &str) -> Option<Map<String, Value>> {
    let t = strip(text);
    let t = fenced(t).unwrap_or(t);
    let (i, j) = (t.find('{')?, t.rfind('}')?);
    if j < i {
        return None;
    }
    match serde_json::from_str::<Value>(&t[i..=j]) {
        Ok(Value::Object(mut o)) => {
            o.values_mut().for_each(sleep_numbers);
            Some(o)
        }
        _ => None,
    }
}

fn sleep_sections(body: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, Vec<&str>)> = Vec::new();
    for ln in body.split('\n') {
        if let Some(h) = ln.strip_prefix("## ") {
            out.push((strip(h).to_string(), Vec::new()));
        } else if let Some(last) = out.last_mut() {
            last.1.push(ln);
        }
    }
    out.into_iter().map(|(h, b)| (h, strip(&b.join("\n")).to_string())).collect()
}

fn section_of(body: &str, heading: &str) -> Option<String> {
    sleep_sections(body).into_iter().rev().find(|(h, _)| h == heading).map(|(_, t)| t)
}

fn sleep_join(secs: &[(String, String)]) -> String {
    let joined: Vec<String> = secs
        .iter()
        .filter(|(h, _)| !h.is_empty())
        .map(|(h, t)| format!("## {h}\n{t}").trim_end_matches(py_space).to_string())
        .collect();
    strip(&joined.join("\n\n")).to_string() + if secs.is_empty() { "" } else { "\n" }
}

fn sleep_as_lines(text: &str) -> String {
    strip(text)
        .split('\n')
        .filter(|ln| !ln.starts_with("## "))
        .map(|ln| {
            if ln.starts_with("- ") || strip(ln).is_empty() {
                ln.to_string()
            } else {
                format!("- {}", strip(strip(ln).trim_start_matches('-')))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn sleep_set_section(body: &str, heading: &str, text: &str, raw: bool) -> String {
    let heading = strip(strip(heading).trim_start_matches('#')).to_string();
    let text = if raw { sleep_demote(strip(text)) } else { sleep_as_lines(text) };
    let mut secs = sleep_sections(body);
    match secs.iter_mut().find(|(h, _)| *h == heading) {
        Some(s) => s.1 = text,
        None => secs.push((heading, text)),
    }
    sleep_join(&secs)
}

fn sleep_drop_section(body: &str, heading: &str) -> (String, Option<String>) {
    let secs = sleep_sections(body);
    let gone = secs.iter().find(|(h, _)| h == heading).map(|(_, t)| t.clone());
    let keep: Vec<(String, String)> = secs.into_iter().filter(|(h, _)| h != heading).collect();
    (sleep_join(&keep), gone)
}

fn sleep_demote(text: &str) -> String {
    text.split('\n').map(|ln| if ln.starts_with("## ") { format!("#{ln}") } else { ln.to_string() }).collect::<Vec<_>>().join("\n")
}

fn sleep_age(day: Option<&str>, today: i64) -> String {
    let Some(day) = day.filter(|d| !d.is_empty()) else { return String::new() };
    let Some(d) = sleep_date(day) else { return String::new() };
    let n = today - d;
    format!("last edited {day}, {}", if n == 0 { "today".to_string() } else { format!("{n} days ago") })
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SleepFile {
    pub(crate) title: String,
    pub(crate) version: i64,
    pub(crate) tokens: i64,
    pub(crate) day: Option<String>,
    pub(crate) body: Option<String>,
}

pub(crate) type SleepView = [SleepFile; 5];

pub(crate) fn sleep_has_body(f: &SleepFile) -> bool {
    match &f.body {
        Some(b) => !strip(b).is_empty(),
        None => f.tokens > 0,
    }
}

pub(crate) fn sleep_box_index(view: &SleepView, today: i64) -> String {
    SLEEP_BOXES
        .iter()
        .map(|&s| {
            let f = &view[s];
            let name = if f.title.is_empty() { "(empty)" } else { &f.title };
            let age = if sleep_has_body(f) { format!(" ({})", sleep_age(f.day.as_deref(), today)) } else { String::new() };
            format!("{s}. {name}{age}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn sleep_render(view: &SleepView, today: i64) -> String {
    let core = &view[0];
    let head = match core.day.as_deref().filter(|d| !d.is_empty()) {
        Some(d) => format!("<core {}>", sleep_age(Some(d), today)),
        None => "<core>".to_string(),
    };
    let secs = sleep_sections(core.body.as_deref().unwrap_or(""));
    let (me, people): (Vec<_>, Vec<_>) = secs.into_iter().partition(|(h, _)| h == SLEEP_ME);
    let body = sleep_join(&[me, people].concat());
    format!("{head}\n{}\n</core>\n<boxes>\n{}\n</boxes>", strip(&body), sleep_box_index(view, today))
}

pub(crate) fn sleep_box_pick(view: &SleepView, which: &str) -> Option<usize> {
    let which = strip(which);
    SLEEP_BOXES.into_iter().find(|&s| {
        let title = &view[s].title;
        which == s.to_string()
            || which == title
            || (!title.is_empty() && title.to_lowercase().contains(&which.to_lowercase()))
    })
}

pub(crate) fn sleep_box_text(s: usize, f: &SleepFile, today: i64) -> String {
    let body = f.body.as_deref().unwrap_or("");
    if strip(body).is_empty() {
        return format!("Box {s} is empty.");
    }
    format!("<box {s} name=\"{}\" {}>\n{}\n</box>", f.title, sleep_age(f.day.as_deref(), today), strip(body))
}

pub(crate) fn box_arg(v: Option<&Value>) -> String {
    v.map_or_else(String::new, text_of)
}

pub(crate) fn sleep_file(f: &Value) -> SleepFile {
    let Some(f) = f.as_object() else { return SleepFile::default() };
    SleepFile {
        title: f.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
        version: whole(f.get("version")).unwrap_or(0),
        tokens: whole(f.get("tokens")).unwrap_or(0),
        day: f.get("day").and_then(Value::as_str).filter(|d| !d.is_empty()).map(str::to_string),
        body: f.get("body").and_then(Value::as_str).map(str::to_string),
    }
}

pub(crate) fn sleep_view(r: &Value) -> SleepView {
    let mut view: SleepView = Default::default();
    view[0].body = Some(String::new());
    if let Some(core) = r.get("core").filter(|c| c.is_object()) {
        view[0] = sleep_file(core);
        view[0].body.get_or_insert_with(String::new);
    }
    for f in r.get("files").and_then(Value::as_array).into_iter().flatten() {
        if let Some(slot) = int_of(f.get("slot")).filter(|s| (1..=4).contains(s)) {
            view[slot as usize] = sleep_file(f);
        }
    }
    view
}

fn sleep_notes_of(r: &Value) -> (i64, Map<String, Value>) {
    let v = match r.as_object() {
        Some(o) if o.contains_key("version") => o.get("version"),
        Some(o) => o.get("notes_version"),
        None => None,
    };
    let mut notes = r.get("notes").and_then(Value::as_object).cloned().unwrap_or_default();
    for k in ["proposals", "mixed"] {
        if !notes.get(k).is_some_and(Value::is_array) {
            notes.insert(k.into(), json!([]));
        }
    }
    let readable = |p: &Value| {
        p.as_object().is_some_and(|o| ["id", "text", "first_day"].iter().all(|k| o.contains_key(*k)))
            && whole(p.get("sleeps")).is_some_and(|n| n >= 1)
            && p.get("first").and_then(Value::as_f64).is_some()
    };
    let kept: Vec<Value> = notes["proposals"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|&p| readable(p))
        .map(|p| {
            let mut p = p.clone();
            p["sleeps"] = json!(whole(p.get("sleeps")));
            p
        })
        .collect();
    notes.insert("proposals".into(), Value::Array(kept));
    (whole(v).filter(|n| *n >= 0).unwrap_or(0), notes)
}

fn sleep_written(r: &Value, writes: &[Value]) -> std::collections::BTreeMap<i64, i64> {
    let mut out = std::collections::BTreeMap::new();
    for key in ["written", "files", "writes"] {
        for x in r.get(key).and_then(Value::as_array).into_iter().flatten() {
            if let (Some(slot), Some(v)) = (int_of(x.get("slot")), whole(x.get("version"))) {
                out.insert(slot, v);
            }
        }
    }
    if let Some(versions) = r.get("versions").and_then(Value::as_object) {
        for (k, v) in versions {
            if let (Some(slot), Some(v)) = (digits_value(k), whole(Some(v))) {
                out.insert(slot.min(i64::MAX as u64) as i64, v);
            }
        }
    }
    for w in writes {
        let slot = w["slot"].as_i64().unwrap_or(0);
        out.entry(slot).or_insert(w["base_version"].as_i64().unwrap_or(0).saturating_add(1));
    }
    out
}

fn sleep_day(today: &str) -> f64 {
    let day: String = today.chars().take(10).collect();
    match sleep_date(&day) {
        Some(d) => (d * 86_400) as f64,
        None => now_ms() as f64 / 1000.0,
    }
}

fn sleep_today(head: &Map<String, Value>) -> String {
    let local = head.get("clock").and_then(|c| c.get("local_now")).and_then(Value::as_str);
    if let Some((shown, _)) = local.and_then(sleep_when) {
        return shown;
    }
    let ms = now_ms();
    let (y, m, d) = civil_from_days(ms.div_euclid(86_400_000));
    let mins = ms.rem_euclid(86_400_000) / 60_000;
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", mins / 60, mins % 60)
}

fn sleep_dated(s: &str) -> bool {
    let c: Vec<char> = s.chars().collect();
    let sep = |i: usize| matches!(c.get(i), Some('-' | '.' | '/'));
    let digits_at = |i: usize| c[i..].iter().take_while(|x| digit(**x).is_some()).count();
    fn runs(i: usize, lo: usize, hi: usize, have: usize) -> impl Iterator<Item = usize> {
        (lo..=hi.min(have)).map(move |n| i + n)
    }
    (0..c.len()).any(|i| {
        let ymd = digits_at(i) >= 4
            && sep(i + 4)
            && runs(i + 5, 1, 2, digits_at(i + 5)).any(|j| sep(j) && digits_at(j + 1) >= 1);
        let dmy = runs(i, 1, 2, digits_at(i)).any(|j| {
            sep(j) && runs(j + 1, 1, 2, digits_at(j + 1)).any(|k| sep(k) && digits_at(k + 1) >= 4)
        });
        ymd || dmy
    })
}

fn sleep_eids(v: Option<&Value>) -> Vec<String> {
    let texts: Vec<String> = match v {
        Some(Value::Array(a)) => a.iter().map(text_of).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    for t in &texts {
        for e in find_eids(t) {
            if !out.contains(&e) {
                out.push(e);
            }
        }
    }
    out
}

fn find_eids(s: &str) -> Vec<String> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == 'e' && (i == 0 || !word(c[i - 1])) {
            let mut j = i + 1;
            while j < c.len() && digit(c[j]).is_some() {
                j += 1;
            }
            if j > i + 1 && (j == c.len() || !word(c[j])) {
                out.push(c[i..j].iter().collect());
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn eid_number(e: &str) -> u64 {
    e.strip_prefix('e').and_then(digits_value).unwrap_or(0)
}

fn listed(v: Option<&Value>) -> Vec<Value> {
    match v {
        Some(x) if !truthy(Some(x)) => Vec::new(),
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Object(o)) => o.keys().map(|k| Value::String(k.clone())).collect(),
        Some(Value::String(s)) => s.chars().map(|c| Value::String(c.to_string())).collect(),
        _ => Vec::new(),
    }
}

fn sleep_renumbered(got: &mut Map<String, Value>, part: &[Line], log: &Log, k: usize) {
    let own: std::collections::HashSet<&str> = part.iter().map(|m| m.eid.as_str()).collect();
    let mut refs: Vec<String> = Vec::new();
    for x in listed(got.get("mixed")) {
        for f in ["line", "source"] {
            if truthy(x.get(f)) {
                refs.push(text_of(&x[f]));
            }
        }
    }
    for it in listed(got.get("items")) {
        if it.is_object() {
            refs.extend(sleep_eids(it.get("evidence")));
        }
    }
    if refs.is_empty() || refs.iter().any(|r| own.contains(r.as_str())) {
        return;
    }
    let n = |r: &str| -> u64 { r.strip_prefix('e').and_then(digits_value).unwrap_or(0) };
    if !refs.iter().all(|r| (1..=part.len() as u64).contains(&n(r))) {
        return;
    }
    let fix = |e: &str| -> Option<String> {
        let i = e.strip_prefix('e').filter(|r| r.bytes().all(|b| b.is_ascii_digit()) && !r.starts_with('0'))?;
        part.get(i.parse::<usize>().ok()?.checked_sub(1)?).map(|m| m.eid.clone())
    };
    if let Some(Value::Array(mixed)) = got.get_mut("mixed") {
        for x in mixed.iter_mut().filter_map(Value::as_object_mut) {
            for f in ["line", "source"] {
                if let Some(v) = x.get(f).filter(|v| !v.is_null()) {
                    if let Some(to) = fix(&text_of(v)) {
                        x.insert(f.into(), Value::String(to));
                    }
                }
            }
        }
    }
    if let Some(Value::Array(items)) = got.get_mut("items") {
        for it in items.iter_mut().filter_map(Value::as_object_mut) {
            if truthy(it.get("evidence")) {
                let ev: Vec<Value> =
                    sleep_eids(it.get("evidence")).into_iter().map(|e| Value::String(fix(&e).unwrap_or(e))).collect();
                it.insert("evidence".into(), Value::Array(ev));
            }
        }
    }
    log.push(json!({"call": format!("read {k}"), "renumbered": true, "part": [part[0].eid, part[part.len() - 1].eid]}));
}

fn sleep_doubt_lines_out(text: &str, drop: Option<&Value>, count: &Count) -> Option<String> {
    let rows: Vec<&str> = text.split('\n').filter(|ln| !strip(ln).is_empty()).collect();
    let mut gone: Vec<usize> = Vec::new();
    for n in drop.and_then(Value::as_array).into_iter().flatten() {
        let Some(n) = digits_value(&text_of(n)) else { continue };
        let ok = (1..=rows.len() as u64).contains(&n) && {
            let row = rows[n as usize - 1];
            !row.chars().any(|c| digit(c).is_some()) && count.of(row) <= SLEEP_DOUBT_TOKENS
        };
        if ok && !gone.contains(&(n as usize)) {
            gone.push(n as usize);
        }
    }
    if gone.is_empty() || gone.len() == rows.len() {
        return None;
    }
    Some(rows.iter().enumerate().filter(|(i, _)| !gone.contains(&(i + 1))).map(|(_, ln)| *ln).collect::<Vec<_>>().join("\n"))
}

#[derive(Debug, Clone, PartialEq)]
struct Item {
    kind: String,
    topic: String,
    text: String,
    evidence: Vec<String>,
}

impl Item {
    fn value(&self) -> Value {
        json!({"kind": self.kind, "topic": self.topic, "text": self.text, "evidence": self.evidence})
    }
}

fn items_json(items: &[Item]) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
    if items.is_empty() {
        return "[]".into();
    }
    let one = |it: &Item| {
        let ev = if it.evidence.is_empty() {
            "[]".to_string()
        } else {
            format!("[\n{}\n]", it.evidence.iter().map(|e| q(e)).collect::<Vec<_>>().join(",\n"))
        };
        format!(
            "{{\n\"kind\": {},\n\"topic\": {},\n\"text\": {},\n\"evidence\": {ev}\n}}",
            q(&it.kind),
            q(&it.topic),
            q(&it.text)
        )
    };
    format!("[\n{}\n]", items.iter().map(one).collect::<Vec<_>>().join(",\n"))
}

fn sleep_checked_items(
    got: Option<&Map<String, Value>>,
    known: &std::collections::HashSet<String>,
    said: &std::collections::HashSet<String>,
) -> (Vec<Item>, usize) {
    let (mut ok, mut dropped) = (Vec::new(), 0);
    for it in listed(got.and_then(|g| g.get("items"))) {
        let ev = if it.is_object() { sleep_eids(it.get("evidence")) } else { Vec::new() };
        let kind = it.get("kind").and_then(Value::as_str).filter(|k| SLEEP_KINDS.contains(k));
        let good = match kind {
            Some(kind) => {
                !strip(&text_or_empty(it.get("text"))).is_empty()
                    && ev.len() >= SLEEP_MIN_EVIDENCE
                    && ev.iter().all(|x| known.contains(x))
                    && (kind != "self" || ev.iter().any(|x| said.contains(x)))
            }
            None => false,
        };
        if good {
            let mut evidence = ev;
            evidence.sort_by_key(|e| eid_number(e));
            ok.push(Item {
                kind: kind.unwrap_or_default().to_string(),
                topic: text_or_empty(it.get("topic")),
                text: strip(&text_of(&it["text"])).to_string(),
                evidence,
            });
        } else {
            dropped += 1;
        }
    }
    (ok, dropped)
}

fn sleep_mark_key(x: &Value) -> String {
    let field = |k: &str| x.get(k).cloned().unwrap_or(Value::Null);
    json!([field("memory"), field("kind"), field("source_memory")]).to_string()
}

fn sleep_checked_mixed(got: Option<&Map<String, Value>>, part: &[Line]) -> (Vec<Value>, usize) {
    let pos = |e: &str| part.iter().position(|m| m.eid == e);
    let (mut mixed, mut dropped) = (Vec::new(), 0);
    for x in listed(got.and_then(|g| g.get("mixed"))) {
        let line = if x.is_object() { text_of(x.get("line").unwrap_or(&Value::Null)) } else { String::new() };
        let kind = x.get("kind").and_then(Value::as_str).filter(|k| SLEEP_MIX_KINDS.contains(k));
        let (Some(at), Some(kind)) = (pos(&line), kind) else {
            dropped += 1;
            continue;
        };
        let src = text_or_empty(x.get("source"));
        let src = pos(&src).filter(|_| src != line);
        let mine = part[at].mine;
        let from_me = src.is_some_and(|s| part[s].mine);
        if (kind == "echo" && (mine || (src.is_some() && !from_me)))
            || (kind == "planted" && (!mine || (src.is_some() && !from_me)))
        {
            dropped += 1;
            continue;
        }
        let why: String = text_or_empty(x.get("why")).chars().take(300).collect();
        mixed.push(json!({"memory": part[at].mid, "at": part[at].at, "kind": kind,
                          "source_memory": src.map(|s| part[s].mid.clone()), "why": why}));
    }
    (mixed, dropped)
}

#[derive(Default)]
struct Log(std::sync::Mutex<Vec<Value>>);

impl Log {
    fn push(&self, v: Value) {
        lock(&self.0).push(v);
    }

    fn take(&self) -> Vec<Value> {
        std::mem::take(&mut *lock(&self.0))
    }
}

struct Run<'a> {
    client: &'a Client,
    store: String,
    being: String,
    s: SleepSettings,
    count: Count,
    since: Option<String>,
    sleep_id: Option<String>,
}

struct Night<'a> {
    run: Run<'a>,
    read: Asker,
    merge: Asker,
    log: Log,
    prompt: String,
    assistant: Option<String>,
    today: Option<String>,
    done: bool,
    line_time: SleepLineTime,
    say: Option<SleepProgress>,
}

impl Night<'_> {
    fn say(&self, msg: &str) {
        if let Some(f) = &self.say {
            f(msg);
        }
    }
}

fn sleep_usage(u: &Map<String, Value>, keys: &[&str]) -> i64 {
    keys.iter().map(|k| whole(u.get(*k)).filter(|n| *n >= 0).unwrap_or(0)).fold(0, i64::saturating_add)
}

fn sleep_retry_wait(e: &ModelCallError, attempt: u32) -> Option<std::time::Duration> {
    if attempt >= SLEEP_RETRIES {
        return None;
    }
    let reason = e.reason.as_deref();
    if e.status != 0
        && !(matches!(e.status, 408 | 409 | 429) || e.status >= 500 || matches!(reason, Some("rate_limit" | "overloaded")))
    {
        return None;
    }
    if let Some(asked) = e.retry_after.filter(|w| *w <= std::time::Duration::from_secs(60)) {
        return Some(asked);
    }
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    let jitter = (h.finish() % 1_000_000) as f64 / 1_000_000.0;
    Some(std::time::Duration::from_secs_f64(8f64.min(0.5 * 2f64.powi(attempt as i32)) * (1.0 - 0.25 * jitter)))
}

struct Asker {
    caller: Arc<dyn ModelCaller>,
    model: String,
    max_tokens: u32,
}

impl Asker {
    async fn ask(&self, system: &str, user: &str) -> Result<(String, Map<String, Value>), ModelCallError> {
        let mut attempt = 0;
        loop {
            let t0 = std::time::Instant::now();
            let c = ModelCall {
                purpose: CallPurpose::Sleep,
                model: self.model.clone(),
                system: vec![system.to_string()],
                user: user.to_string(),
                max_tokens: self.max_tokens,
                effort: None,
                schema: None,
                system_tokens: None,
                deadline: t0 + SLEEP_CALL_TIME,
            };
            match self.caller.call(&c).await {
                Ok(reply) => {
                    let reply = reply_shape(reply);
                    let u = reply.usage.as_ref().and_then(Value::as_object).cloned().unwrap_or_default();
                    let sent = sleep_usage(&u, &["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"]);
                    let wrote = sleep_usage(&u, &["output_tokens"]);
                    let mut meta = Map::new();
                    meta.insert("secs".into(), json!(round_to(t0.elapsed().as_secs_f64(), 1)));
                    meta.insert("in".into(), json!(if sent != 0 { sent } else { sleep_usage(&u, &["prompt_tokens"]) }));
                    meta.insert("out".into(), json!(if wrote != 0 { wrote } else { sleep_usage(&u, &["completion_tokens"]) }));
                    meta.insert("stop".into(), json!(reply.stop));
                    return Ok((reply.text, meta));
                }
                Err(e) => match sleep_retry_wait(&e, attempt) {
                    None => return Err(e),
                    Some(wait) => {
                        attempt += 1;
                        tokio::time::sleep(wait).await;
                    }
                },
            }
        }
    }

    async fn ask_json_seeing(
        &self,
        system: &str,
        user: &str,
        key: &str,
        log: &Log,
        mut seen: impl FnMut(String),
    ) -> Result<Option<Map<String, Value>>, ModelCallError> {
        for attempt in 1..=2 {
            let sent = if attempt == 1 { user.to_string() } else { format!("{user}{SLEEP_NOT_JSON}") };
            let (text, meta) = self.ask(system, &sent).await?;
            let mut entry = Map::new();
            entry.insert("call".into(), json!(key));
            entry.insert("attempt".into(), json!(attempt));
            entry.extend(meta);
            log.push(Value::Object(entry));
            if let Some(got) = sleep_parse_json(&text) {
                return Ok(Some(got));
            }
            seen(text);
        }
        Ok(None)
    }

    async fn ask_json(&self, system: &str, user: &str, key: &str, log: &Log) -> Result<Option<Map<String, Value>>, ModelCallError> {
        self.ask_json_seeing(system, user, key, log, |_| {}).await
    }

    async fn ask_json_logged(
        &self,
        system: &str,
        user: &str,
        key: &str,
        log: &Log,
    ) -> Result<Option<Map<String, Value>>, ModelCallError> {
        let mut last: Option<String> = None;
        let got = self.ask_json_seeing(system, user, key, log, |t| last = Some(t)).await?;
        if let (None, Some(t)) = (&got, last) {
            let chars: Vec<char> = t.chars().collect();
            let head: String = chars.iter().take(300).collect();
            let tail: String = chars[chars.len().saturating_sub(300)..].iter().collect();
            log.push(json!({"call": key, "failed": true, "chars": chars.len(), "head": head, "tail": tail}));
        }
        Ok(got)
    }
}

async fn sleep_read_all(run: &Run<'_>) -> Result<(Map<String, Value>, Vec<Value>), WosError> {
    let mut body = Map::new();
    body.insert("user_id".into(), json!(run.store));
    if let Some(m) = run.client.clock(None)? {
        body.insert("tz_offset_minutes".into(), json!(m));
    }
    body.insert("budget_tokens".into(), json!(SLEEP_PAGE_TOKENS));
    if let Some(id) = &run.sleep_id {
        body.insert("sleep_id".into(), json!(id));
    }
    match &run.since {
        Some(since) => body.insert("since".into(), json!(since)),
        None => body.insert("since_last_sleep".into(), json!(true)),
    };
    let mut rows: Vec<Value> = Vec::new();
    let mut head: Option<Map<String, Value>> = None;
    let mut seen = std::collections::HashSet::new();
    loop {
        let page = keep_records(run.client.post("/api/v1/memory/sleep", Value::Object(body.clone())).await?, "memories");
        let page = page.as_object().cloned().unwrap_or_default();
        let memories = page.get("memories").and_then(Value::as_array).cloned().unwrap_or_default();
        let head = head.get_or_insert_with(|| page.clone());
        rows.extend(memories.iter().cloned());
        let next = page.get("next_cursor");
        if truthy(page.get("exhausted")) || !truthy(next) {
            return Ok((head.clone(), rows));
        }
        let next = next.cloned().unwrap_or_default();
        if !seen.insert(next.to_string()) {
            if memories.is_empty() {
                return Ok((head.clone(), rows));
            }
            return Err(truncated_walk("the service returned a page cursor it had already returned"));
        }
        if seen.len() > MAX_PAGES as usize {
            return Err(truncated_walk("the sleep's reading passed its page limit"));
        }
        body.insert("cursor".into(), next);
        if let Some(sid) = head.get("sleep_id").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            body.insert("sleep_id".into(), json!(sid));
        }
    }
}

type ReaderOut = (Vec<Item>, usize, Vec<Value>, Option<usize>);

async fn sleep_read_one(
    n: &Night<'_>,
    k: usize,
    of: usize,
    part: &[Line],
    many: bool,
    core: &str,
    titles: &str,
) -> Result<ReaderOut, ModelCallError> {
    let digest = sleep_people(SLEEP_DIGEST, SLEEP_ONE_PERSON.digest, SLEEP_MANY_PEOPLE.digest, many);
    let system = fill(&digest, &[("being", Arg::Text(&n.run.being)), ("k", Arg::Num(k as i128)), ("n", Arg::Num(of as i128))]);
    let core = if core.is_empty() { "(empty)" } else { core };
    let user = format!("<core>\n{core}\n</core>\n<boxes>\n{titles}\n</boxes>\n<memories>\n{}\n</memories>", sleep_lines(part));
    let mut got = None;
    for attempt in 1..=3 {
        let key = if attempt == 1 { format!("read {k}") } else { format!("read {k} again {attempt}") };
        got = n.read.ask_json_logged(&system, &user, &key, &n.log).await?;
        if let Some(g) = got.as_mut() {
            sleep_renumbered(g, part, &n.log, k);
            break;
        }
    }
    let known = part.iter().map(|m| m.eid.clone()).collect();
    let said = part.iter().filter(|m| m.mine).map(|m| m.eid.clone()).collect();
    let (found, dropped) = sleep_checked_items(got.as_ref(), &known, &said);
    let (mixed, mixed_dropped) = sleep_checked_mixed(got.as_ref(), part);
    if mixed_dropped > 0 {
        n.log.push(json!({"call": format!("read {k}"), "mixed_dropped": mixed_dropped}));
    }
    Ok((found, dropped, mixed, got.is_none().then_some(k)))
}

async fn sleep_each<T, F>(futures: Vec<F>, width: u64) -> Result<Vec<T>, ModelCallError>
where
    F: std::future::Future<Output = Result<T, ModelCallError>>,
{
    use futures_util::StreamExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    let width = usize::try_from(width).unwrap_or(usize::MAX).max(1);
    let failed = AtomicBool::new(false);
    let runs = futures.into_iter().map(|f| {
        let failed = &failed;
        async move {
            if failed.load(Ordering::Relaxed) {
                return None;
            }
            let r = f.await;
            if r.is_err() {
                failed.store(true, Ordering::Relaxed);
            }
            Some(r)
        }
    });
    let done: Vec<Option<Result<T, ModelCallError>>> = futures_util::stream::iter(runs).buffered(width).collect().await;
    done.into_iter().flatten().collect()
}

async fn sleep_combine(
    n: &Night<'_>,
    g: &[Vec<Item>],
    rounds: usize,
    known_all: &std::collections::HashSet<String>,
    said_all: &std::collections::HashSet<String>,
) -> Result<Vec<Item>, ModelCallError> {
    let flat: Vec<Item> = g.iter().flatten().cloned().collect();
    if g.len() == 1 || flat.is_empty() {
        return Ok(flat);
    }
    let system = fill(SLEEP_COMBINE, &[("being", Arg::Text(&n.run.being))]);
    let got = n.merge.ask_json(&system, &items_json(&flat), &format!("combine {rounds}"), &n.log).await?;
    let (joined, _) = sleep_checked_items(got.as_ref(), known_all, said_all);
    Ok(if joined.is_empty() { flat } else { joined })
}

async fn sleep_settle(n: &Night<'_>, core: &str, sec: &str, text: &str) -> Result<(Option<String>, String), ModelCallError> {
    let text = sleep_as_lines(text);
    let Some(old) = section_of(core, sec) else { return Ok((Some(text), "new".into())) };
    let mut held: Vec<String> = old.split('\n').map(strip).filter(|l| !l.is_empty()).map(str::to_string).collect();
    let tonight: Vec<&str> = text.split('\n').map(strip).filter(|l| !l.is_empty()).collect();
    if held.iter().filter(|ln| !tonight.contains(&ln.as_str())).count() <= SLEEP_OVERTURN {
        return Ok((Some(text), "kept".into()));
    }
    let numbered = held.iter().enumerate().map(|(i, ln)| format!("{}. {ln}", i + 1)).collect::<Vec<_>>().join("\n");
    let system = fill(SLEEP_SETTLE, &[("being", Arg::Text(&n.run.being)), ("name", Arg::Text(sec))]);
    let user = format!("<as it stood>\n{numbered}\n</as it stood>\n<tonight>\n{text}\n</tonight>");
    let Some(g) = n.merge.ask_json(&system, &user, &format!("settle {sec}"), &n.log).await? else {
        return Ok((None, "unshaped".into()));
    };
    let as_line = |x: &str| {
        let t = strip(x);
        if t.starts_with("- ") { t.to_string() } else { format!("- {}", strip(t.trim_start_matches('-'))) }
    };
    let adds: Vec<String> = match g.get("add") {
        Some(Value::Array(a)) => a.iter().map(text_of).filter(|x| !strip(x).is_empty()).map(|x| as_line(&x)).collect(),
        _ => Vec::new(),
    };
    let line = as_line(&text_or_empty(g.get("text")));
    let at = int_like(g.get("line")).unwrap_or(0);
    let in_held = at >= 1 && at as usize <= held.len();
    let op = match g.get("op").and_then(Value::as_str) {
        Some("replace") if line != "- " && in_held => {
            held[at as usize - 1] = line;
            "replace"
        }
        Some("drop") if in_held => {
            held.remove(at as usize - 1);
            "drop"
        }
        _ => "none",
    };
    let fresh: Vec<String> = adds.iter().filter(|x| !held.contains(x)).cloned().collect();
    let how = format!("{op}, +{}", adds.len());
    Ok((Some([held, fresh].concat().join("\n")), how))
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Held {
    title: String,
    body: String,
    version: i64,
    day: Option<String>,
}

type Files = [Held; 5];

#[derive(Default)]
struct Ledger {
    refused: Vec<Value>,
    rooms: Vec<Value>,
}

fn sleep_where(new: &Files, k: usize) -> String {
    if k == 0 { "core".into() } else { format!("box \"{}\"", new[k].title) }
}

#[allow(clippy::too_many_arguments)]
async fn sleep_room(
    n: &Night<'_>,
    new: &mut Files,
    files: &Files,
    k: usize,
    used_now: u64,
    cap: u64,
    ledger: &mut Ledger,
    why: Option<&str>,
) -> Result<(), ModelCallError> {
    let count = &n.run.count;
    let where_ = sleep_where(new, k);
    let core_way = if k == 0 { ", or move people who matter less to a box (say which, under \"moved\")" } else { "" };
    let moved = if k == 0 { ", \"moved\": [{\"section\": \"<heading>\", \"to\": \"1|2|3|4\"}]" } else { "" };
    let system = fill(
        SLEEP_ROOM,
        &[
            ("being", Arg::Text(&n.run.being)),
            ("where", Arg::Text(&where_)),
            ("used", Arg::Num(used_now.into())),
            ("cap", Arg::Num(cap.into())),
            ("core_way", Arg::Text(core_way)),
            ("moved", Arg::Text(moved)),
        ],
    );
    let user = format!("<{where_}>\n{}\n</{where_}>", new[k].body);
    let g = n.merge.ask_json(&system, &user, &format!("room {k}"), &n.log).await?.unwrap_or_default();
    let text = text_or_empty(g.get("text"));
    let mut body = strip(&text).to_string();
    body = if body.starts_with("## ") { sleep_join(&sleep_sections(&body)) } else { String::new() };
    if k == 0 && !body.is_empty() {
        match section_of(&new[0].body, SLEEP_ME) {
            None => body = sleep_drop_section(&body, SLEEP_ME).0,
            Some(mine) => {
                if section_of(&body, SLEEP_ME).as_ref() != Some(&mine) {
                    body = sleep_set_section(&body, SLEEP_ME, &mine, true);
                }
            }
        }
        for mv in listed(g.get("moved")) {
            let to = text_of(mv.get("to").unwrap_or(&Value::Null));
            if !mv.is_object() || !["1", "2", "3", "4"].contains(&to.as_str()) {
                continue;
            }
            let sec = strip(&text_or_empty(mv.get("section"))).to_string();
            if let Some(held) = section_of(&new[0].body, &sec) {
                if sec != SLEEP_ME && section_of(&body, &sec).is_none() {
                    let to: usize = to.parse().unwrap_or(1);
                    new[to].body = sleep_set_section(&new[to].body, &sec, &held, false);
                }
            }
        }
    }
    if !body.is_empty() && count.of(&body) <= cap {
        ledger.rooms.push(json!({"file": k, "before": used_now, "after": count.of(&body)}));
        new[k].body = body;
    } else {
        ledger.rooms.push(json!({"file": k, "before": used_now, "after": null}));
        let why = why.map_or_else(
            || {
                format!(
                    "no room: {} of {} tokens, and the room made came back unshaped or still too big; the file stays as it was",
                    commas(used_now.into()),
                    commas(cap.into())
                )
            },
            str::to_string,
        );
        ledger.refused.push(json!({"edit": {"file": k, "op": "room"}, "why": why}));
        new[k] = files[k].clone();
    }
    Ok(())
}

fn sleep_boxes_fit(n: &Night<'_>, new: &mut Files, files: &Files, ledger: &mut Ledger) {
    for k in SLEEP_BOXES {
        if n.run.count.of(&new[k].body) > n.run.s.box_tokens {
            ledger.refused.push(json!({"edit": {"file": k, "op": "room"}, "why": "over its cap after a move; the box stays as it was"}));
            new[k] = files[k].clone();
        }
    }
}

async fn sleep_seed(n: &Night<'_>, files: &mut Files, sleep_id: &str, day: &str) -> Result<bool, WosError> {
    let core = &files[0];
    if strip(&n.prompt).is_empty() || sleep_sections(&core.body).iter().any(|(h, _)| h == SLEEP_ME) {
        return Ok(false);
    }
    let secs = [vec![(SLEEP_ME.to_string(), sleep_demote(strip(&n.prompt)))], sleep_sections(&core.body)].concat();
    let body = sleep_join(&secs);
    let size = n.run.count.of(&body);
    if size > n.run.s.core_tokens {
        let fix = if n.run.s.core_tokens < SLEEP_CORE_MAX { "raise core_tokens or shorten the prompt" } else { "shorten the prompt" };
        return Err(refused(format!(
            "the core with the prompt in it would be {} tokens, over its {}: {fix}",
            commas(size.into()),
            commas(n.run.s.core_tokens.into())
        )));
    }
    let writes = vec![json!({"slot": 0, "base_version": core.version, "title": "", "body": body, "evidence": [], "day": day})];
    let r = n
        .run
        .client
        .post_idem("/api/v1/memory/self/write", json!({"user_id": n.run.store, "sleep_id": sleep_id, "writes": writes}), None)
        .await?;
    let version = sleep_written(&r, &writes).get(&0).copied().unwrap_or(core.version.saturating_add(1));
    files[0] = Held { title: String::new(), body, version, day: Some(day.to_string()) };
    Ok(true)
}

fn sleep_no_room(message: &str) -> Option<(i64, i64, i64)> {
    let c: Vec<char> = message.chars().collect();
    let lit = |i: usize, s: &str| -> Option<usize> {
        let n = s.chars().count();
        (c.get(i..i + n)?.iter().copied().eq(s.chars())).then_some(i + n)
    };
    let num = |i: usize| -> Option<(i64, usize)> {
        let n = c[i.min(c.len())..].iter().take_while(|x| digit(**x).is_some()).count();
        let v: String = c.get(i..i + n)?.iter().collect();
        Some((digits_value(&v)?.min(i64::MAX as u64) as i64, i + n))
    };
    (0..c.len()).find_map(|i| {
        let j = lit(i, "slot ")?;
        let slot = i64::from(digit(*c.get(j)?)?);
        let mut j = lit(j + 1, " (")?;
        while c.get(j).is_some_and(|x| *x != ')') {
            j += 1;
        }
        let j = lit(j, ") would hold ")?;
        let (tokens, j) = num(j)?;
        let j = lit(j, " tokens, over its cap of ")?;
        let (cap, _) = num(j)?;
        Some((slot, tokens, cap))
    })
}

async fn sleep_write_files(
    n: &Night<'_>,
    sleep_id: &str,
    files: &Files,
    new: &mut Files,
    ev: &[String],
    day: &str,
    ledger: &mut Ledger,
) -> Result<std::collections::BTreeMap<i64, i64>, SleepError> {
    let s = n.run.s;
    let mut made_room: Vec<usize> = Vec::new();
    loop {
        let writes: Vec<Value> = SLEEP_SLOTS
            .into_iter()
            .filter(|&k| new[k].body != files[k].body || new[k].title != files[k].title)
            .map(|k| {
                json!({"slot": k, "base_version": files[k].version, "title": new[k].title, "body": new[k].body,
                       "evidence": ev, "day": day})
            })
            .collect();
        if writes.is_empty() {
            return Ok(Default::default());
        }
        let body = json!({"user_id": n.run.store, "sleep_id": sleep_id, "writes": writes});
        let (e, error) = match n.run.client.write_or_refusal("/api/v1/memory/self/write", body).await {
            Ok(r) => return Ok(sleep_written(&r, &writes)),
            Err(refusal) => refusal,
        };
        let error = error.unwrap_or_default();
        let code = error.get("code").and_then(Value::as_str).or_else(|| error.get("type").and_then(Value::as_str));
        if !matches!(e.status(), Some(400 | 413 | 422)) || code != Some("self_no_room") {
            return Err(e.into());
        }
        let mut d: Map<String, Value> =
            error.iter().filter(|(k, _)| !["message", "type", "request_id"].contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
        let message = match error.get("message").and_then(Value::as_str).filter(|m| !m.is_empty()) {
            Some(m) => m.to_string(),
            None => match &e {
                WosError::Api { message, .. } => crate::split_carried(message).0.to_string(),
                other => other.to_string(),
            },
        };
        if let Some((slot, tokens, cap)) = sleep_no_room(&message) {
            for (key, v) in [("slot", slot), ("tokens", tokens), ("cap", cap)] {
                d.entry(key).or_insert(json!(v));
            }
        }
        let slots: Vec<i64> = writes.iter().map(|w| w["slot"].as_i64().unwrap_or(0)).collect();
        let named = int_of(d.get("slot"));
        let cap_of = |slot: i64| if slot == 0 { s.core_tokens } else { s.box_tokens };
        let k = match named.filter(|x| slots.contains(x)) {
            Some(x) => x,
            None => {
                let fill = |w: &Value| {
                    let slot = w["slot"].as_i64().unwrap_or(0);
                    n.run.count.of(w["body"].as_str().unwrap_or("")) as f64 / cap_of(slot) as f64
                };
                let mut best = &writes[0];
                for w in &writes[1..] {
                    if fill(w) > fill(best) {
                        best = w;
                    }
                }
                best["slot"].as_i64().unwrap_or(0)
            }
        };
        let ku = k as usize;
        let cap = cap_of(k);
        if made_room.contains(&ku) {
            ledger.refused.push(json!({"edit": {"file": k, "op": "room"},
                                       "why": "no room: the service counts it over its cap; the file stays as it was"}));
            new[ku] = files[ku].clone();
            continue;
        }
        made_room.push(ku);
        let used_now = n.run.count.of(&new[ku].body);
        let told = int_of(d.get("tokens"));
        let limit = match int_of(d.get("cap")) {
            Some(c) if named == Some(k) => i128::from(c),
            _ => i128::from(cap),
        };
        let target = match told.map(i128::from) {
            Some(t) if t > 0 && t > limit && named == Some(k) => (limit * i128::from(used_now)).div_euclid(t),
            _ => i128::from(used_now.min(cap)) * 9 / 10,
        };
        let aim = target.min(i128::from(cap)).max(1) as u64;
        let why = "no room: the service counts it over its cap, and the room made came back unshaped or still too big; \
                   the file stays as it was";
        sleep_room(n, new, files, ku, used_now, aim, ledger, Some(why)).await?;
        sleep_boxes_fit(n, new, files, ledger);
    }
}

async fn sleep_done_flow(n: &Night<'_>, sleep_id: &str) -> Result<Value, WosError> {
    if !n.done {
        return Ok(Value::Null);
    }
    n.run.client.post_idem("/api/v1/memory/sleep/done", json!({"user_id": n.run.store, "sleep_id": sleep_id}), None).await
}

fn field(p: &Value, key: &str) -> String {
    text_of(p.get(key).unwrap_or(&Value::Null))
}

async fn sleep_night(n: &Night<'_>) -> Result<Value, SleepError> {
    let (s, being, me, log, count) = (n.run.s, n.run.being.as_str(), SLEEP_ME, &n.log, &n.run.count);
    let client = n.run.client;
    let store = n.run.store.as_str();
    let t0 = std::time::Instant::now();
    let assistant = match &n.assistant {
        Some(a) => a.clone(),
        None => super::store_assistant(client, store).await?.ok_or_else(|| {
            refused(
                "sleep needs the assistant's speaker name to tell its lines from the person's, and the store has none: \
                 set default_speakers.assistant (set_settings) or pass SleepOpts::assistant.",
            )
        })?,
    };
    let (head, rows) = sleep_read_all(&n.run).await?;
    let sid = match head.get("sleep_id").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        Some(sid) => sid.to_string(),
        None => {
            return Err(WosError::Api {
                status: 200,
                message: "the reading came back without a sleep_id: nothing could be written for it".into(),
            }
            .into())
        }
    };
    let today = n.today.clone().unwrap_or_else(|| sleep_today(&head));
    let now = sleep_day(&today);
    let day: String = today.chars().take(10).collect();
    let view = sleep_view(&client.post("/api/v1/memory/self", json!({"user_id": store})).await?);
    let mut files: Files = Default::default();
    files[0] = Held {
        title: view[0].title.clone(),
        body: view[0].body.clone().unwrap_or_default(),
        version: view[0].version,
        day: view[0].day.clone(),
    };
    for k in SLEEP_BOXES {
        if view[k].version != 0 {
            let got = sleep_file(&client.post("/api/v1/memory/self/open", json!({"user_id": store, "slot": k})).await?);
            files[k] = Held {
                title: got.title,
                body: got.body.unwrap_or_default(),
                version: if got.version != 0 { got.version } else { view[k].version },
                day: got.day,
            };
        }
    }
    let (notes_version, mut notes) = sleep_notes_of(&client.post("/api/v1/memory/self/notes", json!({"user_id": store})).await?);
    let before = notes.clone();
    let seeded = if n.prompt.is_empty() { false } else { sleep_seed(n, &mut files, &sid, &day).await? };
    let (mems, many) = sleep_memories(&rows, being, &assistant, n.line_time, count);
    let core = files[0].body.clone();
    let titles = SLEEP_BOXES
        .iter()
        .map(|&k| format!("{k}: {}", if files[k].title.is_empty() { "(empty)" } else { &files[k].title }))
        .collect::<Vec<_>>()
        .join("\n");
    let mut proposals: Vec<Value> = notes["proposals"].as_array().cloned().unwrap_or_default();
    let read_tokens = token_sum(&mems);
    n.say(&format!(
        "{} memories, {} tokens → {} readers ({} at a time)",
        mems.len(),
        commas(read_tokens.into()),
        sleep_parts(&mems, s.part_tokens).len(),
        s.readers
    ));
    let mut out = Map::new();
    out.insert("sleep_id".into(), json!(sid));
    out.insert("sleep_version".into(), json!(SLEEP_VERSION));
    out.insert("today".into(), json!(today));
    out.insert("memories".into(), json!(mems.len()));
    out.insert("tokens".into(), json!(read_tokens));
    out.insert("seeded".into(), json!(seeded));
    if mems.is_empty() {
        for k in ["items", "applied", "refused"] {
            out.insert(k.into(), json!([]));
        }
        out.insert("written".into(), json!({}));
        out.insert("log".into(), Value::Array(log.take()));
        out.insert("secs".into(), json!(0.0));
        out.insert("llm_in".into(), json!(0));
        out.insert("llm_out".into(), json!(0));
        out.insert("done".into(), sleep_done_flow(n, &sid).await?);
        return Ok(Value::Object(out));
    }

    let known_all: std::collections::HashSet<String> = mems.iter().map(|m| m.eid.clone()).collect();
    let said_all: std::collections::HashSet<String> =
        mems.iter().filter(|m| m.mine).map(|m| m.eid.clone()).collect();
    let by_eid: std::collections::HashMap<&str, &Line> = mems.iter().map(|m| (m.eid.as_str(), m)).collect();

    let parts = sleep_parts(&mems, s.part_tokens);
    let readers: Vec<_> = parts
        .iter()
        .enumerate()
        .map(|(i, r)| sleep_read_one(n, i + 1, parts.len(), &mems[r.clone()], many, &core, &titles))
        .collect();
    let (mut lists, mut dropped, mut mixed, mut unread) = (Vec::new(), 0, Vec::new(), Vec::new());
    for (found, d, mx, failed) in sleep_each(readers, s.readers).await? {
        lists.push(found);
        dropped += d;
        mixed.extend(mx);
        unread.extend(failed);
    }
    log.push(json!({"call": "unread parts", "parts": unread}));
    for x in mixed.iter_mut().filter_map(Value::as_object_mut) {
        x.insert("sleep".into(), json!(today));
    }
    if let Some(Value::Array(m)) = notes.get_mut("mixed") {
        let marked: std::collections::HashSet<String> = m.iter().map(sleep_mark_key).collect();
        m.extend(mixed.iter().filter(|x| !marked.contains(&sleep_mark_key(x))).cloned());
    }
    let span_end = mems.last().map(|m| m.mid.clone()).unwrap_or_default();
    let counted_here = |p: &Value| !span_end.is_empty() && p.get("span_end").and_then(Value::as_str) == Some(span_end.as_str());

    let mut rounds = 0;
    let group = usize::try_from(s.group).unwrap_or(usize::MAX);
    while lists.len() > group {
        rounds += 1;
        let groups: Vec<&[Vec<Item>]> = lists.chunks(group).collect();
        let combines = groups.iter().map(|g| sleep_combine(n, g, rounds, &known_all, &said_all)).collect();
        lists = sleep_each(combines, s.readers).await?;
    }
    let items: Vec<Item> = lists.into_iter().flatten().collect();

    let is_ripe = |p: &Value| {
        let sleeps = p.get("sleeps").and_then(Value::as_f64);
        let first = p.get("first").and_then(Value::as_f64);
        !truthy(p.get("applied"))
            && sleeps.is_some_and(|x| x >= SLEEP_RIPE_SLEEPS)
            && first.is_some_and(|f| (now - f) / 86_400.0 >= SLEEP_RIPE_DAYS)
            && !counted_here(p)
    };
    let ripe: Vec<Value> = proposals.iter().filter(|p| is_ripe(p)).cloned().collect();
    n.say(&format!(
        "merging {} items ({rounds} combine rounds) · {} proposals ({} ripe)",
        items.len(),
        proposals.len(),
        ripe.len()
    ));
    let mut new: Files = files.clone();
    let mut ledger = Ledger::default();
    let (mut applied, mut used, mut merge_notes) = (Vec::new(), std::collections::BTreeSet::new(), Vec::new());
    let (mut folds, mut settled, mut deduped, mut tidied) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut touched: std::collections::BTreeSet<(usize, String)> = Default::default();
    if !items.is_empty() || !ripe.is_empty() {
        let blocks = SLEEP_BOXES
            .iter()
            .map(|&k| {
                let f = &files[k];
                format!(
                    "<box {k} name=\"{}\" size=\"{} of {} tokens\">\n{}\n</box {k}>",
                    if f.title.is_empty() { "(empty)" } else { &f.title },
                    commas(count.of(&f.body).into()),
                    commas(s.box_tokens.into()),
                    if f.body.is_empty() { "(empty)" } else { &f.body }
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let prop_text = proposals
            .iter()
            .filter(|p| !truthy(p.get("applied")))
            .map(|p| {
                format!(
                    "{}: {} (seen in {} sleeps since {})",
                    field(p, "id"),
                    field(p, "text"),
                    field(p, "sleeps"),
                    field(p, "first_day")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let ripe_text = ripe.iter().map(|p| format!("{}: {}", field(p, "id"), field(p, "text"))).collect::<Vec<_>>().join("\n");
        let self_rule = fill(SLEEP_SELF_RULE, &[("me", Arg::Text(me)), ("being", Arg::Text(being))]);
        let merge = sleep_people(SLEEP_MERGE, SLEEP_ONE_PERSON.merge, SLEEP_MANY_PEOPLE.merge, many);
        let system = fill(
            &merge,
            &[
                ("being", Arg::Text(being)),
                ("me", Arg::Text(me)),
                ("today", Arg::Text(&today)),
                ("core_used", Arg::Num(count.of(&core).into())),
                ("core_cap", Arg::Num(s.core_tokens.into())),
                ("box_cap", Arg::Num(s.box_tokens.into())),
                ("self_rule", Arg::Text(&self_rule)),
            ],
        );
        let user = format!(
            "<core>\n{}\n</core>\n{blocks}\n<proposals>\n{}\n</proposals>\n<ripe>\n{}\n</ripe>\n<items>\n{}\n</items>",
            if core.is_empty() { "(empty)" } else { &core },
            if prop_text.is_empty() { "(none)" } else { &prop_text },
            if ripe_text.is_empty() { "(none)" } else { &ripe_text },
            items_json(&items)
        );
        let mut got = n.merge.ask_json_logged(&system, &user, "merge", log).await?;
        if got.is_none() {
            got = n.merge.ask_json_logged(&system, &format!("{user}{SLEEP_SHORTER}"), "merge shorter", log).await?;
        }
        let got = got.unwrap_or_default();
        if let Some(Value::Array(a)) = got.get("notes") {
            merge_notes = a.iter().map(text_of).collect();
        }
        let slot_of = |v: Option<&Value>| -> Option<usize> {
            match text_of(v.unwrap_or(&Value::Null)).as_str() {
                "core" | "0" => Some(0),
                "1" => Some(1),
                "2" => Some(2),
                "3" => Some(3),
                "4" => Some(4),
                _ => None,
            }
        };
        let ripe_ids: Vec<Value> = ripe.iter().map(|p| p.get("id").cloned().unwrap_or(Value::Null)).collect();
        let is_ripe_id = |v: Option<&Value>| ripe_ids.iter().any(|id| same(id, v.unwrap_or(&Value::Null)));
        let tonight: Vec<String> = items
            .iter()
            .filter(|it| it.kind == "person")
            .map(|it| strip(&it.topic).to_lowercase())
            .collect();
        let about_tonight = |name: &str| {
            let nm = strip(name).to_lowercase();
            tonight.iter().any(|t| !nm.is_empty() && !t.is_empty() && (t.contains(&nm) || nm.contains(t.as_str())))
        };
        let section_name = |v: Option<&Value>| strip(strip(&text_or_empty(v)).trim_start_matches('#')).to_string();
        let edits = match got.get("edits") {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        for e in edits {
            let Some(eo) = e.as_object() else { continue };
            let mut why: Option<String> = None;
            let op = eo.get("op").and_then(Value::as_str).unwrap_or("");
            let slot = slot_of(eo.get("file"));
            let ev = sleep_eids(eo.get("evidence"));
            let ops = ["rewrite", "remake", "move_section", "title"];
            match slot {
                None => why = Some("unknown file or op".into()),
                Some(_) if !ops.contains(&op) || !eo.get("op").is_some_and(Value::is_string) => {
                    why = Some("unknown file or op".into())
                }
                Some(slot)
                    if (op == "rewrite" || op == "remake")
                        && (ev.is_empty() || ev.iter().any(|x| !known_all.contains(x)))
                        && !(op == "rewrite" && slot == 0 && is_ripe_id(eo.get("ripe"))) =>
                {
                    why = Some("no evidence, or evidence not in this sleep".into())
                }
                Some(slot) if op == "rewrite" => {
                    let sec = section_name(eo.get("section"));
                    if sec.is_empty() || strip(&text_or_empty(eo.get("text"))).is_empty() {
                        why = Some("rewrite needs a section and text".into());
                    } else if slot == 0 && sec == me && !is_ripe_id(eo.get("ripe")) {
                        why = Some(format!("\"{me}\" changes only from a ripe proposal"));
                    } else if slot == 0 && sec != me && !about_tonight(&sec) {
                        why = Some("nothing new about this person tonight".into());
                    } else if slot != 0 && sleep_dated(&sec) {
                        why = Some("a box section is a matter, not its state on a date: rewrite the matter's section".into());
                    } else {
                        let mut text = Some(text_of(&eo["text"]));
                        if slot == 0 && sec != me {
                            let (t, how) = sleep_settle(n, &core, &sec, text.as_deref().unwrap_or("")).await?;
                            settled.push(json!({"section": sec, "how": how}));
                            text = t;
                        }
                        match text {
                            None => {
                                why = Some("one night overturns one line of a person; the sorting came back unshaped".into())
                            }
                            Some(text) => {
                                let raw = slot == 0 && sec == me;
                                new[slot].body = sleep_set_section(&new[slot].body, &sec, &text, raw);
                                touched.insert((slot, sec));
                            }
                        }
                    }
                }
                Some(slot) if op == "remake" => {
                    let body = strip(&text_or_empty(eo.get("text"))).to_string();
                    let title = strip(&text_or_empty(eo.get("title"))).to_string();
                    if slot == 0 {
                        why = Some("the core is rewritten by sections, not remade".into());
                    } else if body.is_empty() || title.is_empty() {
                        why = Some("remake needs a title and the whole file".into());
                    } else if sleep_dated(&title) || sleep_sections(&body).iter().any(|(h, _)| sleep_dated(h)) {
                        why = Some("a box is a matter, not its state on a date: remake it under the matter's name".into());
                    } else {
                        new[slot].body =
                            if body.starts_with("## ") { body } else { sleep_set_section("", "Notes", &body, false) };
                        new[slot].title = title.chars().take(80).collect();
                        for (h, _) in sleep_sections(&new[slot].body) {
                            touched.insert((slot, h));
                        }
                    }
                }
                Some(slot) if op == "move_section" => {
                    let to = slot_of(eo.get("to"));
                    let sec = section_name(eo.get("section"));
                    match to {
                        Some(to) if to != slot && !((slot == 0 || to == 0) && sec == me) => {
                            let (body, text) = sleep_drop_section(&new[slot].body, &sec);
                            match text {
                                None => why = Some("no such section".into()),
                                Some(text) => {
                                    new[slot].body = body;
                                    new[to].body = sleep_set_section(&new[to].body, &sec, &text, false);
                                }
                            }
                        }
                        _ => why = Some("move needs another file, and the self section stays".into()),
                    }
                }
                Some(slot) => {
                    let title = strip(&text_or_empty(eo.get("text"))).to_string();
                    if slot == 0 || title.is_empty() {
                        why = Some("the core has no title; a title needs text".into());
                    } else if sleep_dated(&title) {
                        why = Some("a box is a matter, not its state on a date: name it for the matter".into());
                    } else {
                        new[slot].title = title.chars().take(80).collect();
                    }
                }
            }
            match why {
                Some(why) => ledger.refused.push(json!({"edit": e, "why": why})),
                None => {
                    if truthy(eo.get("ripe")) {
                        for p in proposals.iter_mut() {
                            if same(p.get("id").unwrap_or(&Value::Null), &eo["ripe"]) {
                                if let Some(p) = p.as_object_mut() {
                                    p.insert("applied".into(), json!(today));
                                }
                            }
                        }
                    }
                    used.extend(ev);
                    applied.push(e);
                }
            }
        }
        let in_core: Vec<String> = sleep_sections(&new[0].body).into_iter().map(|(h, _)| h).filter(|h| h != me).collect();
        for k in SLEEP_BOXES {
            for (h, _) in sleep_sections(&new[k].body) {
                if in_core.contains(&h) {
                    new[k].body = sleep_drop_section(&new[k].body, &h).0;
                    deduped.push(json!({"file": k, "section": h}));
                }
            }
        }
        for (k, h) in &touched {
            let k = *k;
            let Some(t) = section_of(&new[k].body, h).filter(|t| !t.is_empty()) else { continue };
            if k == 0 && h == me {
                continue;
            }
            let where_ = sleep_where(&new, k);
            let numbered = t
                .split('\n')
                .filter(|ln| !strip(ln).is_empty())
                .enumerate()
                .map(|(i, ln)| format!("{}: {ln}", i + 1))
                .collect::<Vec<_>>()
                .join("\n");
            let system = fill(SLEEP_TIDY, &[("being", Arg::Text(being)), ("where", Arg::Text(&where_))]);
            let user = format!("<section heading=\"{h}\">\n{numbered}\n</section>");
            let g = n.merge.ask_json(&system, &user, &format!("tidy {k}:{h}"), log).await?.unwrap_or_default();
            if let Some(text) = sleep_doubt_lines_out(&t, g.get("drop"), count) {
                tidied.push(json!({"file": k, "section": h, "before": count.of(&t), "after": count.of(&text),
                                   "before_text": t, "after_text": text}));
                new[k].body = sleep_set_section(&new[k].body, h, &text, false);
            }
        }
        for k in SLEEP_SLOTS {
            for (h, t) in sleep_sections(&new[k].body) {
                if k == 0 && h == me {
                    continue;
                }
                let ls: Vec<&str> = t.split('\n').filter(|ln| !strip(ln).is_empty()).collect();
                let long = ls.iter().filter(|ln| count.of(ln) > SLEEP_LINE_TOKENS).count();
                let limit = if k == 0 { SLEEP_FOLD_LINES * 2 } else { SLEEP_FOLD_LINES };
                if ls.len() <= limit && long == 0 {
                    continue;
                }
                let why = [
                    if ls.len() > limit { format!("{} lines", ls.len()) } else { String::new() },
                    if long > 0 { format!("{long} lines longer than one idea") } else { String::new() },
                ]
                .into_iter()
                .filter(|x| !x.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
                let where_ = sleep_where(&new, k);
                let system = fill(
                    SLEEP_FOLD_SECTION,
                    &[
                        ("being", Arg::Text(being)),
                        ("where", Arg::Text(&where_)),
                        ("why", Arg::Text(&why)),
                        ("lines", Arg::Num(limit as i128)),
                    ],
                );
                let user = format!("<section heading=\"{h}\">\n{t}\n</section>");
                let g = n.merge.ask_json(&system, &user, &format!("fold {k}:{h}"), log).await?.unwrap_or_default();
                let text = sleep_as_lines(&text_or_empty(g.get("text")));
                let ok_lines = text.split('\n').filter(|ln| !strip(ln).is_empty()).count();
                if ok_lines > 0 && ok_lines <= limit && count.of(&text) < count.of(&t) {
                    folds.push(json!({"file": k, "section": h, "why": why, "before": count.of(&t), "after": count.of(&text)}));
                    new[k].body = sleep_set_section(&new[k].body, &h, &text, false);
                } else {
                    ledger.refused.push(json!({"edit": {"file": k, "op": "fold", "section": h},
                                               "why": format!("fold came back unshaped or longer ({why})")}));
                }
            }
        }
        for k in SLEEP_SLOTS {
            let cap = if k == 0 { s.core_tokens } else { s.box_tokens };
            let used_now = count.of(&new[k].body);
            if used_now <= cap {
                continue;
            }
            sleep_room(n, &mut new, &files, k, used_now, cap, &mut ledger, None).await?;
        }
        sleep_boxes_fit(n, &mut new, &files, &mut ledger);
        let mut seen_now: Vec<Value> = Vec::new();
        let given = match got.get("proposals") {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        for p in given {
            if !p.is_object() || strip(&text_or_empty(p.get("text"))).is_empty() {
                continue;
            }
            let ev = sleep_eids(p.get("evidence"));
            if !ev.iter().any(|x| by_eid.get(x.as_str()).is_some_and(|m| m.mine)) {
                ledger.refused.push(json!({"proposal": p, "why": "a view of the being rests on something it said"}));
                continue;
            }
            let text = strip(&text_of(&p["text"])).to_string();
            let same_as = p.get("same_as").unwrap_or(&Value::Null);
            let found = proposals
                .iter()
                .position(|q| same(q.get("id").unwrap_or(&Value::Null), same_as))
                .or_else(|| proposals.iter().position(|q| counted_here(q) && q.get("text") == Some(&json!(text))));
            match found {
                Some(i) => {
                    let id = proposals[i].get("id").cloned().unwrap_or(Value::Null);
                    if !seen_now.iter().any(|x| same(x, &id)) {
                        let q = &mut proposals[i];
                        if !counted_here(q) {
                            let sleeps = q.get("sleeps").cloned().unwrap_or(Value::Null);
                            let sleeps = match (sleeps.as_i64(), sleeps.as_f64()) {
                                (Some(i), _) => json!(i.saturating_add(1)),
                                (None, Some(f)) => json!(f + 1.0),
                                _ => json!(1),
                            };
                            if let Some(q) = q.as_object_mut() {
                                q.insert("sleeps".into(), sleeps);
                                q.insert("last_day".into(), json!(today));
                                q.insert("text".into(), json!(text));
                                q.insert("span_end".into(), json!(span_end));
                            }
                        }
                        seen_now.push(id);
                    }
                }
                None => {
                    let pid = format!("p{}", proposals.len() + 1);
                    proposals.push(json!({"id": pid, "text": text, "sleeps": 1, "first": now,
                                          "first_day": today, "last_day": today, "span_end": span_end}));
                    seen_now.push(json!(pid));
                }
            }
        }
    }
    notes.insert("proposals".into(), Value::Array(proposals.clone()));
    let ev: Vec<String> =
        used.iter().filter_map(|e: &String| by_eid.get(e.as_str()).map(|m| m.mid.clone())).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    let written = sleep_write_files(n, &sid, &files, &mut new, &ev, &day, &mut ledger).await?;
    if notes != before {
        client
            .post_idem(
                "/api/v1/memory/self/notes/write",
                json!({"user_id": store, "base_version": notes_version, "notes": notes}),
                None,
            )
            .await?;
    }
    let ended = sleep_done_flow(n, &sid).await?;
    let secs = round_to(t0.elapsed().as_secs_f64(), 1);
    let slots: Vec<String> = written.keys().map(i64::to_string).collect();
    n.say(&format!(
        "done in {secs:?}s · {} items · {} edits, {} refused · {} who-said-what marks · written {}",
        items.len(),
        applied.len(),
        ledger.refused.len(),
        mixed.len(),
        if slots.is_empty() { "none".to_string() } else { format!("[{}]", slots.join(", ")) }
    ));
    let log = log.take();
    let total = |key: &str| -> i64 { log.iter().map(|x| x.get(key).and_then(Value::as_i64).unwrap_or(0)).fold(0, i64::saturating_add) };
    let (llm_in, llm_out) = (total("in"), total("out"));
    let written: Map<String, Value> = written.into_iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
    out.insert("readers".into(), json!(parts.len()));
    out.insert("combine_rounds".into(), json!(rounds));
    out.insert("items".into(), Value::Array(items.iter().map(Item::value).collect()));
    out.insert("items_dropped_by_check".into(), json!(dropped));
    out.insert("applied".into(), Value::Array(applied));
    out.insert("refused".into(), Value::Array(ledger.refused));
    out.insert("written".into(), Value::Object(written));
    out.insert("proposals".into(), Value::Array(proposals));
    out.insert("ripe".into(), Value::Array(ripe.iter().map(|p| p.get("id").cloned().unwrap_or(Value::Null)).collect()));
    out.insert("notes".into(), json!(merge_notes));
    out.insert("folds".into(), Value::Array(folds));
    out.insert("settled".into(), Value::Array(settled));
    out.insert("deduped".into(), Value::Array(deduped));
    out.insert("tidied".into(), Value::Array(tidied));
    out.insert("rooms".into(), Value::Array(ledger.rooms));
    out.insert("mixed".into(), Value::Array(mixed));
    out.insert("log".into(), Value::Array(log));
    out.insert("secs".into(), json!(secs));
    out.insert("llm_in".into(), json!(llm_in));
    out.insert("llm_out".into(), json!(llm_out));
    out.insert("done".into(), ended);
    Ok(Value::Object(out))
}

async fn sleep_estimate_flow(run: &Run<'_>, prices: Option<(f64, f64)>) -> Result<Value, WosError> {
    let (head, rows) = sleep_read_all(run).await?;
    let view = sleep_view(&run.client.post("/api/v1/memory/self", json!({"user_id": run.store})).await?);
    let (mems, _) = sleep_memories(&rows, &run.being, "", SleepLineTime::CreatedAt, &run.count);
    let parts = sleep_parts(&mems, run.s.part_tokens);
    let tokens = token_sum(&mems);
    let titles = SLEEP_BOXES
        .iter()
        .map(|&k| format!("{k}: {}", if view[k].title.is_empty() { "(empty)" } else { &view[k].title }))
        .collect::<Vec<_>>()
        .join("\n");
    let core = view[0].body.as_deref().unwrap_or("");
    let frame = [SLEEP_DIGEST, core, titles.as_str()].into_iter().fold(0, |n: u64, t| n.saturating_add(run.count.of(t)));
    let sent = (u128::from(tokens) + parts.len() as u128 * u128::from(frame)) * u128::from(SLEEP_IN_PER_100);
    let sent = u64::try_from(sent.div_ceil(100)).unwrap_or(u64::MAX);
    let written = u64::try_from((u128::from(tokens) * u128::from(SLEEP_OUT_PER_100)).div_ceil(100)).unwrap_or(u64::MAX);
    let prices = prices.unwrap_or(SLEEP_ESTIMATE_PRICES);
    let cost = round_to((sent as f64 * prices.0 + written as f64 * prices.1) * (1.0 + SLEEP_MARGIN) / 1_000_000.0, 6);
    let sid = head.get("sleep_id").and_then(Value::as_str);
    Ok(json!({
        "sleep_id": sid, "memories": mems.len(), "tokens": tokens, "parts": parts.len(),
        "readers": run.s.readers.min(parts.len() as u64), "input_tokens": sent, "output_tokens": written, "cost": cost,
        "basis": {"sleep_version": SLEEP_VERSION, "model": SLEEP_ESTIMATE_MODEL, "effort": "high", "prompt_caching": false,
                  "prices": [prices.0, prices.1], "margin": SLEEP_MARGIN},
        "sleepy": run.s.sleepy(tokens - SLEEP_LINE_FRAME * mems.len() as u64),
    }))
}

fn sleep_asker(client: &Client, caller: &Arc<dyn ModelCaller>, what: &str, max_tokens: u32) -> Result<Asker, WosError> {
    if let Some(base) = caller.base_url() {
        provider_base(base, Some(&client.base_url))?;
    }
    let model = super::check_model_name(caller.model().unwrap_or(SLEEP_MODEL), &format!("{what}.model"))?;
    Ok(Asker { caller: caller.clone(), model, max_tokens })
}

fn required(v: &str, name: &str) -> Result<String, WosError> {
    match strip(v) {
        "" => Err(refused(format!("{name} is required."))),
        t => Ok(t.to_string()),
    }
}

fn sleep_run<'a>(
    client: &'a Client,
    user_id: Option<&str>,
    being: &str,
    settings: SleepSettings,
    since: Option<&str>,
    sleep_id: Option<&str>,
    count_tokens: Option<CountTokens>,
) -> Result<Run<'a>, WosError> {
    let store = client.uid(user_id)?;
    let being = required(being, "being")?;
    settings.check()?;
    let since = since.map(|s| required(s, "since")).transpose()?;
    let sleep_id = sleep_id.map(|s| required(s, "sleep_id")).transpose()?;
    Ok(Run { client, store, being, s: settings, count: Count(count_tokens), since, sleep_id })
}

pub(crate) async fn sleep(
    client: &Client,
    user_id: Option<&str>,
    llm: Arc<dyn ModelCaller>,
    opts: &SleepOpts,
) -> Result<Value, SleepError> {
    let run = sleep_run(
        client,
        user_id,
        &opts.being,
        opts.settings,
        opts.since.as_deref(),
        opts.sleep_id.as_deref(),
        opts.count_tokens.clone(),
    )?;
    if let Some(today) = &opts.today {
        let day: String = today.chars().take(10).collect();
        let rest: Vec<char> = today.chars().skip(10).collect();
        let clock = rest.is_empty()
            || (rest.len() == 6
                && rest[0] == ' '
                && rest[3] == ':'
                && [1, 2, 4, 5].iter().all(|i| digit(rest[*i]).is_some()));
        if sleep_date(&day).is_none() || !clock {
            return Err(refused(format!(
                "today must be \"YYYY-MM-DD HH:MM\" or \"YYYY-MM-DD\", the sleep's day, got {}.",
                serde_json::to_string(today).unwrap_or_default()
            ))
            .into());
        }
    }
    let assistant = opts.assistant.as_deref().map(|a| required(a, "assistant")).transpose()?;
    let tokens = check_whole(u64::from(opts.max_tokens), "max_tokens", 1, u64::from(MODEL_MAX_TOKENS))? as u32;
    let read = sleep_asker(client, &llm, "llm", tokens)?;
    let merge = match &opts.merge_llm {
        None => Asker { caller: read.caller.clone(), model: read.model.clone(), max_tokens: tokens },
        Some(m) => sleep_asker(client, m, "merge_llm", tokens)?,
    };
    let night = Night {
        run,
        read,
        merge,
        log: Log::default(),
        prompt: opts.prompt.clone(),
        assistant,
        today: opts.today.clone(),
        done: opts.done,
        line_time: opts.line_time,
        say: opts.progress.clone(),
    };
    sleep_night(&night).await
}

pub(crate) async fn sleep_estimate(client: &Client, user_id: Option<&str>, opts: &SleepEstimateOpts) -> Result<Value, WosError> {
    for (name, v) in [("input_price", opts.input_price), ("output_price", opts.output_price)] {
        if let Some(v) = v.filter(|v| !v.is_finite() || *v < 0.0) {
            return Err(refused(format!("{name} must be a price in USD per million tokens, 0 or more, got {v}.")));
        }
    }
    let prices = match (opts.input_price, opts.output_price) {
        (Some(i), Some(o)) => Some((i, o)),
        (None, None) => None,
        _ => return Err(refused("Give input_price and output_price together, or neither for Claude Sonnet 5.5's.")),
    };
    let run = sleep_run(client, user_id, SLEEP_BEING, opts.settings, opts.since.as_deref(), None, opts.count_tokens.clone())?;
    sleep_estimate_flow(&run, prices).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{caller_fn, openai_caller, BoxFuture, ErrorKind, ModelReply, OpenAiCallerOpts, SleepFileWrite, SleepReadOpts, ToolFormat};
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::io::{Read, Write};
    use std::sync::Mutex;

    const KEY: &str = "wos-test-xxxxxxxxxx";

    fn sha256(data: &[u8]) -> String {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
            0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
            0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
            0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
            0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
            0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
            0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut h: [u32; 8] =
            [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
        let mut msg = data.to_vec();
        msg.push(0x80);
        while msg.len() % 64 != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
        for chunk in msg.chunks(64) {
            let mut w = [0u32; 64];
            for i in 0..16 {
                w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
            }
            let mut v = h;
            for i in 0..64 {
                let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
                let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
                let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
                let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
                let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
                let t2 = s0.wrapping_add(maj);
                v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
            }
            for (a, b) in h.iter_mut().zip(v) {
                *a = a.wrapping_add(b);
            }
        }
        h.iter().map(|x| format!("{x:08x}")).collect()
    }

    fn py_dumps(v: &Value) -> String {
        match v {
            Value::Null => "null".into(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) if n.is_f64() => float_repr(n.as_f64().unwrap_or(0.0)),
            Value::Number(n) => n.to_string(),
            Value::String(s) => serde_json::to_string(s).unwrap(),
            Value::Array(a) => format!("[{}]", a.iter().map(py_dumps).collect::<Vec<_>>().join(", ")),
            Value::Object(o) => {
                let mut keys: Vec<&String> = o.keys().collect();
                keys.sort();
                let fields: Vec<String> =
                    keys.iter().map(|k| format!("{}: {}", serde_json::to_string(k).unwrap(), py_dumps(&o[*k]))).collect();
                format!("{{{}}}", fields.join(", "))
            }
        }
    }

    fn digest(v: &Value) -> String {
        sha256(py_dumps(v).as_bytes())
    }

    #[test]
    fn sha256_is_sha256() {
        assert_eq!(sha256(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let long = "a".repeat(1_000);
        assert_eq!(sha256(long.as_bytes()), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
    }

    const PROMPT: &str = "I am the assistant in these conversations with this user.\n## Style\nShort answers.";
    const DAYS: [&str; 4] = ["2026-09-01", "2026-09-15", "2026-10-01", "2026-10-05"];

    fn filler() -> String {
        "the garden path and the morning light ".repeat(46)
    }

    fn big() -> String {
        format!("- {}", "a long line of plant names ".repeat(1700))
    }

    fn settings() -> SleepSettings {
        SleepSettings { box_tokens: 10_000, part_tokens: 50_000, readers: 2, group: 2, ..Default::default() }
    }

    fn night_rows(n: usize, day: &str) -> Vec<Value> {
        (0..if n == 0 { 300 } else { 20 })
            .map(|i: usize| {
                let secs = 9 * 3600 + i * 20 + (i / 50) * 3600;
                let words = if n == 0 { filler() } else { "short words about the day".to_string() };
                let text = format!("Night {} line {i}: {words}", n + 1);
                json!({"id": format!("m{}-{i:04}", n + 1), "content": text,
                       "created_at": format!("{day}T{:02}:{:02}:{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60),
                       "speaker": (["Ana", "Kai"][i % 2]),
                       "tokens": sleep_tokens(&text)})
            })
            .collect()
    }

    const SUMMARY_KEYS: [&str; 16] = [
        "seeded", "readers", "combine_rounds", "items", "items_dropped_by_check", "applied", "refused", "written", "ripe",
        "notes", "folds", "settled", "deduped", "tidied", "rooms", "mixed",
    ];

    fn night_summary(res: &Value) -> Value {
        Value::Object(SUMMARY_KEYS.iter().map(|k| (k.to_string(), res.get(*k).cloned().unwrap_or(Value::Null))).collect())
    }

    const REFERENCE_CALLS: usize = 27;
    const REFERENCE_CALLS_DIGEST: &str = "66cc5bf1dbde699a22947dc8e1b55991ec0fd5f410bcf6bcc27f3f171144cf4a";
    const REFERENCE_NIGHTS_DIGEST: &str = "bbedfa7a042245922190822e61443b41086ee03466b8f61da790cae7bb0184b7";
    const REFERENCE_STATE_DIGEST: &str = "e3ebd0a08e7d1eae4e93b0a26bc7cdfec5c2cd5677d46e634ad367351117033b";

    fn between<'a>(text: &'a str, from: &str, to: &str) -> &'a str {
        let rest = &text[text.find(from).unwrap() + from.len()..];
        &rest[..rest.find(to).unwrap()]
    }

    fn lines_of(user: &str) -> Vec<(String, String, String)> {
        between(user, "<memories>\n", "\n</memories>")
            .split('\n')
            .map(|ln| {
                let p: Vec<&str> = ln.splitn(4, " · ").collect();
                (p[0].to_string(), p[1].to_string(), p[2].to_string())
            })
            .collect()
    }

    fn section_in(user: &str) -> (String, String) {
        let heading = between(user, "<section heading=\"", "\">\n").to_string();
        let body = user.split_once('\n').unwrap().1;
        (heading, body[..body.rfind("\n</section>").unwrap()].to_string())
    }

    fn read_answer(day: &str, k: usize, lines: &[(String, String, String)], first: bool) -> String {
        let users: Vec<&str> = lines.iter().filter(|l| l.2 == "user").map(|l| l.0.as_str()).collect();
        let mine: Vec<&str> = lines.iter().filter(|l| l.2 != "user").map(|l| l.0.as_str()).collect();
        let v = if day == DAYS[0] {
            match k {
                1 => json!({"items": [
                    {"kind": "person", "topic": "Jamie", "text": "Jamie is a close friend who likes gardening.",
                     "evidence": [users[0], users[1]]},
                    {"kind": "work", "topic": "Garden project", "text": "The garden project is planting tomatoes.",
                     "evidence": [users[0], mine[0]]},
                    {"kind": "self", "topic": "me", "text": "I like short answers.", "evidence": [mine[0], mine[1]]},
                    {"kind": "self", "topic": "me", "text": "Not mine to say.", "evidence": [users[0], users[1]]},
                    {"kind": "work", "topic": "Seeds", "text": "One line is not enough.", "evidence": [users[2]]},
                    {"kind": "mood", "topic": "x", "text": "Not a kind.", "evidence": [users[0], users[1]]},
                ], "mixed": [
                    {"line": users[1], "kind": "echo", "source": mine[0], "why": "gave back my words as theirs"},
                    {"line": users[2], "kind": "planted", "why": "never said by me"},
                    {"line": mine[2], "kind": "speaker", "why": "reads as the person's"},
                    {"line": users[3], "kind": "wrong_fact", "why": "a mistake, not who said it"},
                ]}),
                2 => json!({"items": [{"kind": "person", "topic": "Jamie", "text": "Jamie waters the garden on weekends.",
                                       "evidence": ["e1", "e3"]}],
                            "mixed": [{"line": "e2", "kind": "speaker", "why": "reads as the person's"}]}),
                _ if first => return "I could not read this part.".into(),
                _ => json!({"items": [{"kind": "work", "topic": "Garden project", "text": "Tomatoes are planted; watering daily.",
                                       "evidence": [users[0], mine[0]]}]}),
            }
        } else if day == DAYS[1] {
            json!({"items": [
                {"kind": "person", "topic": "Jamie", "text": "Jamie prefers flowers now.", "evidence": [users[0], users[1]]},
                {"kind": "person", "topic": "Alex", "text": "Alex is Jamie's sibling.", "evidence": [users[0], users[2]]},
                {"kind": "work", "topic": "Garden project", "text": "The fence is built.", "evidence": [users[1], mine[0]]},
                {"kind": "self", "topic": "me", "text": "I keep answers short.", "evidence": [mine[0], mine[1]]},
            ], "mixed": []})
        } else if day == DAYS[2] {
            json!({"items": [
                {"kind": "person", "topic": "Alex", "text": "Alex visits every month.", "evidence": [users[0], users[1]]},
                {"kind": "work", "topic": "Big list", "text": "A long list of plants.", "evidence": [users[1], users[2]]},
                {"kind": "self", "topic": "me", "text": "I still keep answers short.", "evidence": [mine[0], mine[1]]},
            ], "mixed": []})
        } else {
            json!({"items": [], "mixed": []})
        };
        v.to_string()
    }

    fn combine_answer(flat: &[Value]) -> String {
        let mut merged: Vec<((String, String), Value)> = Vec::new();
        for it in flat {
            let key = (it["kind"].as_str().unwrap().to_string(), it["topic"].as_str().unwrap().to_string());
            match merged.iter_mut().find(|(k, _)| *k == key) {
                Some((_, m)) => {
                    m["text"] = it["text"].clone();
                    let mut ev: Vec<String> =
                        m["evidence"].as_array().unwrap().iter().chain(it["evidence"].as_array().unwrap()).map(|e| e.as_str().unwrap().to_string()).collect();
                    ev.sort_by_key(|e| eid_number(e));
                    ev.dedup();
                    m["evidence"] = json!(ev);
                }
                None => merged.push((key, json!({"kind": it["kind"], "topic": it["topic"], "text": it["text"],
                                                 "evidence": it["evidence"]}))),
            }
        }
        json!({"items": merged.into_iter().map(|(_, v)| v).collect::<Vec<_>>()}).to_string()
    }

    fn merge_answer(day: &str, user: &str) -> String {
        let items: Vec<Value> = serde_json::from_str(between(user, "<items>\n", "\n</items>")).unwrap();
        let ev = |kind: &str, topic: &str| -> Value {
            items.iter().find(|it| it["kind"] == kind && it["topic"] == topic).map_or(json!([]), |it| it["evidence"].clone())
        };
        let v = if day == DAYS[0] {
            let (person, work, me) = (ev("person", "Jamie"), ev("work", "Garden project"), ev("self", "me"));
            json!({"edits": [
                {"file": "core", "op": "rewrite", "section": "Jamie", "evidence": person,
                 "text": "- Jamie is a close friend.\n- Jamie likes gardening.\n- Jamie waters the garden on weekends."},
                {"file": "core", "op": "rewrite", "section": "Robin", "text": "Robin is new.", "evidence": person},
                {"file": "core", "op": "rewrite", "section": "Me", "text": "I am different now.", "evidence": person},
                {"file": "1", "op": "title", "text": "Garden"},
                {"file": "1", "op": "rewrite", "section": "Garden project", "evidence": work,
                 "text": "- Planting tomatoes.\n- Watering daily.\n- Status: not yet confirmed.\n- Next: build a fence.\n- Soil is clay.\n- Bought seeds."},
                {"file": "2", "op": "remake", "title": "Trip 2026-09-01", "text": "## Trip\n- Plan the route.", "evidence": work},
                {"file": "3", "op": "remake", "title": "Reading list", "text": "Books to read next.", "evidence": work},
                {"file": "core", "op": "move_section", "section": "Nobody", "to": "2"},
                {"file": "core", "op": "delete"},
                {"file": "1", "op": "rewrite", "section": "Other", "text": "x", "evidence": ["e99999"]},
            ], "proposals": [
                {"text": "I prefer short answers.", "same_as": null, "evidence": me},
                {"text": "From the person only.", "same_as": null, "evidence": person},
            ], "notes": ["1: what the garden needs"]})
        } else if day == DAYS[1] {
            json!({"edits": [
                {"file": "core", "op": "rewrite", "section": "Jamie", "evidence": ev("person", "Jamie"),
                 "text": "- Jamie is my oldest friend.\n- Jamie prefers flowers now.\n- Jamie waters on Sundays."},
                {"file": "core", "op": "rewrite", "section": "Alex", "text": "- Alex is Jamie's sibling.",
                 "evidence": ev("person", "Alex")},
                {"file": "1", "op": "rewrite", "section": "Garden project", "text": "- Tomatoes planted.\n- Fence built.",
                 "evidence": ev("work", "Garden project")},
            ], "proposals": [{"text": "I prefer short answers, always.", "same_as": "p1", "evidence": ev("self", "me")}],
               "notes": []})
        } else if day == DAYS[2] {
            json!({"edits": [
                {"file": "core", "op": "move_section", "section": "Alex", "to": "3"},
                {"file": "4", "op": "rewrite", "section": "Big list", "text": big(), "evidence": ev("work", "Big list")},
                {"file": "4", "op": "title", "text": "Big list"},
                {"file": "2", "op": "remake", "title": "People notes", "text": "## Jamie\n- Dup.\n## Neighbors\n- Kind people.",
                 "evidence": ev("work", "Big list")},
                {"file": "core", "op": "rewrite", "section": "Jamie", "text": "- Changed.", "evidence": ev("person", "Alex")},
            ], "proposals": [{"text": "I prefer short answers, still.", "same_as": "p1", "evidence": ev("self", "me")}],
               "notes": []})
        } else if !user.contains("Your last answer was too long") {
            return r#"{"edits": [{"file": "core", "op": "rewrite", "text": "cut off"#.into();
        } else {
            json!({"edits": [{"file": "core", "op": "rewrite", "section": "Me", "ripe": "p1",
                              "text": "I am the assistant in these conversations with this user.\nI keep answers short."}],
                   "proposals": [], "notes": []})
        };
        v.to_string()
    }

    fn scripted(system: &str, user: &str, first: bool) -> String {
        if system.contains(" you read part ") {
            let k: usize = between(system, "you read part ", " of").parse().unwrap();
            let lines = lines_of(user);
            let day = lines[0].1[..10].to_string();
            return read_answer(&day, k, &lines, first);
        }
        if system.contains("Several readers each read one part") {
            let flat: Vec<Value> = serde_json::from_str(user).unwrap();
            return combine_answer(&flat);
        }
        if system.contains(", finishing.") {
            let day = &between(system, "Today is ", ".")[..10];
            return merge_answer(day, user);
        }
        if system.contains("Tonight you rewrote how") {
            return json!({"add": ["Jamie prefers flowers now."], "op": "replace", "line": 2,
                          "text": "Jamie likes flowers more than vegetables."})
            .to_string();
        }
        if system.contains("lines that are only a note of doubt") {
            let drop: Vec<u64> = section_in(user)
                .1
                .split('\n')
                .filter_map(|ln| ln.split_once(": "))
                .filter(|(_, t)| t.contains("not yet confirmed"))
                .map(|(n, _)| n.parse().unwrap())
                .collect();
            return json!({"drop": drop}).to_string();
        }
        if system.contains("has grown:") {
            let (heading, text) = section_in(user);
            let folded = if heading == "Big list" {
                format!("{text} and more")
            } else {
                text.split('\n').take(3).collect::<Vec<_>>().join("\n")
            };
            return json!({"text": folded}).to_string();
        }
        if system.contains("there is no room") {
            return json!({"text": "## Big list\n- Trimmed to what matters."}).to_string();
        }
        panic!("a call the script does not know: {}", &system[..80]);
    }

    #[derive(Default)]
    struct Model {
        state: Mutex<ModelState>,
        named: Option<&'static str>,
        settle: Option<String>,
    }

    #[derive(Default)]
    struct ModelState {
        seen: HashMap<String, usize>,
        calls: Vec<String>,
        texts: Vec<(String, String)>,
        models: Vec<String>,
        fail: Vec<ModelCallError>,
    }

    impl Model {
        fn failing(fail: Vec<ModelCallError>) -> Arc<Self> {
            let m = Model::default();
            m.state.lock().unwrap().fail = fail;
            Arc::new(m)
        }

        fn calls(&self) -> Vec<String> {
            self.state.lock().unwrap().calls.clone()
        }

        fn texts(&self) -> Vec<(String, String)> {
            self.state.lock().unwrap().texts.clone()
        }

        fn models(&self) -> Vec<String> {
            self.state.lock().unwrap().models.clone()
        }

        fn digest(&self) -> String {
            let mut calls = self.calls();
            calls.sort();
            sha256(calls.join("\n").as_bytes())
        }
    }

    impl ModelCaller for Model {
        fn call<'a>(&'a self, c: &'a ModelCall) -> BoxFuture<'a, Result<ModelReply, ModelCallError>> {
            Box::pin(async move {
                assert!(c.purpose == CallPurpose::Sleep && c.system.len() == 1 && c.max_tokens == 16_000, "{c:?}");
                let (system, user) = (c.system[0].clone(), c.user.clone());
                let first = {
                    let mut st = self.state.lock().unwrap();
                    let key = sha256(format!("{system}\u{0}{user}").as_bytes());
                    let seen = st.seen.entry(key.clone()).or_insert(0);
                    *seen += 1;
                    let first = *seen == 1;
                    st.calls.push(key);
                    st.texts.push((system.clone(), user.clone()));
                    st.models.push(c.model.clone());
                    if !st.fail.is_empty() {
                        return Err(st.fail.remove(0));
                    }
                    first
                };
                let text = match &self.settle {
                    Some(s) if system.contains("Tonight you rewrote how") => s.clone(),
                    _ => scripted(&system, &user, first),
                };
                let usage = json!({"input_tokens": sleep_tokens(&user), "output_tokens": sleep_tokens(&text)});
                Ok(ModelReply { text, stop: "end".into(), usage: Some(usage) })
            })
        }

        fn model(&self) -> Option<&str> {
            self.named
        }
    }

    struct Seen {
        path: String,
        body: Value,
        headers: HashMap<String, String>,
    }

    #[derive(Clone)]
    struct Version {
        version: i64,
        title: String,
        body: String,
        tokens: i64,
        evidence: Value,
        day: Value,
        sleep_id: Value,
    }

    struct SleepSpan {
        since: usize,
        until: usize,
        handed: HashSet<String>,
    }

    struct Service {
        rows: Vec<Value>,
        last: usize,
        sleeps: BTreeMap<String, SleepSpan>,
        slots: [Vec<Version>; 5],
        notes: Option<Value>,
        notes_version: i64,
        caps: [i64; 5],
        scale: i64,
        room_message: bool,
        fail: Vec<(String, u16, Value)>,
        seen: Vec<Seen>,
        local_now: String,
        assistant: Value,
    }

    impl Default for Service {
        fn default() -> Self {
            Service {
                rows: Vec::new(),
                last: 0,
                sleeps: BTreeMap::new(),
                slots: Default::default(),
                notes: None,
                notes_version: 0,
                caps: [70_000, 20_000, 20_000, 20_000, 20_000],
                scale: 1,
                room_message: true,
                fail: Vec::new(),
                seen: Vec::new(),
                local_now: "2026-10-20T09:30:00+09:00".into(),
                assistant: json!("Kai"),
            }
        }
    }

    fn coded(status: u16, code: &str, message: &str) -> (u16, Value) {
        (status, json!({"error": {"code": code, "message": message}}))
    }

    impl Service {
        fn count(&self, text: &str) -> i64 {
            sleep_tokens(text) as i64 * self.scale
        }

        fn latest(&self, k: usize) -> Option<&Version> {
            self.slots[k].last()
        }

        fn state(&self) -> Value {
            let slots: Map<String, Value> = (0..5)
                .filter(|k| !self.slots[*k].is_empty())
                .map(|k| {
                    let vs = self.slots[k]
                        .iter()
                        .map(|v| json!({"version": v.version, "title": v.title, "body": v.body, "evidence": v.evidence, "day": v.day}))
                        .collect();
                    (k.to_string(), Value::Array(vs))
                })
                .collect();
            let notes = self.notes.clone().unwrap_or(json!({}));
            json!({"slots": slots, "proposals": notes.get("proposals").cloned().unwrap_or(json!([])),
                   "mixed": notes.get("mixed").cloned().unwrap_or(json!([]))})
        }

        fn entry(&self, k: usize, v: Option<&Version>) -> Value {
            match v {
                None => json!({"slot": k, "title": "", "version": 0, "tokens": 0, "written_at": null, "day": null}),
                Some(v) => json!({"slot": k, "title": v.title, "version": v.version, "tokens": v.tokens,
                                  "written_at": "2026-10-20T00:00:00Z", "day": v.day}),
            }
        }

        fn handle(&mut self, path: &str, body: Value, headers: HashMap<String, String>) -> (u16, Value) {
            self.seen.push(Seen { path: path.to_string(), body: body.clone(), headers });
            if let Some(i) = self.fail.iter().position(|(p, _, _)| p == path) {
                let (_, status, payload) = self.fail.remove(i);
                return (status, payload);
            }
            let route = path.trim_start_matches("/api/v1/memory/");
            if let Some(query) = route.strip_prefix("settings?") {
                let user = query.strip_prefix("user_id=").unwrap_or("default");
                return (200, json!({"user_id": user, "time_zone": null, "default_speakers": {"user": "Ana", "assistant": self.assistant},
                                    "speakers": ["Ana"], "history": []}));
            }
            let takes: Option<&[&str]> = match route {
                "sleep/done" => Some(&["user_id", "sleep_id"]),
                "self" => Some(&["user_id"]),
                "self/open" => Some(&["user_id", "slot", "version"]),
                "self/history" => Some(&["user_id", "slot"]),
                "self/write" => Some(&["user_id", "sleep_id", "writes"]),
                "self/notes" => Some(&["user_id"]),
                "self/notes/write" => Some(&["user_id", "base_version", "notes"]),
                _ => None,
            };
            let b = body.as_object().cloned().unwrap_or_default();
            let mut extra: Vec<String> =
                takes.map_or(Vec::new(), |t| b.keys().filter(|k| !t.contains(&k.as_str())).cloned().collect());
            if route == "self/write" {
                let known = ["slot", "base_version", "title", "body", "evidence", "day"];
                for w in b["writes"].as_array().into_iter().flatten() {
                    extra.extend(w.as_object().unwrap().keys().filter(|k| !known.contains(&k.as_str())).cloned());
                }
            }
            if route == "self/notes/write" {
                extra.extend(b["notes"].as_object().into_iter().flatten().map(|(k, _)| k.clone()).filter(|k| k != "proposals" && k != "mixed"));
            }
            if !extra.is_empty() {
                return coded(400, "invalid_request", &format!("unknown fields {extra:?}"));
            }
            let user = b["user_id"].clone();
            match route {
                "sleep" => {
                    let sid = match b.get("sleep_id").and_then(Value::as_str) {
                        Some(s) => s.to_string(),
                        None => {
                            let sid = format!("s-{}", self.sleeps.len() + 1);
                            self.sleeps.insert(sid.clone(), SleepSpan { since: self.last, until: self.rows.len(), handed: HashSet::new() });
                            sid
                        }
                    };
                    let (since, until) = (self.sleeps[&sid].since, self.sleeps[&sid].until);
                    let mut i = b.get("cursor").and_then(|c| c.get("after")).and_then(Value::as_u64).map_or(since, |x| x as usize);
                    let budget = b.get("budget_tokens").and_then(Value::as_i64).unwrap_or(5000);
                    let (mut page, mut used) = (Vec::new(), 0);
                    while i < until {
                        let t = self.rows[i]["tokens"].as_i64().unwrap();
                        if !page.is_empty() && used + t > budget {
                            break;
                        }
                        page.push(self.rows[i].clone());
                        used += t;
                        i += 1;
                    }
                    let span = self.sleeps.get_mut(&sid).unwrap();
                    span.handed.extend(page.iter().map(|x| x["id"].as_str().unwrap().to_string()));
                    let exhausted = i >= until;
                    (200, json!({"user_id": user, "sleep_id": sid, "count": page.len(), "memories": page, "tokens": used,
                                 "exhausted": exhausted, "next_cursor": if exhausted { Value::Null } else { json!({"after": i}) },
                                 "clock": {"local_now": self.local_now}}))
                }
                "sleep/done" => {
                    let sid = b["sleep_id"].as_str().unwrap();
                    let until = self.sleeps[sid].until;
                    let changed = until > self.last;
                    self.last = self.last.max(until);
                    (200, json!({"user_id": user, "sleep_id": sid, "changed": changed, "last_sleep": {"until": self.last}}))
                }
                "self" => {
                    let mut core = self.entry(0, self.latest(0));
                    core.as_object_mut().unwrap().remove("slot");
                    core["body"] = json!(self.latest(0).map_or("", |v| v.body.as_str()));
                    let files: Vec<Value> = (1..5).map(|k| self.entry(k, self.latest(k))).collect();
                    (200, json!({"user_id": user, "core": core, "files": files, "notes_version": self.notes_version}))
                }
                "self/open" => {
                    let k = b["slot"].as_u64().unwrap() as usize;
                    let vs = &self.slots[k];
                    let v = match b.get("version").and_then(Value::as_i64) {
                        Some(n) if n < 1 || n as usize > vs.len() => return coded(404, "self_version_not_found", "no such version"),
                        Some(n) => Some(&vs[n as usize - 1]),
                        None => vs.last(),
                    };
                    let mut out = self.entry(k, v);
                    let o = out.as_object_mut().unwrap();
                    o.insert("user_id".into(), user);
                    o.insert("latest_version".into(), json!(vs.len()));
                    o.insert("body".into(), json!(v.map_or("", |v| v.body.as_str())));
                    o.insert("evidence".into(), v.map_or(json!([]), |v| v.evidence.clone()));
                    o.insert("sleep_id".into(), v.map_or(Value::Null, |v| v.sleep_id.clone()));
                    (200, out)
                }
                "self/history" => {
                    let k = b["slot"].as_u64().unwrap() as usize;
                    let vs = &self.slots[k];
                    let versions: Vec<Value> = vs
                        .iter()
                        .map(|v| {
                            let mut e = self.entry(k, Some(v));
                            e["evidence_count"] = json!(v.evidence.as_array().map_or(0, Vec::len));
                            e["sleep_id"] = v.sleep_id.clone();
                            e
                        })
                        .collect();
                    (200, json!({"user_id": user, "slot": k, "latest_version": vs.len(), "count": vs.len(), "versions": versions}))
                }
                "self/write" => {
                    let sid = b["sleep_id"].as_str().unwrap_or("").to_string();
                    let Some(span) = self.sleeps.get(&sid) else {
                        return coded(404, "sleep_not_found", "no sleep of this store has that sleep_id");
                    };
                    let writes = b["writes"].as_array().cloned().unwrap_or_default();
                    for w in &writes {
                        let ev = w.get("evidence").and_then(Value::as_array).cloned().unwrap_or_default();
                        if !ev.iter().all(|e| span.handed.contains(e.as_str().unwrap())) {
                            return coded(400, "self_evidence_unknown", "evidence the sleep was not handed");
                        }
                        let k = w["slot"].as_u64().unwrap() as usize;
                        if self.latest(k).map_or(0, |v| v.version) != w["base_version"].as_i64().unwrap() {
                            return coded(409, "self_version_conflict", "the file changed since it was read");
                        }
                        let n = self.count(w["body"].as_str().unwrap());
                        if n > self.caps[k] {
                            let named = if k == 0 { "slot 0 (the core)".to_string() } else { format!("slot {k} (file {k})") };
                            let message = if self.room_message {
                                format!(
                                    "{named} would hold {n} tokens, over its cap of {}: there is no room. Make room (fold, or \
                                     move from the core to a file) and write again. Nothing was written.",
                                    self.caps[k]
                                )
                            } else {
                                "there is no room".to_string()
                            };
                            return coded(400, "self_no_room", &message);
                        }
                    }
                    let mut out = Vec::new();
                    for w in &writes {
                        let k = w["slot"].as_u64().unwrap() as usize;
                        let body = w["body"].as_str().unwrap().to_string();
                        let v = Version {
                            version: w["base_version"].as_i64().unwrap() + 1,
                            title: w.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
                            tokens: self.count(&body),
                            body,
                            evidence: w.get("evidence").cloned().unwrap_or(json!([])),
                            day: w.get("day").cloned().unwrap_or(Value::Null),
                            sleep_id: json!(sid),
                        };
                        let mut e = self.entry(k, Some(&v));
                        e["evidence_count"] = json!(v.evidence.as_array().map_or(0, Vec::len));
                        e["sleep_id"] = json!(sid);
                        out.push(e);
                        self.slots[k].push(v);
                    }
                    (200, json!({"user_id": user, "sleep_id": sid, "written": out}))
                }
                "self/notes" => {
                    let notes = self.notes.clone().unwrap_or(json!({"proposals": [], "mixed": []}));
                    (200, json!({"user_id": user, "version": self.notes_version, "written_at": null,
                                 "bytes": notes.to_string().len(), "notes": notes}))
                }
                "self/notes/write" => {
                    if b["base_version"].as_i64() != Some(self.notes_version) {
                        return coded(409, "self_notes_conflict", "the notes changed since they were read");
                    }
                    let n = &b["notes"];
                    self.notes = Some(json!({"proposals": n.get("proposals").cloned().unwrap_or(json!([])),
                                             "mixed": n.get("mixed").cloned().unwrap_or(json!([]))}));
                    self.notes_version += 1;
                    (200, json!({"user_id": user, "version": self.notes_version, "written_at": "2026-10-20T00:00:00Z"}))
                }
                "work/open" => (200, json!({"work_id": "w-1", "steps_left": 5, "deeper_left": 3,
                                            "memories": [{"n": 1, "id": "id-1", "content": "Jamie likes flowers."}]})),
                other => panic!("a route the stand-in does not know: {other}"),
            }
        }
    }

    type Svc = Arc<Mutex<Service>>;

    fn serve(svc: Svc) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            while let Ok((mut sock, _)) = listener.accept() {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 65536];
                let head_end = loop {
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(i + 4);
                    }
                    match sock.read(&mut chunk) {
                        Ok(0) | Err(_) => break None,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                };
                let Some(head_end) = head_end else { continue };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let mut headers = HashMap::new();
                for line in head.lines().skip(1) {
                    if let Some((k, v)) = line.split_once(':') {
                        headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                    }
                }
                let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
                while buf.len() < head_end + len {
                    match sock.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                let body = serde_json::from_slice(&buf[head_end..]).unwrap_or(json!({}));
                let (status, payload) = svc.lock().unwrap().handle(&path, body, headers);
                let payload = payload.to_string();
                let _ = sock.write_all(
                    format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                        payload.len()
                    )
                    .as_bytes(),
                );
            }
        });
        base
    }

    fn new_with(svc: Service) -> (Svc, Client) {
        let svc = Arc::new(Mutex::new(svc));
        let base = serve(svc.clone());
        (svc, Client::with_base_url(KEY, &base))
    }

    fn new() -> (Svc, Client) {
        new_with(Service::default())
    }

    fn give(svc: &Svc, rows: Vec<Value>) {
        svc.lock().unwrap().rows.extend(rows);
    }

    fn paths(svc: &Svc) -> Vec<String> {
        svc.lock().unwrap().seen.iter().map(|s| s.path.trim_start_matches("/api/v1/memory/").to_string()).collect()
    }

    fn night(today: &str) -> SleepOpts {
        SleepOpts { today: Some(today.into()), settings: settings(), ..Default::default() }
    }

    async fn sleep_with(mem: &Client, model: &Arc<Model>, opts: SleepOpts) -> Result<Value, SleepError> {
        mem.sleep("u", model.clone(), &opts).await
    }

    fn wos(r: Result<Value, SleepError>) -> WosError {
        match r {
            Err(SleepError::Wos(e)) => e,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn the_prompts_are_the_reference_bytes() {
        for (name, text, want) in [
            ("_SLEEP_DIGEST", SLEEP_DIGEST, "8cff7e06101f3cf1f69256323a5d40563e576d9ecd963016b35a57980ce62541"),
            ("_SLEEP_COMBINE", SLEEP_COMBINE, "dc3132527b20cdef6ce3af6bf7da1bc1d885aa74914230566c6212765ab4942b"),
            ("_SLEEP_MERGE", SLEEP_MERGE, "7ccc820aa16ec2f16f36bf0b4afa57010ece0f36fc03fe6303d28a02a750ac2f"),
            ("_SLEEP_SHORTER", SLEEP_SHORTER, "1697835120c7f56cc71b2dc229987c4cf45d682eb3084480886f49bc23832730"),
            ("_SLEEP_SELF_RULE", SLEEP_SELF_RULE, "47b04387ca56c24adf5d4e31b6072b8a61bdf36d9880704428e7c509441b8ea1"),
            ("_SLEEP_SETTLE", SLEEP_SETTLE, "c20ba6effc265bd5612530f0f657e660a30eff85f28fd846b79ba5bed20163ca"),
            ("_SLEEP_FOLD_SECTION", SLEEP_FOLD_SECTION, "294ec09cd08d75e1778d567c7a3b1eb2dd4dd221f662334f51d8449260fe3881"),
            ("_SLEEP_TIDY", SLEEP_TIDY, "dff0600c9f98c1cc5928f75dd0101e5928e31f06a0d6aadc1a2f553706e3c16c"),
            ("_SLEEP_ROOM", SLEEP_ROOM, "41aff5a9ccb596e41031bd269a78dc4bc71aa5a9a47ec08f67ac8361c4926168"),
            ("_SLEEP_READER_NOTE", SLEEP_READER_NOTE, "0fa78c4e82cdb880ad70871895a61d7dcdc6505d5b1d1a0498c6c3c75669f028"),
        ] {
            assert_eq!(sha256(text.as_bytes()), want, "{name}");
        }
        assert_eq!(sha256(py_dumps(&open_box_tool()).as_bytes()), "4fa83367c475682278e48a00a6b24fdd86a35240c36965001512e4219c1d64d8");
        assert_eq!(SLEEP_NOT_JSON, "\n\nYour last answer was not valid JSON. Answer with the JSON object only.");
        assert_eq!(sha256(SLEEP_NOT_JSON.as_bytes()), "eeaaab2e5fb02a8f626858bd883425b5a5d6b8e84d6b01fa80b924f0530c0703");
    }

    #[test]
    fn the_settings_are_the_reference_ones() {
        assert_eq!(
            (SLEEP_RIPE_SLEEPS, SLEEP_RIPE_DAYS, SLEEP_MIN_EVIDENCE, SLEEP_FOLD_LINES, SLEEP_LINE_TOKENS),
            (3.0, 28.0, 2, 4, 90)
        );
        assert_eq!((SLEEP_DOUBT_TOKENS, SLEEP_OVERTURN, SLEEP_MIN_PART, SLEEP_MAX_READERS), (25, 1, 50_000, 200));
        assert_eq!(SLEEP_KINDS, ["self", "person", "work"]);
        assert_eq!(SLEEP_MIX_KINDS, ["echo", "planted", "speaker"]);
        let s = SleepSettings::default();
        assert_eq!(
            (s.core_tokens, s.box_tokens, s.part_tokens, s.readers, s.group, s.sleepy_tokens),
            (70_000, 20_000, 100_000, 10, 10, 10_000_000)
        );
        assert_eq!((SLEEP_PAGE_TOKENS, SLEEP_LINE_FRAME, SLEEP_MAX_TOKENS), (20_000, 12, 16_000));
        assert_eq!((SLEEP_IN_PER_100, SLEEP_OUT_PER_100, SLEEP_MARGIN, SLEEP_ESTIMATE_PRICES), (172, 7, 0.15, (2.0, 10.0)));
        assert_eq!((SLEEP_ESTIMATE_MODEL, SLEEP_VERSION, crate::SLEEP_VERSION), ("claude-sonnet-5-5", "1.0.0", "1.0.0"));
        let o = SleepOpts::default();
        assert_eq!((o.being.as_str(), o.prompt.as_str(), o.done, o.max_tokens), ("the assistant", "", true, 16_000));
        assert_eq!((o.line_time, o.settings), (SleepLineTime::CreatedAt, s));
    }

    #[test]
    fn settings_are_checked() {
        let with = |f: fn(&mut SleepSettings)| {
            let mut s = SleepSettings::default();
            f(&mut s);
            match s.check() {
                Err(WosError::Api { status: 400, message, .. }) => message,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(with(|s| s.core_tokens = 9_999), "core_tokens must be 10,000-70,000 (got 9,999)");
        assert_eq!(with(|s| s.core_tokens = 70_001), "core_tokens must be 10,000-70,000 (got 70,001)");
        assert_eq!(with(|s| s.box_tokens = 20_001), "box_tokens must be 10,000-20,000 (got 20,001)");
        assert_eq!(with(|s| s.part_tokens = 49_999), "part_tokens must be 50,000 or more (got 49,999)");
        assert_eq!(with(|s| s.readers = 0), "readers must be 1-200 (got 0)");
        assert_eq!(with(|s| s.readers = 201), "readers must be 1-200 (got 201)");
        assert_eq!(with(|s| s.group = 1), "group must be 2-200 (got 1)");
        assert_eq!(with(|s| s.sleepy_tokens = 0), "sleepy_tokens must be 1 or more (got 0)");
        let s = SleepSettings { part_tokens: 50_000, readers: 200, ..Default::default() };
        assert!(s.check().is_ok());
        assert_eq!(s, SleepSettings { part_tokens: 50_000, readers: 200, ..Default::default() });
        assert!(format!("{s:?}").contains("part_tokens: 50000"));
        assert!(!s.sleepy(9_999_999));
        assert!(s.sleepy(10_000_000));
        assert!(SleepSettings { sleepy_tokens: 1, ..Default::default() }.sleepy(1));
    }

    #[test]
    fn the_token_estimate_counts_digits_ascii_and_wide_characters_apart() {
        assert_eq!(sleep_tokens(""), 0);
        assert_eq!(sleep_tokens("abcd"), 1);
        assert_eq!(sleep_tokens("abcde"), 2);
        assert_eq!(sleep_tokens("2024"), 2);
        assert_eq!(sleep_tokens("\u{4eca}\u{65e5}"), 3);
        assert_eq!(sleep_tokens("é"), 1);
    }

    fn python() -> Value {
        serde_json::from_str(include_str!("testdata/sleep_py.json")).unwrap()
    }

    fn cases<'a>(py: &'a Value, key: &str) -> &'a Vec<Value> {
        py[key].as_array().unwrap()
    }

    #[test]
    fn dates_eids_and_the_no_room_message_are_found_as_the_python_patterns_find_them() {
        let py = python();
        for c in cases(&py, "dated") {
            assert_eq!(sleep_dated(c[0].as_str().unwrap()), c[1].as_bool().unwrap(), "{}", c[0]);
        }
        for c in cases(&py, "eids") {
            let want: Vec<String> = serde_json::from_value(c[1].clone()).unwrap();
            assert_eq!(sleep_eids(Some(&c[0])), want, "{}", c[0]);
        }
        for c in cases(&py, "no_room") {
            let got = sleep_no_room(c[0].as_str().unwrap()).map(|(a, b, d)| json!([a, b, d]));
            assert_eq!(got.unwrap_or(Value::Null), c[1], "{}", c[0]);
        }
    }

    #[test]
    fn an_answer_is_read_as_the_python_sleep_reads_it() {
        let py = python();
        for c in cases(&py, "parse") {
            let got = sleep_parse_json(c[0].as_str().unwrap()).map(Value::Object);
            assert_eq!(got.unwrap_or(Value::Null), c[1], "{}", c[0]);
        }
    }

    #[test]
    fn times_counts_and_numbers_are_the_python_sleeps() {
        let py = python();
        for c in cases(&py, "when") {
            let got = sleep_when(c[0].as_str().unwrap()).map(|(s, t)| json!([s, t]));
            assert_eq!(got.unwrap_or(Value::Null), c[1], "{}", c[0]);
        }
        for c in cases(&py, "tokens") {
            assert_eq!(sleep_tokens(c[0].as_str().unwrap()), c[1].as_u64().unwrap(), "{}", c[0]);
        }
        for c in cases(&py, "str") {
            assert_eq!(text_of(&c[0]), c[1].as_str().unwrap(), "{}", c[0]);
        }
        for c in cases(&py, "floats") {
            assert_eq!(float_repr(c[0].as_f64().unwrap()), c[1].as_str().unwrap());
        }
        for c in cases(&py, "rounds") {
            let got = round_to(c[0].as_f64().unwrap(), c[1].as_u64().unwrap() as usize);
            assert_eq!(got, c[2].as_f64().unwrap(), "round({}, {})", c[0], c[1]);
        }
        assert_eq!((commas(0), commas(999), commas(1_000), commas(-1_234_567)), ("0".into(), "999".into(), "1,000".into(), "-1,234,567".into()));
        let filled = fill("{a} {{b}} {n:,} {n} {{{c}}} {missing}", &[("a", Arg::Text("x")), ("n", Arg::Num(12345)), ("c", Arg::Text("y"))]);
        assert_eq!(filled, "x {b} 12,345 12345 {y} {missing}");
        assert_eq!(digit('٣'), Some(3));
        assert_eq!(digit('²'), None);
        assert_eq!(digits_value("１２"), Some(12));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn four_nights_are_the_reference_sleep_call_for_call() {
        let (svc, mem) = new();
        let model = Arc::new(Model::default());
        let mut got = Vec::new();
        for (n, day) in DAYS.iter().enumerate() {
            give(&svc, night_rows(n, day));
            let opts = SleepOpts { prompt: PROMPT.into(), ..night(&format!("{day} 23:00")) };
            got.push(sleep_with(&mem, &model, opts).await.unwrap());
        }
        assert_eq!(model.calls().len(), REFERENCE_CALLS);
        assert_eq!(model.digest(), REFERENCE_CALLS_DIGEST, "the calls the model got");
        let summaries = Value::Array(got.iter().map(night_summary).collect());
        assert_eq!(digest(&summaries), REFERENCE_NIGHTS_DIGEST, "what each night did");
        let svc = svc.lock().unwrap();
        assert_eq!(digest(&svc.state()), REFERENCE_STATE_DIGEST, "the files and the notes");

        let (n1, n2, n3, n4) = (&got[0], &got[1], &got[2], &got[3]);
        assert_eq!(n1["seeded"], true);
        assert_eq!((&n1["readers"], &n1["combine_rounds"]), (&json!(3), &json!(1)));
        assert!(svc.slots[0][0].body.starts_with("## Me\nI am the assistant in these conversations"));
        assert!(svc.slots[0][0].body.contains("### Style"));
        let calls: Vec<&str> =
            n1["log"].as_array().unwrap().iter().filter(|x| x.get("attempt").is_some()).map(|x| x["call"].as_str().unwrap()).collect();
        assert!(calls.contains(&"read 3 again 2"), "a part with no JSON is read again");
        assert!(n1["log"].as_array().unwrap().iter().any(|x| x["renumbered"] == true), "a part numbered from e1 is put back");
        assert_eq!(n1["items_dropped_by_check"], 3);
        let kinds: Vec<&str> = n1["mixed"].as_array().unwrap().iter().map(|m| m["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["echo", "speaker", "speaker"]);
        assert!(n1["mixed"].as_array().unwrap().iter().all(|m| m["sleep"] == "2026-09-01 23:00"));
        let whys: Vec<&str> = n1["refused"].as_array().unwrap().iter().map(|r| r["why"].as_str().unwrap()).collect();
        for why in [
            "nothing new about this person tonight",
            "\"Me\" changes only from a ripe proposal",
            "a box is a matter, not its state on a date: remake it under the matter's name",
            "no such section",
            "unknown file or op",
            "no evidence, or evidence not in this sleep",
            "a view of the being rests on something it said",
        ] {
            assert!(whys.contains(&why), "{why}");
        }
        let sections = |v: &Value| -> Vec<String> { v.as_array().unwrap().iter().map(|t| t["section"].as_str().unwrap().to_string()).collect() };
        assert_eq!(sections(&n1["tidied"]), ["Garden project"]);
        assert_eq!(sections(&n1["folds"]), ["Garden project"]);
        assert_eq!(svc.latest(1).unwrap().title, "Garden");
        assert_eq!(n2["settled"], json!([{"section": "Jamie", "how": "replace, +1"}, {"section": "Alex", "how": "new"}]));
        assert_eq!(n3["rooms"], json!([{"file": 4, "before": n3["rooms"][0]["before"], "after": 9}]));
        assert_eq!(n3["deduped"], json!([{"file": 2, "section": "Jamie"}]));
        assert!(svc.latest(3).unwrap().body.contains("## Alex"));
        let log4 = n4["log"].as_array().unwrap();
        let failed: Vec<&str> = log4.iter().filter(|x| x["failed"] == true).map(|x| x["call"].as_str().unwrap()).collect();
        assert_eq!(failed, ["merge"]);
        assert!(log4.iter().any(|x| x["call"] == "merge shorter"));
        assert_eq!(n4["ripe"], json!(["p1"]));
        assert!(svc.latest(0).unwrap().body.contains("I keep answers short."));
        assert_eq!(svc.notes.as_ref().unwrap()["proposals"][0]["applied"], "2026-10-05 23:00");
        assert_eq!(svc.last, svc.rows.len());
        assert_eq!(svc.sleeps.len(), 4);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_settle_answer_with_a_number_past_what_every_client_reads_is_not_json() {
        let opts = |n: usize| SleepOpts { prompt: PROMPT.into(), ..night(&format!("{} 23:00", DAYS[n])) };
        for (line, how) in [
            ("Infinity", "unshaped"),
            ("-Infinity", "unshaped"),
            ("NaN", "unshaped"),
            ("1e999", "unshaped"),
            ("-1e999", "unshaped"),
            ("1e300", "none, +0"),
            ("2.0", "drop, +0"),
        ] {
            let (svc, mem) = new();
            give(&svc, night_rows(0, DAYS[0]));
            sleep_with(&mem, &Arc::new(Model::default()), opts(0)).await.unwrap();
            give(&svc, night_rows(1, DAYS[1]));
            let model = Arc::new(Model { settle: Some(format!(r#"{{"add": [], "op": "drop", "line": {line}}}"#)), ..Default::default() });
            let res = sleep_with(&mem, &model, opts(1)).await.unwrap();
            assert_eq!(res["settled"][0], json!({"section": "Jamie", "how": how}), "{line}");
            let tries: Vec<Value> =
                res["log"].as_array().unwrap().iter().filter(|x| x["call"] == "settle Jamie").map(|x| x["attempt"].clone()).collect();
            assert_eq!(tries, if how == "unshaped" { vec![json!(1), json!(2)] } else { vec![json!(1)] }, "{line}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_span_run_again_after_its_sleep_done_failed_counts_each_proposal_and_each_mark_once() {
        let (svc, mem) = new();
        let model = Arc::new(Model::default());
        let expire = || {
            let (status, payload) = coded(410, "sleep_expired", "This sleep was read too long ago to finish: read again.");
            svc.lock().unwrap().fail.push(("/api/v1/memory/sleep/done".into(), status, payload));
        };
        let notes = |key: &str| svc.lock().unwrap().notes.as_ref().unwrap()[key].as_array().unwrap().clone();
        let counts = || -> Vec<Value> { notes("proposals").iter().map(|p| json!([p["id"], p["sleeps"], p["span_end"]])).collect() };
        let opts = |n: usize| SleepOpts { prompt: PROMPT.into(), ..night(&format!("{} 23:00", DAYS[n])) };
        let (rows1, rows2) = (night_rows(0, DAYS[0]), night_rows(1, DAYS[1]));
        let (end1, end2) = (rows1.last().unwrap()["id"].clone(), rows2.last().unwrap()["id"].clone());
        give(&svc, rows1);
        expire();
        assert_eq!(wos(sleep_with(&mem, &model, opts(0)).await).status(), Some(410));
        let marks = notes("mixed").len();
        assert!(marks > 0);
        assert_eq!(counts(), [json!(["p1", 1, end1])]);
        sleep_with(&mem, &model, opts(0)).await.unwrap();
        assert_eq!(counts(), [json!(["p1", 1, end1])], "a proposal made by this span is not made again");
        assert_eq!(notes("mixed").len(), marks, "nor a mark");
        give(&svc, rows2);
        expire();
        assert_eq!(wos(sleep_with(&mem, &model, opts(1)).await).status(), Some(410));
        assert_eq!(counts(), [json!(["p1", 2, end2])]);
        let again = sleep_with(&mem, &model, opts(1)).await.unwrap();
        assert_eq!(counts(), [json!(["p1", 2, end2])], "the span counted it once");
        assert_eq!(again["done"]["changed"], true);
        let svc = svc.lock().unwrap();
        assert_eq!(svc.last, svc.rows.len());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn notes_from_before_span_end_still_read_and_their_proposals_count() {
        let (svc, mem) = new();
        svc.lock().unwrap().notes = Some(json!({"proposals": [{"id": "p1", "text": "I prefer short answers.", "sleeps": 1, "first": 0,
            "first_day": "2026-09-01 23:00", "last_day": "2026-09-01 23:00"}], "mixed": []}));
        let rows = night_rows(1, DAYS[1]);
        let end = rows.last().unwrap()["id"].clone();
        give(&svc, rows);
        sleep_with(&mem, &Arc::new(Model::default()), night(&format!("{} 23:00", DAYS[1]))).await.unwrap();
        let svc = svc.lock().unwrap();
        let p = &svc.notes.as_ref().unwrap()["proposals"][0];
        assert_eq!(json!([p["id"], p["sleeps"], p["span_end"]]), json!(["p1", 2, end]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn one_night_asks_the_service_in_this_order() {
        let (svc, mem) = new();
        give(&svc, night_rows(1, DAYS[1]));
        let res = sleep_with(&mem, &Arc::new(Model::default()), night("2026-09-15 23:00")).await.unwrap();
        assert_eq!(paths(&svc), ["settings?user_id=u", "sleep", "self", "self/notes", "self/write", "self/notes/write", "sleep/done"]);
        let svc = svc.lock().unwrap();
        assert_eq!(svc.seen[1].body, json!({"user_id": "u", "budget_tokens": 20_000, "since_last_sleep": true}));
        let write = &svc.seen[4].body;
        assert_eq!(write["sleep_id"], "s-1");
        let writes = write["writes"].as_array().unwrap();
        assert_eq!(writes.iter().map(|w| w["slot"].as_u64().unwrap()).collect::<Vec<_>>(), [0, 1]);
        assert!(writes.iter().all(|w| w["base_version"] == 0 && w["day"] == "2026-09-15"));
        let ids: HashSet<&str> = svc.rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert!(writes[0]["evidence"].as_array().unwrap().iter().all(|e| ids.contains(e.as_str().unwrap())));
        for x in &svc.seen[4..] {
            assert!(x.headers.contains_key("idempotency-key"), "{}", x.path);
        }
        assert_eq!(res["written"], json!({"0": 1, "1": 1}));
        assert_eq!(res["done"]["changed"], true);
        assert_eq!((&res["sleep_id"], &res["memories"], &res["today"]), (&json!("s-1"), &json!(20), &json!("2026-09-15 23:00")));
        let log_in: i64 = res["log"].as_array().unwrap().iter().filter_map(|x| x.get("in")).map(|x| x.as_i64().unwrap()).sum();
        assert_eq!(res["llm_in"], log_in);
        assert_eq!(res["sleep_version"], SLEEP_VERSION);
        assert!(res["llm_out"].as_i64().unwrap() > 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pages_are_read_to_the_end_and_today_comes_from_the_reading() {
        let (svc, mem) = new();
        give(&svc, night_rows(0, DAYS[0])[..120].to_vec());
        let opts = SleepOpts { done: false, settings: settings(), ..Default::default() };
        let res = sleep_with(&mem, &Arc::new(Model::default()), opts).await.unwrap();
        let svc = svc.lock().unwrap();
        let pages: Vec<&Value> = svc.seen.iter().filter(|x| x.path.ends_with("/sleep")).map(|x| &x.body).collect();
        assert!(pages.len() > 2);
        assert!(pages[0].get("sleep_id").is_none());
        for p in &pages[1..] {
            assert_eq!((&p["sleep_id"], &p["since_last_sleep"]), (&json!("s-1"), &json!(true)));
        }
        assert_eq!(res["memories"], 120);
        assert_eq!(res["today"], "2026-10-20 09:30", "the person's now from the reading's clock");
        assert!(res["done"].is_null());
        assert!(!svc.seen.iter().any(|x| x.path.ends_with("sleep/done")));
        assert_eq!(svc.last, 0, "not done: the next sleep reads this again");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_empty_span_seeds_the_self_and_ends_the_sleep_without_the_model() {
        let (svc, mem) = new();
        let model = Arc::new(Model::default());
        let opts = |today: &str| SleepOpts { prompt: "I am Mira.".into(), ..night(today) };
        let res = sleep_with(&mem, &model, opts("2026-10-20 08:00")).await.unwrap();
        assert!(model.calls().is_empty());
        assert_eq!((&res["seeded"], &res["memories"]), (&json!(true), &json!(0)));
        {
            let s = svc.lock().unwrap();
            assert_eq!(s.latest(0).unwrap().body, "## Me\nI am Mira.\n");
            assert_eq!(s.latest(0).unwrap().evidence, json!([]));
        }
        assert_eq!(paths(&svc), ["settings?user_id=u", "sleep", "self", "self/notes", "self/write", "sleep/done"]);
        let res = sleep_with(&mem, &model, opts("2026-10-21 08:00")).await.unwrap();
        assert_eq!(res["seeded"], false, "the self is there: not seeded again");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_prompt_over_the_core_is_refused_before_the_model_is_called() {
        let (svc, mem) = new();
        let model = Arc::new(Model::default());
        let s = SleepSettings { core_tokens: 10_000, part_tokens: 50_000, ..Default::default() };
        let opts = SleepOpts { prompt: "word ".repeat(9_000), settings: s, ..Default::default() };
        let e = wos(sleep_with(&mem, &model, opts).await).to_string();
        assert!(e.contains("the core with the prompt in it would be 10,229 tokens, over its 10,000: raise core_tokens or shorten the prompt"), "{e}");
        let at_most = SleepOpts { prompt: "word ".repeat(65_000), settings: SleepSettings { part_tokens: 50_000, ..Default::default() }, ..Default::default() };
        let e = wos(sleep_with(&mem, &model, at_most).await).to_string();
        assert!(e.contains("the core with the prompt in it would be 7"), "{e}");
        assert!(e.contains(" tokens, over its 70,000: shorten the prompt"), "{e}");
        assert!(!e.contains("raise"), "{e}");
        assert!(model.calls().is_empty());
        assert!(!paths(&svc).contains(&"self/write".to_string()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_file_changed_meanwhile_is_a_conflict_and_none_of_the_night_is_kept() {
        let (svc, mem) = new();
        give(&svc, night_rows(1, DAYS[1]));
        svc.lock().unwrap().fail = vec![(
            "/api/v1/memory/self/write".into(),
            409,
            json!({"error": {"code": "self_version_conflict", "message": "the file changed since it was read"}}),
        )];
        let e = wos(sleep_with(&mem, &Arc::new(Model::default()), night("2026-09-15 23:00")).await);
        assert_eq!((e.status(), e.kind()), (Some(409), ErrorKind::Conflict));
        assert!(e.to_string().contains("the file changed since it was read"));
        let p = paths(&svc);
        assert_eq!(p.iter().filter(|x| *x == "self/write").count(), 1, "not tried again");
        assert!(!p.contains(&"self/notes/write".to_string()) && !p.contains(&"sleep/done".to_string()));
        let svc = svc.lock().unwrap();
        assert_eq!(svc.state()["slots"], json!({}));
        assert_eq!(svc.last, 0, "the next sleep reads this span again");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_file_the_service_counts_over_its_cap_is_made_room_for_once_then_kept_as_it_was() {
        let (svc, mem) = new();
        give(&svc, night_rows(1, DAYS[1]));
        {
            let mut s = svc.lock().unwrap();
            (s.scale, s.caps[1], s.room_message) = (2, 8, false);
        }
        let model = Arc::new(Model::default());
        let res = sleep_with(&mem, &model, night("2026-09-15 23:00")).await.unwrap();
        let svc = svc.lock().unwrap();
        let writes: Vec<&Value> = svc.seen.iter().filter(|x| x.path.ends_with("/self/write")).map(|x| &x.body["writes"]).collect();
        assert_eq!(writes.len(), 3);
        let slots: Vec<Vec<u64>> =
            writes.iter().map(|ws| ws.as_array().unwrap().iter().map(|w| w["slot"].as_u64().unwrap()).collect()).collect();
        assert_eq!(slots, [vec![0, 1], vec![0, 1], vec![0]]);
        assert_eq!(writes[1][1]["body"], "## Big list\n- Trimmed to what matters.\n");
        let room: Vec<String> = model.texts().into_iter().map(|(s, _)| s).filter(|s| s.contains("there is no room")).collect();
        assert_eq!(room.len(), 1);
        assert!(room[0].contains("left its box \"\" at 13 tokens, over its cap\nof 11: there is no room"), "{}", room[0]);
        assert_eq!(res["rooms"], json!([{"file": 1, "before": 13, "after": 9}]));
        let refused = res["refused"].as_array().unwrap();
        assert_eq!(
            refused[refused.len() - 1],
            json!({"edit": {"file": 1, "op": "room"}, "why": "no room: the service counts it over its cap; the file stays as it was"})
        );
        assert_eq!(res["written"], json!({"0": 1}));
        assert!(svc.slots[1].is_empty());
        assert!(svc.seen.iter().any(|x| x.path.ends_with("sleep/done")));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn when_the_service_names_the_file_and_its_count_the_room_is_made_against_them() {
        let (svc, mem) = new();
        give(&svc, night_rows(1, DAYS[1]));
        {
            let mut s = svc.lock().unwrap();
            (s.scale, s.caps[1]) = (2, 20);
        }
        let model = Arc::new(Model::default());
        let res = sleep_with(&mem, &model, night("2026-09-15 23:00")).await.unwrap();
        let room: Vec<String> = model.texts().into_iter().map(|(s, _)| s).filter(|s| s.contains("there is no room")).collect();
        assert!(room[0].contains("at 13 tokens, over its cap\nof 10: there is no room"), "{}", room[0]);
        assert_eq!(res["rooms"], json!([{"file": 1, "before": 13, "after": 9}]));
        assert_eq!(res["written"], json!({"0": 1, "1": 1}));
        assert_eq!(svc.lock().unwrap().latest(1).unwrap().body, "## Big list\n- Trimmed to what matters.\n");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_no_room_answer_that_counts_0_tokens_makes_room_against_the_clients_own_cap() {
        let (svc, mem) = new();
        give(&svc, night_rows(1, DAYS[1]));
        let error = json!({"error": {"code": "self_no_room", "message": "there is no room", "slot": 1, "tokens": 0, "cap": -1}});
        svc.lock().unwrap().fail = vec![("/api/v1/memory/self/write".into(), 400, error)];
        let model = Arc::new(Model::default());
        let res = sleep_with(&mem, &model, night("2026-09-15 23:00")).await.unwrap();
        let room: Vec<String> = model.texts().into_iter().map(|(s, _)| s).filter(|s| s.contains("there is no room")).collect();
        assert!(room[0].contains("at 13 tokens, over its cap\nof 11: there is no room"), "{}", room[0]);
        assert_eq!(res["rooms"], json!([{"file": 1, "before": 13, "after": 9}]));
        assert_eq!(res["written"], json!({"0": 1, "1": 1}));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn token_counts_and_versions_too_big_to_add_stop_at_the_largest_number_instead_of_panicking() {
        let huge = |mut r: Value| {
            r["tokens"] = json!(4_000_000_000_000_000_000u64);
            r
        };
        let (svc, mem) = new();
        give(&svc, night_rows(1, DAYS[1]).into_iter().map(huge).collect());
        let part = SleepSettings { part_tokens: 50_000, ..Default::default() };
        let est = mem.sleep_estimate("u", &SleepEstimateOpts { settings: part, ..Default::default() }).await.unwrap();
        assert_eq!((&est["tokens"], &est["input_tokens"], &est["sleepy"]), (&json!(u64::MAX), &json!(u64::MAX), &json!(true)));
        let max: CountTokens = Arc::new(|_: &str| u64::MAX);
        let counted = SleepEstimateOpts { settings: part, count_tokens: Some(max), ..Default::default() };
        assert_eq!(mem.sleep_estimate("u", &counted).await.unwrap()["tokens"], json!(u64::MAX));
        let written = sleep_written(&json!({}), &[json!({"slot": 0, "base_version": i64::MAX})]);
        assert_eq!(written, BTreeMap::from([(0, i64::MAX)]));
        let usage: Map<String, Value> = serde_json::from_value(json!({"input_tokens": i64::MAX, "output_tokens": i64::MAX})).unwrap();
        assert_eq!(sleep_usage(&usage, &["input_tokens", "output_tokens"]), i64::MAX);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn another_refusal_ends_the_sleep_and_is_not_made_room_for() {
        let (svc, mem) = new();
        give(&svc, night_rows(1, DAYS[1]));
        svc.lock().unwrap().fail =
            vec![("/api/v1/memory/self/write".into(), 400, json!({"error": {"code": "self_evidence_unknown", "message": "no"}}))];
        let e = wos(sleep_with(&mem, &Arc::new(Model::default()), night("2026-09-15 23:00")).await);
        assert_eq!(e.kind(), ErrorKind::BadRequest);
        assert!(!paths(&svc).contains(&"sleep/done".to_string()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_rate_limited_call_is_tried_again_and_another_failure_ends_the_sleep() {
        let now = std::time::Duration::ZERO;
        let (svc, mem) = new();
        give(&svc, night_rows(1, DAYS[1]));
        let model = Model::failing(vec![
            ModelCallError::new(429, "slow down").with_retry_after(now),
            ModelCallError::new(529, "busy").with_retry_after(now),
        ]);
        let res = sleep_with(&mem, &model, night("2026-09-15 23:00")).await.unwrap();
        assert_eq!(res["written"], json!({"0": 1, "1": 1}));
        let calls = model.calls();
        assert!(calls[0] == calls[1] && calls[1] == calls[2]);

        let (svc, mem) = new();
        give(&svc, night_rows(1, DAYS[1]));
        let model = Model::failing(vec![ModelCallError::new(400, "bad request")]);
        match sleep_with(&mem, &model, night("2026-09-15 23:00")).await {
            Err(SleepError::Model(e)) => assert_eq!(e.status, 400),
            other => panic!("{other:?}"),
        }
        assert_eq!(model.calls().len(), 1);
        assert_eq!(paths(&svc), ["settings?user_id=u", "sleep", "self", "self/notes"]);

        let (svc, mem) = new();
        give(&svc, night_rows(1, DAYS[1]));
        let model = Model::failing(vec![ModelCallError::new(429, "slow down").with_retry_after(now); 3]);
        assert!(matches!(sleep_with(&mem, &model, night("2026-09-15 23:00")).await, Err(SleepError::Model(_))));
        assert_eq!(model.calls().len(), 3, "tried twice more, then the sleep ends");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_reader_that_fails_ends_the_night_before_another_part_is_sent() {
        let (svc, mem) = new();
        give(&svc, night_rows(0, DAYS[0]));
        let model = Model::failing(vec![ModelCallError::new(400, "bad request")]);
        let opts = SleepOpts { settings: SleepSettings { readers: 1, ..settings() }, ..night("2026-09-01 23:00") };
        match sleep_with(&mem, &model, opts).await {
            Err(SleepError::Model(e)) => assert_eq!(e.status, 400),
            other => panic!("{other:?}"),
        }
        assert_eq!(model.calls().len(), 1, "the parts behind the failed one are not sent");
        assert!(!paths(&svc).contains(&"self/write".to_string()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merge_llm_writes() {
        let (svc, mem) = new();
        give(&svc, night_rows(1, DAYS[1]));
        let (reader, writer) = (Arc::new(Model::default()), Arc::new(Model::default()));
        let w = writer.clone();
        let merge = caller_fn(move |c: ModelCall| {
            let w = w.clone();
            async move { w.call(&c).await }
        });
        let opts = SleepOpts { merge_llm: Some(merge), ..night("2026-09-15 23:00") };
        let res = sleep_with(&mem, &reader, opts).await.unwrap();
        assert_eq!(reader.calls().len(), 1);
        assert!(writer.texts().iter().all(|(s, _)| {
            s.contains(", finishing.") || s.contains("Tonight") || s.contains("note of doubt") || s.contains("has grown")
        }));
        assert_eq!(res["written"], json!({"0": 1, "1": 1}));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_model_is_the_callers_own_else_sonnet() {
        let (svc, mem) = new();
        give(&svc, night_rows(1, DAYS[1]));
        let named = Arc::new(Model { named: Some("some-model"), ..Default::default() });
        sleep_with(&mem, &named, night("2026-09-15 23:00")).await.unwrap();
        assert!(named.models().iter().all(|m| m == "some-model") && !named.models().is_empty());
        let (svc, mem) = new();
        give(&svc, night_rows(1, DAYS[1]));
        let plain = Arc::new(Model::default());
        sleep_with(&mem, &plain, night("2026-09-15 23:00")).await.unwrap();
        assert!(plain.models().iter().all(|m| m == "claude-sonnet-5-5") && !plain.models().is_empty());
        assert_eq!(SLEEP_MODEL, "claude-sonnet-5-5");
        assert_eq!(crate::ASK_MODEL, "claude-haiku-5-5", "the router and skills keep theirs");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lines_are_dated_by_when_they_happened_when_asked() {
        let (svc, mem) = new();
        let mut rows = night_rows(1, DAYS[1]);
        for r in &mut rows {
            r["event_date"] = json!("2023-05-21T00:00:00Z");
        }
        give(&svc, rows);
        let model = Arc::new(Model::default());
        let opts = SleepOpts { line_time: SleepLineTime::EventDate, done: false, ..night("2026-09-15 23:00") };
        sleep_with(&mem, &model, opts).await.unwrap();
        let reads = |m: &Model| -> Vec<String> { m.texts().into_iter().filter(|(s, _)| s.contains(" you read part ")).map(|(_, u)| u).collect() };
        assert!(reads(&model)[0].contains("e1 · 2023-05-21 00:00 · user · Night 2 line 0"));
        sleep_with(&mem, &model, night("2026-09-15 23:00")).await.unwrap();
        assert!(reads(&model).last().unwrap().contains("e2 · 2026-09-15 09:00 · the assistant · Night 2 line 1"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn whose_line_it_is_goes_by_its_speaker_and_the_assistants_name() {
        let rows = || {
            let mut rows = night_rows(1, "2026-11-02")[..4].to_vec();
            rows[2].as_object_mut().unwrap().remove("speaker");
            rows[3]["speaker"] = json!(" Kai ");
            rows[3]["origin"] = json!("x");
            rows
        };
        let reads = |m: &Model| -> String {
            m.texts().into_iter().filter(|(s, _)| s.contains(" you read part ")).map(|(_, u)| u).collect::<Vec<_>>().join("\n")
        };
        let lines = |m: &Model| -> Vec<bool> {
            let text = reads(m);
            (0..4).map(|i| text.contains(&format!("· user · Night 2 line {i}:"))).collect()
        };
        let (svc, mem) = new();
        give(&svc, rows());
        let model = Arc::new(Model::default());
        sleep_with(&mem, &model, SleepOpts { done: false, ..night("2026-11-02 23:00") }).await.unwrap();
        assert_eq!(lines(&model), [true, false, true, false], "Ana's line and the one with no speaker are a person's");
        assert_eq!(paths(&svc)[..2], ["settings?user_id=u", "sleep"]);
        assert_eq!(paths(&svc).iter().filter(|p| p.starts_with("settings")).count(), 1);
        let (svc, mem) = new();
        give(&svc, rows());
        let model = Arc::new(Model::default());
        let opts = SleepOpts { assistant: Some(" Ana ".into()), done: false, ..night("2026-11-02 23:00") };
        sleep_with(&mem, &model, opts).await.unwrap();
        assert_eq!(lines(&model), [false, true, true, true]);
        assert_eq!(paths(&svc)[0], "sleep");
        for none in [Value::Null, json!("  ")] {
            let (svc, mem) = new_with(Service { assistant: none, ..Default::default() });
            give(&svc, rows());
            let model = Arc::new(Model::default());
            let e = wos(sleep_with(&mem, &model, night("2026-11-02 23:00")).await);
            assert_eq!(e.status(), Some(400));
            for want in ["default_speakers.assistant", "SleepOpts::assistant"] {
                assert!(e.to_string().contains(want), "{e}");
            }
            assert_eq!(paths(&svc), ["settings?user_id=u"]);
            assert!(model.calls().is_empty());
        }
        let (svc, mem) = new();
        let e = wos(sleep_with(&mem, &Arc::new(Model::default()), SleepOpts { assistant: Some(" ".into()), ..Default::default() }).await);
        assert!(e.to_string().contains("assistant is required"), "{e}");
        assert!(paths(&svc).is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_sleeps_names_are_trimmed_as_the_python_and_typescript_sleeps_trim_them() {
        let reads = |m: &Model| -> String {
            m.texts().into_iter().filter(|(s, _)| s.contains(" you read part ")).map(|(_, u)| u).collect::<Vec<_>>().join("\n")
        };
        let kai = "e2 · 2026-11-02 09:00 · the assistant · Night 2 line 1";
        let (svc, mem) = new_with(Service { assistant: json!("Kai\u{feff}"), ..Default::default() });
        give(&svc, night_rows(1, "2026-11-02")[..2].to_vec());
        let model = Arc::new(Model::default());
        sleep_with(&mem, &model, SleepOpts { done: false, ..night("2026-11-02 23:00") }).await.unwrap();
        assert!(reads(&model).contains(kai), "the store's name is trimmed of JavaScript whitespace: {}", reads(&model));
        let (svc, mem) = new();
        give(&svc, night_rows(1, "2026-11-02")[..2].to_vec());
        let model = Arc::new(Model::default());
        let opts = SleepOpts { assistant: Some("Kai\u{feff}".into()), done: false, ..night("2026-11-02 23:00") };
        sleep_with(&mem, &model, opts).await.unwrap();
        assert!(!reads(&model).contains(kai), "a given name is trimmed of Python whitespace only: {}", reads(&model));
        let (svc, mem) = new();
        for (name, opts) in [
            ("being", SleepOpts { being: "\u{1c}".into(), ..Default::default() }),
            ("since", SleepOpts { since: Some(" \u{1d}".into()), ..Default::default() }),
            ("sleep_id", SleepOpts { sleep_id: Some("\u{1e}".into()), ..Default::default() }),
            ("assistant", SleepOpts { assistant: Some("\u{1f}".into()), ..Default::default() }),
        ] {
            let e = wos(sleep_with(&mem, &Arc::new(Model::default()), opts).await);
            assert!(e.to_string().contains(&format!("{name} is required")), "{name}: {e}");
        }
        let opts = SleepEstimateOpts { since: Some("\u{1c}".into()), ..Default::default() };
        assert!(mem.sleep_estimate("u", &opts).await.unwrap_err().to_string().contains("since is required"));
        assert!(paths(&svc).is_empty());
    }

    #[test]
    fn a_line_number_is_read_as_python_reads_int() {
        for (v, want) in [
            (json!(" 12 "), Some(12)),
            (json!("+5"), Some(5)),
            (json!("-3"), Some(-3)),
            (json!("1_000"), Some(1000)),
            (json!("-1_0"), Some(-10)),
            (json!("\u{661}\u{662}"), Some(12)),
            (json!("\u{ff11}\u{ff12}"), Some(12)),
            (json!(true), Some(1)),
            (json!(false), Some(0)),
            (json!(2.9), Some(2)),
            (json!(-2.9), Some(-2)),
            (json!("2.0"), None),
            (json!(""), None),
            (json!("1__0"), None),
            (json!("_1"), None),
            (json!("1_"), None),
            (json!("\u{b2}"), None),
            (Value::Null, None),
        ] {
            assert_eq!(int_like(Some(&v)), want, "int({v})");
        }
    }

    #[test]
    fn the_many_people_texts_stand_for_the_one_person_passages_once() {
        assert_eq!(
            SLEEP_MANY_PEOPLE.digest,
            "Each line from a person is marked with that person's name, and different names are different\n  people: \
             each name is its own topic, and what one of them said, wants or is never goes under another.\n  A line \
             marked \"user\" names no one: give it to no named person."
        );
        assert_eq!(
            SLEEP_MANY_PEOPLE.merge,
            "and under one name: the memories mark each person by name, so each person has a section under their\n   own \
             name, and what one of them said, wants or is never goes into another's."
        );
        for (text, one, many) in [
            (SLEEP_DIGEST, SLEEP_ONE_PERSON.digest, SLEEP_MANY_PEOPLE.digest),
            (SLEEP_MERGE, SLEEP_ONE_PERSON.merge, SLEEP_MANY_PEOPLE.merge),
        ] {
            assert_eq!(text.matches(one).count(), 1);
            assert_eq!(sleep_people(text, one, many, false), text);
            let got = sleep_people(text, one, many, true);
            assert_eq!(got, text.replace(one, many));
            assert!(!got.contains(one) && got.matches(many).count() == 1);
        }
        let missed = std::panic::catch_unwind(|| sleep_people(SLEEP_COMBINE, SLEEP_ONE_PERSON.digest, "x", true));
        assert!(missed.is_err());
    }

    #[test]
    fn people_are_counted_across_the_whole_night() {
        let row = |speaker: Option<&str>| {
            let mut r = json!({"content": "words", "created_at": "2026-11-02T09:00:00Z"});
            if let Some(s) = speaker {
                r["speaker"] = json!(s);
            }
            r
        };
        let who = |rows: &[Value]| -> (Vec<String>, Vec<bool>, bool) {
            let (lines, many) = sleep_memories(rows, "Kai", "Kai", SleepLineTime::CreatedAt, &Count(None));
            (lines.iter().map(|m| m.who.clone()).collect(), lines.iter().map(|m| m.mine).collect(), many)
        };
        let one = [row(Some("Dana")), row(Some("Kai")), row(None), row(Some(" Dana "))];
        assert_eq!(who(&one), (vec!["user".into(), "Kai".into(), "user".into(), "user".into()], vec![false, true, false, false], false));
        let two = [row(Some("Dana")), row(Some("Kai")), row(None), row(Some(" Dana ")), row(Some("Mina"))];
        let (labels, mine, many) = who(&two);
        assert!(many);
        assert_eq!(labels, ["Dana", "Kai", "user", "Dana", "Mina"]);
        assert_eq!(mine, [false, true, false, false, false]);
    }

    fn two_people_rows() -> Vec<Value> {
        [
            (Some("Dana"), "I am vegetarian."),
            (Some("Kai"), "Noted, Dana: no meat for you."),
            (Some("Mina"), "I work the night shift."),
            (Some("Kai"), "Noted, Mina: your mornings stay free."),
            (Some("Dana"), "No meat at the dinner, please."),
            (Some("Mina"), "Keep my mornings free."),
            (None, "I like cooking for both of them."),
            (Some("Kai"), "I like planning the dinner."),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (speaker, text))| {
            let mut r = json!({"id": format!("d{i}"), "content": text, "created_at": format!("2026-11-02T09:{:02}:00Z", i),
                               "tokens": sleep_tokens(text)});
            if let Some(s) = speaker {
                r["speaker"] = json!(s);
            }
            r
        })
        .collect()
    }

    fn two_people_model(seen: Arc<Mutex<Vec<(String, String)>>>) -> Arc<dyn ModelCaller> {
        caller_fn(move |c: ModelCall| {
            let (system, user) = (c.system[0].clone(), c.user.clone());
            seen.lock().unwrap().push((system.clone(), user.clone()));
            async move {
                let v = if system.contains(" you read part ") {
                    json!({"items": [
                        {"kind": "person", "topic": "Dana", "text": "Dana is vegetarian.", "evidence": ["e1", "e5"]},
                        {"kind": "person", "topic": "Mina", "text": "Mina works the night shift.", "evidence": ["e3", "e6"]},
                        {"kind": "self", "topic": "me", "text": "I like cooking.", "evidence": ["e7", "e8"]},
                        {"kind": "self", "topic": "me", "text": "Not mine to say.", "evidence": ["e1", "e3"]},
                    ], "mixed": [
                        {"line": "e6", "kind": "echo", "source": "e4", "why": "gave back my words as hers"},
                        {"line": "e7", "kind": "planted", "why": "never said by them"},
                        {"line": "e3", "kind": "planted", "why": "a person's line is not planted"},
                        {"line": "e7", "kind": "echo", "why": "the being's line is not an echo"},
                    ]})
                } else if system.contains(", finishing.") {
                    json!({"edits": [
                        {"file": "core", "op": "rewrite", "section": "Dana", "text": "- Dana is vegetarian.", "evidence": ["e1", "e5"]},
                        {"file": "core", "op": "rewrite", "section": "Mina", "text": "- Mina works the night shift.",
                         "evidence": ["e3", "e6"]},
                    ], "proposals": [
                        {"text": "I like cooking.", "same_as": null, "evidence": ["e7"]},
                        {"text": "From the people only.", "same_as": null, "evidence": ["e1", "e3"]},
                    ], "notes": []})
                } else if system.contains("lines that are only a note of doubt") {
                    json!({"drop": []})
                } else {
                    panic!("a call the script does not know: {}", &system[..80]);
                };
                let text = v.to_string();
                Ok(ModelReply { text, stop: "end".into(), usage: Some(json!({"input_tokens": 1, "output_tokens": 1})) })
            }
        })
    }

    #[tokio::test(flavor = "current_thread")]
    async fn two_people_are_named_apart_and_each_rests_on_their_own_lines() {
        let (svc, mem) = new();
        give(&svc, two_people_rows());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let opts = SleepOpts { being: "Kai".into(), done: false, ..night("2026-11-02 23:00") };
        let res = mem.sleep("u", two_people_model(seen.clone()), &opts).await.unwrap();
        let seen = seen.lock().unwrap().clone();
        let read: Vec<&(String, String)> = seen.iter().filter(|(s, _)| s.contains(" you read part ")).collect();
        let merge: Vec<&(String, String)> = seen.iter().filter(|(s, _)| s.contains(", finishing.")).collect();
        assert_eq!((read.len(), merge.len()), (1, 1));
        let labels: Vec<String> = lines_of(&read[0].1).into_iter().map(|l| l.2).collect();
        assert_eq!(labels, ["Dana", "Kai", "Mina", "Kai", "Dana", "Mina", "user", "Kai"]);
        assert!(read[0].1.contains("e7 · 2026-11-02 09:06 · user · I like cooking for both of them."));
        let one = fill(SLEEP_ONE_PERSON.digest, &[("being", Arg::Text("Kai"))]);
        assert!(read[0].0.contains(SLEEP_MANY_PEOPLE.digest) && !read[0].0.contains(&one), "{}", read[0].0);
        let one = fill(SLEEP_ONE_PERSON.merge, &[("being", Arg::Text("Kai"))]);
        assert!(merge[0].0.contains(SLEEP_MANY_PEOPLE.merge) && !merge[0].0.contains(&one), "{}", merge[0].0);
        assert!(read[0].0.contains("A line marked \"user\" names no one") && !merge[0].0.contains("\"user\""));
        let items: Vec<(String, Value)> =
            res["items"].as_array().unwrap().iter().map(|it| (text_of(&it["topic"]), it["evidence"].clone())).collect();
        assert_eq!(
            items,
            [("Dana".into(), json!(["e1", "e5"])), ("Mina".into(), json!(["e3", "e6"])), ("me".into(), json!(["e7", "e8"]))]
        );
        assert_eq!(res["items_dropped_by_check"], json!(1), "the self on the people's lines alone");
        let marks: Vec<(String, String)> =
            res["mixed"].as_array().unwrap().iter().map(|x| (text_of(&x["memory"]), text_of(&x["kind"]))).collect();
        assert_eq!(marks, [("d5".to_string(), "echo".to_string()), ("d6".into(), "echo".into())]);
        let core = svc.lock().unwrap().latest(0).unwrap().body.clone();
        assert!(core.contains("## Dana\n- Dana is vegetarian.") && core.contains("## Mina\n- Mina works the night shift."), "{core}");
        assert_eq!(res["applied"].as_array().unwrap().len(), 2);
        let proposals: Vec<String> = res["proposals"].as_array().unwrap().iter().map(|p| text_of(&p["text"])).collect();
        assert!(proposals.is_empty(), "a view resting on the line with no speaker alone is a person's: {proposals:?}");
        assert_eq!(res["refused"].as_array().unwrap().len(), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_night_with_one_person_reads_the_one_person_texts() {
        let (svc, mem) = new();
        let mut rows = two_people_rows();
        rows.retain(|r| r["speaker"] != "Mina");
        give(&svc, rows);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let opts = SleepOpts { being: "Kai".into(), done: false, ..night("2026-11-02 23:00") };
        mem.sleep("u", two_people_model(seen.clone()), &opts).await.unwrap();
        let seen = seen.lock().unwrap().clone();
        let read = seen.iter().find(|(s, _)| s.contains(" you read part ")).unwrap();
        let labels: Vec<String> = lines_of(&read.1).into_iter().map(|l| l.2).collect();
        assert_eq!(labels, ["user", "Kai", "Kai", "user", "user", "Kai"]);
        let digest = fill(SLEEP_DIGEST, &[("being", Arg::Text("Kai")), ("k", Arg::Num(1)), ("n", Arg::Num(1))]);
        assert_eq!(read.0, digest);
        let merge = seen.iter().find(|(s, _)| s.contains(", finishing.")).unwrap();
        assert!(merge.0.contains(&fill(SLEEP_ONE_PERSON.merge, &[("being", Arg::Text("Kai"))])));
        assert!(!merge.0.contains(SLEEP_MANY_PEOPLE.merge));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_estimate_reads_counts_and_prices_and_the_sleep_reads_on_under_its_sleep() {
        let (svc, mem) = new();
        let rows = night_rows(1, DAYS[1]);
        give(&svc, rows.clone());
        let part = SleepSettings { part_tokens: 50_000, ..Default::default() };
        let priced = SleepEstimateOpts { input_price: Some(1.0), output_price: Some(5.0), settings: part, ..Default::default() };
        let est = mem.sleep_estimate("u", &priced).await.unwrap();
        let tokens: u64 = rows.iter().map(|r| r["tokens"].as_u64().unwrap() + 12).sum();
        let frame = sleep_tokens(SLEEP_DIGEST) + sleep_tokens("") + sleep_tokens("1: (empty)\n2: (empty)\n3: (empty)\n4: (empty)");
        let (sent, written) = (((tokens + frame) * 172).div_ceil(100), (tokens * 7).div_ceil(100));
        let basis = |prices: [f64; 2]| {
            json!({"sleep_version": "1.0.0", "model": "claude-sonnet-5-5", "effort": "high", "prompt_caching": false,
                   "margin": 0.15, "prices": prices})
        };
        let cost = round_to((sent as f64 * 1.0 + written as f64 * 5.0) * 1.15 / 1_000_000.0, 6);
        assert_eq!(
            est,
            json!({"sleep_id": "s-1", "memories": 20, "tokens": tokens, "parts": 1, "readers": 1, "input_tokens": sent,
                   "output_tokens": written, "cost": cost, "basis": basis([1.0, 5.0]), "sleepy": false})
        );
        assert_eq!(paths(&svc), ["sleep", "self"]);
        let opts = SleepOpts { sleep_id: est["sleep_id"].as_str().map(str::to_string), ..night("2026-09-15 23:00") };
        let nightly = sleep_with(&mem, &Arc::new(Model::default()), opts).await.unwrap();
        assert_eq!(nightly["sleep_id"], "s-1");
        assert_eq!(paths(&svc)[2..4], ["settings?user_id=u", "sleep"], "the estimate reads no settings; the night does");
        assert_eq!(svc.lock().unwrap().seen[3].body["sleep_id"], "s-1");
        assert_eq!(svc.lock().unwrap().sleeps.len(), 1, "no second sleep opened");
        let sleepy = SleepSettings { part_tokens: 50_000, sleepy_tokens: 1, ..Default::default() };
        let none = mem.sleep_estimate("u", &SleepEstimateOpts { settings: sleepy, ..Default::default() }).await.unwrap();
        assert_eq!((&none["cost"], &none["memories"]), (&json!(0.0), &json!(0)));
        give(&svc, night_rows(2, DAYS[2]));
        let chars: CountTokens = Arc::new(|t: &str| t.chars().count() as u64);
        let by_chars = SleepEstimateOpts { settings: sleepy, count_tokens: Some(chars), ..Default::default() };
        let est = mem.sleep_estimate("u", &by_chars).await.unwrap();
        assert_eq!(est["sleepy"], true);
        let want: u64 = night_rows(2, DAYS[2]).iter().map(|r| r["content"].as_str().unwrap().chars().count() as u64 + 12).sum();
        assert_eq!(est["tokens"], want);
        let cost = round_to(
            (est["input_tokens"].as_f64().unwrap() * 2.0 + est["output_tokens"].as_f64().unwrap() * 10.0) * 1.15 / 1_000_000.0,
            6,
        );
        assert_eq!((&est["cost"], &est["basis"]), (&json!(cost), &basis([2.0, 10.0])), "no prices: Claude Sonnet 5.5's, plus 15%");
        assert!(est["cost"].as_f64().unwrap() > 0.0);
        let n = svc.lock().unwrap().seen.len();
        for bad in [-1.0, f64::NAN, f64::INFINITY] {
            let o = SleepEstimateOpts { input_price: Some(bad), output_price: Some(1.0), ..Default::default() };
            assert!(mem.sleep_estimate("u", &o).await.unwrap_err().to_string().contains("input_price must be a price"));
        }
        for (i, o) in [(Some(1.0), None), (None, Some(5.0))] {
            let o = SleepEstimateOpts { input_price: i, output_price: o, ..Default::default() };
            assert!(mem.sleep_estimate("u", &o).await.unwrap_err().to_string().contains("together, or neither"));
        }
        assert_eq!(svc.lock().unwrap().seen.len(), n);
    }

    struct Wontopos;

    impl ModelCaller for Wontopos {
        fn call<'a>(&'a self, _: &'a ModelCall) -> BoxFuture<'a, Result<ModelReply, ModelCallError>> {
            unreachable!("never called")
        }

        fn base_url(&self) -> Option<&str> {
            Some("https://api.wontopos.com")
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn arguments_are_refused_before_anything_is_sent() {
        let (svc, mem) = new();
        let model = Arc::new(Model::default());
        let local = openai_caller(OpenAiCallerOpts {
            base_url: "http://localhost:11434/v1".into(),
            model: "m".into(),
            ..Default::default()
        })
        .unwrap();
        let m: Arc<dyn ModelCaller> = model.clone();
        let host: Arc<dyn ModelCaller> = Arc::new(Wontopos);
        let local: Arc<dyn ModelCaller> = Arc::new(local);
        let cases: Vec<(Arc<dyn ModelCaller>, SleepOpts, &str)> = vec![
            (m.clone(), SleepOpts { settings: SleepSettings { readers: 0, ..Default::default() }, ..Default::default() }, "readers must be 1-200"),
            (m.clone(), SleepOpts { today: Some("yesterday".into()), ..Default::default() }, "today must be"),
            (m.clone(), SleepOpts { today: Some("2026-10-20T08:00".into()), ..Default::default() }, "today must be"),
            (m.clone(), SleepOpts { today: Some("2026-02-30".into()), ..Default::default() }, "today must be"),
            (m.clone(), SleepOpts { max_tokens: 0, ..Default::default() }, "max_tokens must be a whole number from 1 to 128000"),
            (m.clone(), SleepOpts { max_tokens: 128_001, ..Default::default() }, "max_tokens must be"),
            (m.clone(), SleepOpts { being: " ".into(), ..Default::default() }, "being is required"),
            (m.clone(), SleepOpts { since: Some(String::new()), ..Default::default() }, "since is required"),
            (m.clone(), SleepOpts { sleep_id: Some(" ".into()), ..Default::default() }, "sleep_id is required"),
            (host.clone(), SleepOpts::default(), "is a Wontopos host"),
            (local, SleepOpts { merge_llm: Some(host), ..Default::default() }, "is a Wontopos host"),
        ];
        for (llm, opts, want) in cases {
            let e = wos(mem.sleep("u", llm, &opts).await);
            assert_eq!(e.status(), Some(400));
            assert!(e.to_string().contains(want), "want {want:?}, got {e}");
        }
        for (opts, want) in [
            (
                SleepOpts { today: Some("yesterday".into()), ..Default::default() },
                "[400] today must be \"YYYY-MM-DD HH:MM\" or \"YYYY-MM-DD\", the sleep's day, got \"yesterday\".",
            ),
            (SleepOpts { being: " ".into(), ..Default::default() }, "[400] being is required."),
        ] {
            assert_eq!(wos(mem.sleep("u", m.clone(), &opts).await).to_string(), want);
        }
        assert!(wos(mem.sleep(" ", model.clone(), &SleepOpts::default()).await).to_string().contains("user_id was passed but is blank"));
        assert!(svc.lock().unwrap().seen.is_empty());
        assert!(model.calls().is_empty());
        for today in ["2026-10-20", "2026-10-20 08:00"] {
            let (svc, mem) = new();
            let res = sleep_with(&mem, &model, SleepOpts { today: Some(today.into()), ..Default::default() }).await.unwrap();
            assert_eq!(res["today"], today);
            assert_eq!(paths(&svc).len(), 5);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn progress_hears_each_stage() {
        let (svc, mem) = new();
        give(&svc, night_rows(1, DAYS[1]));
        let heard = Arc::new(Mutex::new(Vec::<String>::new()));
        let h = heard.clone();
        let opts = SleepOpts { progress: Some(Arc::new(move |m: &str| h.lock().unwrap().push(m.to_string()))), ..night("2026-09-15 23:00") };
        sleep_with(&mem, &Arc::new(Model::default()), opts).await.unwrap();
        let heard = heard.lock().unwrap();
        assert_eq!(heard.len(), 3);
        assert!(heard[0].starts_with("20 memories, "), "{}", heard[0]);
        assert!(heard[1].starts_with("merging 4 items (0 combine rounds)"), "{}", heard[1]);
        assert!(heard[2].starts_with("done in ") && heard[2].ends_with("written [0, 1]"), "{}", heard[2]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sleep_read_since_the_last_sleep_or_an_instant_and_done() {
        let (svc, mem) = new();
        give(&svc, night_rows(1, DAYS[1]));
        let last = |svc: &Svc| svc.lock().unwrap().seen.last().map(|s| (s.body.clone(), s.headers.clone())).unwrap();
        let page = mem.sleep_read("u", &SleepReadOpts { since_last_sleep: Some(true), budget_tokens: Some(500), ..Default::default() }).await.unwrap();
        assert_eq!(last(&svc).0, json!({"user_id": "u", "budget_tokens": 500, "since_last_sleep": true}));
        let next = SleepReadOpts {
            since_last_sleep: Some(true),
            sleep_id: page["sleep_id"].as_str().map(str::to_string),
            cursor: Some(page["next_cursor"].clone()),
            ..Default::default()
        };
        mem.sleep_read("u", &next).await.unwrap();
        assert_eq!(last(&svc).0["sleep_id"], "s-1");
        mem.sleep_read("u", &SleepReadOpts { since: Some(" 2026-09-01T00:00:00Z ".into()), sleep_id: Some("s-1".into()), ..Default::default() })
            .await
            .unwrap();
        assert_eq!(last(&svc).0["since"], "2026-09-01T00:00:00Z");
        let r = mem.sleep_done(" s-1 ", "u").await.unwrap();
        assert_eq!(last(&svc).0, json!({"user_id": "u", "sleep_id": "s-1"}));
        assert!(last(&svc).1.contains_key("idempotency-key"));
        assert_eq!(r["changed"], true);
        let n = svc.lock().unwrap().seen.len();
        let timed = mem.with_tz(9.0).unwrap();
        let since = || Some("2026-09-01T00:00:00Z".to_string());
        for (opts, want) in [
            (SleepReadOpts { since_last_sleep: Some(true), since: since(), ..Default::default() }, "pass one of them"),
            (SleepReadOpts { since_last_sleep: Some(true), days_ago: Some(1), ..Default::default() }, "not with days_ago, room or today_start"),
            (SleepReadOpts { since: since(), room: Some("n1".into()), ..Default::default() }, "not with days_ago, room or today_start"),
            (SleepReadOpts { since_last_sleep: Some(true), today_start: since(), ..Default::default() }, "not with days_ago"),
            (SleepReadOpts { since: Some(" ".into()), ..Default::default() }, "since is required"),
        ] {
            let e = timed.sleep_read("u", &opts).await.unwrap_err().to_string();
            assert!(e.contains(want), "want {want:?}, got {e}");
        }
        assert!(mem.sleep_done("", "u").await.unwrap_err().to_string().contains("sleep_id is required"));
        assert_eq!(svc.lock().unwrap().seen.len(), n);
        timed.sleep_read("u", &SleepReadOpts { days_ago: Some(1), since_last_sleep: Some(false), ..Default::default() }).await.unwrap();
        assert!(last(&svc).0.get("since_last_sleep").is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_file_and_note_routes() {
        let (svc, mem) = new();
        give(&svc, night_rows(1, DAYS[1]));
        let page = mem.sleep_read("u", &SleepReadOpts { since_last_sleep: Some(true), ..Default::default() }).await.unwrap();
        let sid = page["sleep_id"].as_str().unwrap().to_string();
        let ids: Vec<String> = svc.lock().unwrap().rows[..2].iter().map(|r| r["id"].as_str().unwrap().to_string()).collect();
        let last = |svc: &Svc| svc.lock().unwrap().seen.last().map(|s| (s.body.clone(), s.headers.clone())).unwrap();
        let w = mem
            .write_sleep_files(
                &sid,
                &[
                    SleepFileWrite { slot: 0, body: "## Me\nI am Mira.\n".into(), day: Some("2026-10-20".into()), ..Default::default() },
                    SleepFileWrite { slot: 2, title: "Garden".into(), body: "## Beds\n- Two.\n".into(), evidence: ids.clone(), ..Default::default() },
                ],
                "u",
            )
            .await
            .unwrap();
        let written: Vec<(u64, u64, u64)> = w["written"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| (x["slot"].as_u64().unwrap(), x["version"].as_u64().unwrap(), x["evidence_count"].as_u64().unwrap()))
            .collect();
        assert_eq!(written, [(0, 1, 0), (2, 1, 2)]);
        assert!(last(&svc).1.contains_key("idempotency-key"));
        assert_eq!(
            last(&svc).0["writes"][1],
            json!({"slot": 2, "base_version": 0, "title": "Garden", "body": "## Beds\n- Two.\n", "evidence": ids})
        );
        let files = mem.sleep_files("u").await.unwrap();
        assert_eq!(files["core"]["body"], "## Me\nI am Mira.\n");
        let listed: Vec<(u64, u64)> =
            files["files"].as_array().unwrap().iter().map(|f| (f["slot"].as_u64().unwrap(), f["version"].as_u64().unwrap())).collect();
        assert_eq!(listed, [(1, 0), (2, 1), (3, 0), (4, 0)]);
        assert!(files["files"][1].get("body").is_none());
        let again = SleepFileWrite { slot: 2, base_version: 1, title: "Garden".into(), body: "## Beds\n- Three.\n".into(), ..Default::default() };
        mem.write_sleep_files(&sid, &[again], "u").await.unwrap();
        assert_eq!(mem.open_sleep_file(2, "u", None).await.unwrap()["body"], "## Beds\n- Three.\n");
        assert_eq!(mem.open_sleep_file(2, "u", Some(1)).await.unwrap()["body"], "## Beds\n- Two.\n");
        assert_eq!(last(&svc).0, json!({"user_id": "u", "slot": 2, "version": 1}));
        let hist = mem.sleep_file_history(2, "u").await.unwrap();
        let versions: Vec<u64> = hist["versions"].as_array().unwrap().iter().map(|v| v["version"].as_u64().unwrap()).collect();
        assert_eq!(versions, [1, 2]);
        let notes = mem.sleep_notes("u").await.unwrap();
        assert_eq!((&notes["version"], &notes["notes"]), (&json!(0), &json!({"proposals": [], "mixed": []})));
        mem.write_sleep_notes(0, &json!({"proposals": [], "mixed": [{"memory": ids[0]}]}), "u").await.unwrap();
        assert_eq!(mem.sleep_notes("u").await.unwrap()["notes"]["mixed"], json!([{"memory": ids[0]}]));
        assert_eq!(mem.write_sleep_notes(0, &json!({}), "u").await.unwrap_err().kind(), ErrorKind::Conflict);
        let n = svc.lock().unwrap().seen.len();
        let one = |slot: u32| SleepFileWrite { slot, ..Default::default() };
        for (writes, want) in [
            (vec![], "writes must be a list of 1 to 5 writes, one a file."),
            (vec![one(0), one(1), one(2), one(3), one(4), one(1)], "writes must be a list of 1 to 5"),
            (vec![one(5)], "writes[0].slot must be 0 (the core) or 1 to 4 (a box), got 5."),
            (vec![SleepFileWrite { evidence: vec!["".into()], ..one(1) }], "writes[0].evidence must be a list of memory ids."),
            (vec![SleepFileWrite { day: Some("today".into()), ..one(1) }], "writes[0].day must be \"YYYY-MM-DD\", got \"today\"."),
            (vec![one(1), one(1)], "writes[1].slot 1 is written twice: one write a file."),
        ] {
            let e = mem.write_sleep_files(&sid, &writes, "u").await.unwrap_err().to_string();
            assert!(e.contains(want), "want {want:?}, got {e}");
        }
        for slot in [5, 99] {
            assert!(mem.open_sleep_file(slot, "u", None).await.unwrap_err().to_string().contains("slot must be 0"));
            assert!(mem.sleep_file_history(slot, "u").await.unwrap_err().to_string().contains("slot must be 0"));
        }
        assert!(mem.open_sleep_file(1, "u", Some(0)).await.unwrap_err().to_string().contains("version must be a whole number"));
        assert!(mem.write_sleep_notes(0, &json!([]), "u").await.unwrap_err().to_string().contains("notes must be an object"));
        assert!(mem.write_sleep_files(" ", &[one(1)], "u").await.unwrap_err().to_string().contains("sleep_id is required"));
        assert_eq!(svc.lock().unwrap().seen.len(), n);
    }

    fn version(n: i64, title: &str, body: &str, tokens: i64, day: &str) -> Version {
        Version {
            version: n,
            title: title.into(),
            body: body.into(),
            tokens,
            evidence: json!([]),
            day: json!(day),
            sleep_id: Value::Null,
        }
    }

    fn files_service() -> Service {
        let mut svc = Service::default();
        svc.slots[0] = vec![version(2, "", "## Jamie\n- Close friend.\n\n## Me\nI am the assistant.\n", 10, "2026-10-01")];
        svc.slots[1] = vec![version(1, "Garden", "## Beds\n- Two beds of tomatoes.\n", 9, "2026-09-28")];
        svc.slots[3] = vec![version(1, "Reading list", "  ", 1, "2026-10-05")];
        svc
    }

    fn scroll2(mem: &Client) -> Client {
        mem.with_model("scroll-2")
    }

    const TOOLS_TODAY: [((bool, bool, bool), &str); 8] = [
        ((false, false, false), "81b100c86e145fc85bde3044ad55b441397c80b8af53d4bf8fb5b7c2a2180bb7"),
        ((false, false, true), "bb8fafea3d722148d1fed9bdc480b02140594aeeb9e5ade744d29e6b09c32123"),
        ((false, true, false), "248413498d4644742be51182540d14641ee87aed84409f227948c18dec630caa"),
        ((false, true, true), "5da071c6a1de7190a4ea2fe7137f264f4b68ebe656b0ffa169b7fdbce13bf2e1"),
        ((true, false, false), "bfb41a8d660bb77f4a6b033b0d319ff6ffcf4f4ebb183d66a69436ea55272c96"),
        ((true, false, true), "c5fcbda4b02200b423c45d67c768f7e79b13278f2cc31a16d07ad2685e2a9f6e"),
        ((true, true, false), "4380b38d91910bdb804a133ce18cc3056e759731e4abccc71d00fe0c961775bd"),
        ((true, true, true), "e149cc1a01bd4883fe5ec1b10ffda10f0dbb6325a94bf405d28fa067ae9e8679"),
    ];

    fn tools_digest(t: &crate::MemoryTools) -> String {
        let blob = json!([t.note(), t.definitions(ToolFormat::Anthropic), t.definitions(ToolFormat::OpenAi)]);
        sha256(serde_json::to_string(&blob).unwrap().as_bytes())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn memory_tools_with_sleep_off_are_the_tools_without_it_to_the_byte() {
        let (svc, mem) = new_with(files_service());
        let mem = scroll2(&mem);
        for ((speakers, dates, images), want) in TOOLS_TODAY {
            let on = |t: crate::MemoryTools| t.with_speakers(speakers).with_dates(dates).with_images(images);
            assert_eq!(tools_digest(&on(mem.memory_tools("u", None).unwrap())), want, "{speakers} {dates} {images}");
            assert_eq!(tools_digest(&on(mem.memory_tools("u", None).unwrap().with_sleep(false))), want);
            assert_eq!(tools_digest(&on(mem.memory_tools("u", None).unwrap().with_sleep(true).with_sleep(false))), want);
        }
        let tools = mem.memory_tools("u", None).unwrap();
        let names: Vec<String> = tools.definitions(ToolFormat::Anthropic).iter().map(|d| d["name"].as_str().unwrap().to_string()).collect();
        assert!(!names.contains(&"open_box".to_string()));
        let e = tools.run("open_box", &json!({"box": "1"})).await.unwrap_err().to_string();
        assert!(e.contains("not a memory tool"), "{e}");
        assert!(tools.load().await.unwrap_err().to_string().contains("make the tools with with_sleep(true)"));
        tools.begin("q").await.unwrap();
        assert_eq!(paths(&svc), ["work/open"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn memory_tools_with_sleep_carry_the_core_the_boxes_and_the_reader_note() {
        let (svc, mem) = new_with(files_service());
        let mem = scroll2(&mem);
        let tools = mem.memory_tools("u", None).unwrap().with_sleep(true).with_today("2026-10-05").unwrap();
        let base = mem.memory_tools("u", None).unwrap().note();
        assert_eq!(tools.note(), base, "before the files are read, the note without them");
        assert!(paths(&svc).is_empty(), "the note reads nothing");
        tools.load().await.unwrap();
        let files = [
            "<core last edited 2026-10-01, 4 days ago>",
            "## Me",
            "I am the assistant.",
            "",
            "## Jamie",
            "- Close friend.",
            "</core>",
            "<boxes>",
            "1. Garden (last edited 2026-09-28, 7 days ago)",
            "2. (empty)",
            "3. Reading list (last edited 2026-10-05, today)",
            "4. (empty)",
            "</boxes>",
        ]
        .join("\n");
        assert_eq!(tools.note(), format!("{base}\n\n{SLEEP_READER_NOTE}\n{files}"));
        assert_eq!(paths(&svc), ["self"]);
        let defs = tools.definitions(ToolFormat::Anthropic);
        assert_eq!(defs[..defs.len() - 1], mem.memory_tools("u", None).unwrap().definitions(ToolFormat::Anthropic)[..]);
        assert_eq!(defs[defs.len() - 1], open_box_tool());
        let b = open_box_tool();
        assert_eq!(
            tools.definitions(ToolFormat::OpenAi).last().unwrap(),
            &json!({"type": "function", "function": {"name": "open_box", "description": b["description"], "parameters": b["input_schema"]}})
        );

        let opened = "<box 1 name=\"Garden\" last edited 2026-09-28, 7 days ago>\n## Beds\n- Two beds of tomatoes.\n</box>";
        assert_eq!(tools.run("open_box", &json!({"box": "1"})).await.unwrap(), opened);
        assert_eq!(tools.run_arguments("open_box", r#"{"box": "garden"}"#).await.unwrap(), opened);
        assert_eq!(tools.run_content("open_box", &json!({"box": 1})).await.unwrap(), crate::ToolContent::Text(opened.into()));
        assert_eq!(svc.lock().unwrap().seen.last().unwrap().body, json!({"user_id": "u", "slot": 1}));
        let n = svc.lock().unwrap().seen.len();
        assert_eq!(tools.run("open_box", &json!({"box": "2"})).await.unwrap(), "Box 2 is empty.");
        assert_eq!(svc.lock().unwrap().seen.len(), n, "an empty box asks nothing");
        assert_eq!(tools.run("open_box", &json!({"box": "Reading list"})).await.unwrap(), "Box 3 is empty.");
        let index = between(&files, "<boxes>\n", "\n</boxes>").to_string();
        assert_eq!(
            tools.run("open_box", &json!({"box": "Recipes"})).await.unwrap(),
            format!("No box by that name. The boxes are:\n{index}")
        );
        assert_eq!(tools.run_arguments("open_box", "{not json").await.unwrap(), "Refused: the arguments are not JSON.");
        svc.lock().unwrap().fail =
            vec![("/api/v1/memory/self/open".into(), 404, json!({"error": {"code": "self_version_not_found", "message": "gone"}}))];
        assert!(tools.run("open_box", &json!({"box": "1"})).await.unwrap().starts_with("Refused: [404]"));

        tools.begin("q").await.unwrap();
        let p = paths(&svc);
        assert_eq!(p.iter().filter(|x| *x == "self").count(), 1);
        assert_eq!(p.last().unwrap(), "work/open");
        svc.lock().unwrap().slots[0].push(version(3, "", "## Me\nI am the assistant, rested.\n", 9, "2026-10-05"));
        let note = tools.load().await.unwrap();
        assert!(note.contains("<core last edited 2026-10-05, today>\n## Me\nI am the assistant, rested.\n</core>"), "{note}");
        assert_eq!(note, tools.note());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn memory_tools_with_sleep_read_the_files_at_the_first_turn_and_date_them_on_the_clock() {
        let (svc, mem) = new_with(files_service());
        let mem = scroll2(&mem).with_tz(-12.0).unwrap();
        let tools = mem.memory_tools("u", None).unwrap().with_sleep(true);
        tools.begin("q").await.unwrap();
        assert_eq!(paths(&svc), ["self", "work/open"]);
        assert!(tools.note().contains("<core last edited 2026-10-01, "));
        let day = today_on(Some(-12.0));
        let want = format!("{} days ago", day - sleep_date("2026-10-01").unwrap());
        assert!(tools.note().contains(&want) || (day == sleep_date("2026-10-01").unwrap() && tools.note().contains("today")));
        let e = mem.memory_tools("u", None).unwrap().with_sleep(true).with_today("2026-13-01").unwrap_err().to_string();
        assert!(e.contains("today must be a day"), "{e}");
        let e = mem.memory_tools("u", None).unwrap().with_today("2026-10-01").unwrap_err().to_string();
        assert!(e.contains("it goes with with_sleep(true)"), "{e}");
        let plain = mem.memory_tools("u", None).unwrap();
        assert!(plain.load().await.unwrap_err().to_string().contains("make the tools with with_sleep(true)"));
    }

    mod attacks;
}
