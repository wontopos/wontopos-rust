use super::{
    check_whole, civil_from_days, days_from_civil, keep_records, lock, now_ms, provider_base, refused,
    reply_shape, truncated_walk, CallPurpose, Client, ModelCall, ModelCallError, ModelCaller, WosError, MAX_PAGES,
    MODEL_MAX_TOKENS,
};
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// The version of the sleep's texts, steps and rules.
pub const SLEEP_VERSION: &str = "1.0.3";

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
lines and leave out what a search gives back. Every section stays under its own heading, with at
least one line{core_way}: an answer that leaves a section out is not used. Keep what tonight added
that matters, and keep {being}'s voice.

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
const SLEEP_AFTER_ASKS: usize = 100;
const SLEEP_WRITE_REFUSALS: usize = 10;
const SLEEP_SPLIT_DEPTH: usize = 2;
const SLEEP_ASKED_OUT: &str = "the sleep puts at most 100 questions to the model to settle, tidy and fold after the merge";
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
    /// End the store's last sleep here, unless a part went unread or the merge came back unshaped (default `true`).
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

fn sleep_who(speaker: &str, being: &str) -> String {
    let low = speaker.to_lowercase();
    if low == crate::js_trim(being).to_lowercase() || low == "user" || speaker.contains(" \u{b7} ") {
        format!("\"{speaker}\"")
    } else {
        speaker.to_string()
    }
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
            let who = if mine { being.to_string() } else if many && !speaker.is_empty() { sleep_who(&speaker, being) } else { "user".to_string() };
            let size = match (&count.0, whole(r.get("tokens"))) {
                (Some(f), _) => f(&text),
                (None, Some(n)) if n >= 0 => n as u64,
                (None, _) => sleep_tokens(&text),
            };
            Line {
                eid: format!("e{}", i + 1),
                mid: r.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                at,
                who,
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

const SLEEP_TAGS: [&str; 10] = ["memories", "core", "boxes", "box", "proposals", "ripe", "items", "section", "tonight", "as it stood"];

fn sleep_control(c: char) -> bool {
    c.is_control() || c == '\u{2028}' || c == '\u{2029}'
}

fn sleep_folded(text: &str, breaks: fn(char) -> bool, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' && chars.peek() == Some(&'\n') {
            chars.next();
        }
        if breaks(c) {
            out.push_str(with);
        } else {
            out.push(c);
        }
    }
    out
}

fn sleep_untagged(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, c) in text.char_indices() {
        let tag = c == '<' && {
            let rest = &text[i + 1..];
            let rest = rest.strip_prefix('/').unwrap_or(rest);
            SLEEP_TAGS.iter().any(|t| rest.as_bytes().get(..t.len()).is_some_and(|head| head.eq_ignore_ascii_case(t.as_bytes())))
        };
        out.push(if tag { '\u{2039}' } else { c });
    }
    out
}

fn sleep_shown(text: &str) -> String {
    let breaks = |c| matches!(c, '\r' | '\n' | '\u{b}' | '\u{c}' | '\u{1c}'..='\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}');
    sleep_untagged(&sleep_folded(text, breaks, "\n    "))
}

/// The sections of the core a room's "moved" takes to a box, as (box, index of the section). A move names one
/// section: the only one of that exact name, else the only one the model was shown under it. A name two sections
/// answer to moves neither of them, and `Me`, a move with no name, and a section whose name a box cannot keep (`#`)
/// stay where they are.
fn sleep_room_moves(held: &[(String, String)], moved: Option<&Value>) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for mv in listed(moved) {
        let to = text_of(mv.get("to").unwrap_or(&Value::Null));
        if !mv.is_object() || !["1", "2", "3", "4"].contains(&to.as_str()) {
            continue;
        }
        let section = text_or_empty(mv.get("section"));
        let name = sleep_heading(&section);
        if name.is_empty() {
            continue;
        }
        let mut at: Vec<usize> = (0..held.len()).filter(|j| held[*j].0 == name).collect();
        if at.is_empty() {
            at = (0..held.len()).filter(|j| sleep_seen_as(&held[*j].0) == name).collect();
        }
        if at.len() != 1 || out.iter().any(|(_, j)| *j == at[0]) {
            continue;
        }
        let h = held[at[0]].0.as_str();
        if h != SLEEP_ME && sleep_heading(h) == h {
            out.push((to.parse().unwrap_or(1), at[0]));
        }
    }
    out
}

/// Which section of the file each section of a room's answer is, and the sections it leaves in doubt. A section of
/// the file answers to its own name when the answer has that heading, else to the heading the model was shown for
/// it; one that moved answers to none. The answer's sections of one heading stand, in order, for the file's
/// sections that answer to it, in the file's order. When the answer has fewer than the file, which ones it kept
/// cannot be told: it is paired from the top (in the core `Me` first, whose text is not the answer's to write), and
/// every one of them is in doubt.
fn sleep_room_pairs(held: &[(String, String)], given: &[(String, String)], moved_at: &[usize], core: bool) -> (Vec<Option<usize>>, Vec<usize>) {
    let mut out: Vec<Option<usize>> = vec![None; given.len()];
    let mut asked: Vec<(&str, Vec<usize>)> = Vec::new();
    for (i, (name, _)) in given.iter().enumerate() {
        if name.is_empty() {
            continue;
        }
        match asked.iter_mut().find(|(known, _)| *known == name.as_str()) {
            Some((_, at)) => at.push(i),
            None => asked.push((name.as_str(), vec![i])),
        }
    }
    let answers_to: Vec<Option<String>> = held
        .iter()
        .enumerate()
        .map(|(j, (h, _))| {
            if h.is_empty() || moved_at.contains(&j) {
                None
            } else if asked.iter().any(|(known, _)| *known == h.as_str()) {
                Some(h.clone())
            } else {
                Some(sleep_seen_as(h))
            }
        })
        .collect();
    let mut doubt: Vec<usize> = Vec::new();
    for (name, at) in asked {
        let mut group: Vec<usize> = (0..held.len()).filter(|j| answers_to[*j].as_deref() == Some(name)).collect();
        if at.len() < group.len() {
            doubt.extend(group.iter().copied());
            let is_me = |j: &usize| core && held[*j].0 == SLEEP_ME;
            let mut first: Vec<usize> = group.iter().copied().filter(is_me).collect();
            first.extend(group.iter().copied().filter(|j| !is_me(j)));
            first.truncate(at.len());
            first.sort_unstable();
            group = first;
        }
        for (i, j) in at.into_iter().zip(group) {
            out[i] = Some(j);
        }
    }
    (out, doubt)
}

/// A heading as the model is shown it: `sleep_kept` turns a rare line break into a new line and guards the
/// sleep's own tags.
fn sleep_seen_as(name: &str) -> String {
    sleep_heading(sleep_kept(name).split('\n').next().unwrap_or("")).to_string()
}

fn sleep_kept(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let rare = match c {
            '\r' => chars.peek() != Some(&'\n'),
            '\u{b}' | '\u{c}' | '\u{1c}'..='\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}' => true,
            _ => false,
        };
        if rare {
            out.push_str("\n    ");
        } else {
            out.push(c);
        }
    }
    sleep_untagged(&out)
}

fn sleep_named(text: &str) -> String {
    sleep_untagged(&sleep_folded(text, sleep_control, " "))
}

fn sleep_lines(mems: &[Line]) -> String {
    mems.iter()
        .map(|m| {
            let who = sleep_untagged(&sleep_folded(&m.who, sleep_control, " "));
            format!("{} · {} · {who} · {}", m.eid, m.at, sleep_shown(&m.text))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn sleep_heading(text: &str) -> &str {
    let mut h = strip(text);
    while h.starts_with('#') {
        h = strip(h.trim_start_matches('#'));
    }
    h
}

fn sleep_heading_broken(e: &Map<String, Value>, op: &str) -> bool {
    let mut named = vec![
        sleep_heading(&text_or_empty(e.get("section"))).to_string(),
        strip(&text_or_empty(e.get("title"))).to_string(),
    ];
    if op == "title" {
        named.push(strip(&text_or_empty(e.get("text"))).to_string());
    }
    if op == "remake" {
        named.extend(sleep_sections(strip(&text_or_empty(e.get("text")))).into_iter().map(|(h, _)| h));
    }
    named.iter().any(|x| x.chars().any(sleep_control))
}

fn sleep_speaks_of(topic: &str, name: &str) -> bool {
    !topic.is_empty()
        && !name.is_empty()
        && (topic == name
            || (name.chars().count() >= 2 && topic.starts_with(name))
            || (topic.chars().count() >= 3 && name.starts_with(topic)))
}

fn fenced(t: &str) -> Option<&str> {
    let (mut last, mut fence) = (None, false);
    for (i, c) in t.char_indices().rev() {
        if c == '}' && fence {
            last = Some(i);
            break;
        }
        fence = t[i..].starts_with("```") || (fence && py_space(c));
    }
    let mut from = 0;
    while let Some(p) = t[from..].find("```") {
        let start = from + p;
        let mut q = start + 3;
        if t[q..].starts_with("json") {
            q += 4;
        }
        q = t.len() - t[q..].trim_start_matches(py_space).len();
        if t[q..].starts_with('{') {
            return last.filter(|&r| r > q).map(|r| &t[q..=r]);
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
            let name = sleep_heading(h);
            out.push((if name.is_empty() { strip(h) } else { name }.to_string(), Vec::new()));
        } else if let Some(last) = out.last_mut() {
            last.1.push(ln);
        }
    }
    out.into_iter().map(|(h, b)| (h, strip(&b.join("\n")).to_string())).collect()
}

fn sleep_core(body: &str) -> String {
    let lines: Vec<&str> = body.split('\n').collect();
    let first = lines.iter().position(|ln| ln.starts_with("## ")).unwrap_or(lines.len());
    let lead = strip(&lines[..first].join("\n")).to_string();
    if lead.is_empty() {
        return body.to_string();
    }
    let mut secs = sleep_sections(body);
    match secs.iter().position(|(h, _)| h == SLEEP_ME) {
        Some(at) => secs[at].1 = strip(&format!("{lead}\n{}", secs[at].1)).to_string(),
        None => secs.insert(0, (SLEEP_ME.to_string(), lead)),
    }
    sleep_join(&secs)
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
    let heading = sleep_heading(heading).to_string();
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
    let gone: Vec<&str> = secs.iter().filter(|(h, _)| h == heading).map(|(_, t)| t.as_str()).collect();
    let text = if gone.is_empty() { None } else { Some(gone.into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n")) };
    let keep: Vec<(String, String)> = secs.iter().filter(|(h, _)| h != heading).cloned().collect();
    (sleep_join(&keep), text)
}

fn sleep_demote(text: &str) -> String {
    text.split('\n').map(|ln| if ln.starts_with("## ") { format!("#{ln}") } else { ln.to_string() }).collect::<Vec<_>>().join("\n")
}

fn sleep_age(day: Option<&str>, today: i64) -> String {
    let Some(day) = day.filter(|d| !d.is_empty()) else { return String::new() };
    let Some(d) = sleep_date(day) else { return String::new() };
    let n = today - d;
    let ago = if n <= 0 { "today".to_string() } else if n == 1 { "1 day ago".to_string() } else { format!("{n} days ago") };
    format!("last edited {day}, {ago}")
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
            let name = sleep_named(&f.title);
            let name = if name.is_empty() { "(empty)" } else { &name };
            let age = if sleep_has_body(f) { sleep_age(f.day.as_deref(), today) } else { String::new() };
            if age.is_empty() { format!("{s}. {name}") } else { format!("{s}. {name} ({age})") }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn sleep_render(view: &SleepView, today: i64) -> String {
    let core = &view[0];
    let age = sleep_age(core.day.as_deref(), today);
    let head = if age.is_empty() { "<core>".to_string() } else { format!("<core {age}>") };
    let secs = sleep_sections(&sleep_core(core.body.as_deref().unwrap_or("")));
    let (me, people): (Vec<_>, Vec<_>) = secs.into_iter().partition(|(h, _)| h == SLEEP_ME);
    let body = sleep_join(&[me, people].concat());
    format!("{head}\n{}\n</core>\n<boxes>\n{}\n</boxes>", sleep_kept(strip(&body)), sleep_box_index(view, today))
}

pub(crate) fn sleep_box_pick(view: &SleepView, which: &str) -> Option<usize> {
    let which = strip(which);
    if which.is_empty() {
        return None;
    }
    let lower = which.to_lowercase();
    let title = |s: usize| view[s].title.as_str();
    SLEEP_BOXES
        .into_iter()
        .find(|&s| which == s.to_string() || which == title(s))
        .or_else(|| SLEEP_BOXES.into_iter().find(|&s| !title(s).is_empty() && title(s).to_lowercase() == lower))
        .or_else(|| SLEEP_BOXES.into_iter().find(|&s| !title(s).is_empty() && title(s).to_lowercase().contains(&lower)))
}

pub(crate) fn sleep_box_text(s: usize, f: &SleepFile, today: i64) -> String {
    let body = f.body.as_deref().unwrap_or("");
    if strip(body).is_empty() {
        return format!("Box {s} is empty.");
    }
    let age = sleep_age(f.day.as_deref(), today);
    let age = if age.is_empty() { age } else { format!(" {age}") };
    format!("<box {s} name=\"{}\"{age}>\n{}\n</box>", sleep_named(&f.title), sleep_kept(strip(body)))
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
    let kept: Vec<Value> = notes["proposals"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            let mut p = p.clone();
            if sleep_readable(&p) {
                p["sleeps"] = json!(whole(p.get("sleeps")));
            }
            p
        })
        .collect();
    notes.insert("proposals".into(), Value::Array(kept));
    (whole(v).filter(|n| *n >= 0).unwrap_or(0), notes)
}

fn sleep_readable(p: &Value) -> bool {
    p.as_object().is_some_and(|o| ["id", "text", "first_day"].iter().all(|k| o.contains_key(*k)))
        && whole(p.get("sleeps")).is_some_and(|n| n >= 1)
        && p.get("first").and_then(Value::as_f64).is_some()
}

const SLEEP_NOTES_CAP: u64 = 200_000;

fn sleep_next_id(entries: &[Value]) -> String {
    let top = entries
        .iter()
        .filter_map(|p| p.get("id").and_then(Value::as_str)?.strip_prefix('p').map(str::to_string))
        .filter(|d| (1..=9).contains(&d.len()) && d.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|d| d.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    format!("p{}", top + 1)
}

fn sleep_size(v: &Value) -> u64 {
    match v {
        Value::String(s) => s.len() as u64 + 2,
        Value::Array(a) => 2 + a.iter().map(|x| sleep_size(x) + 1).sum::<u64>(),
        Value::Object(o) => 2 + o.iter().map(|(k, x)| k.len() as u64 + 2 + sleep_size(x) + 2).sum::<u64>(),
        _ => 8,
    }
}

fn sleep_notes_fit(notes: &mut Map<String, Value>) {
    let mut size = sleep_size(&Value::Object(notes.clone()));
    for key in ["proposals", "mixed"] {
        let all = notes.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
        let mut keep = Vec::new();
        for x in all {
            if size > SLEEP_NOTES_CAP && (key == "mixed" || (sleep_readable(&x) && truthy(x.get("applied")))) {
                size -= sleep_size(&x) + 1;
            } else {
                keep.push(x);
            }
        }
        notes.insert(key.into(), Value::Array(keep));
    }
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
    let digits_at = |i: usize| c[i..].iter().take(4).take_while(|x| digit(**x).is_some()).count();
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

fn sleep_renumbered(got: &mut Map<String, Value>, part: &[Line], log: &Log, name: &str) {
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
    log.push(json!({"call": format!("read {name}"), "renumbered": true, "part": [part[0].eid, part[part.len() - 1].eid]}));
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
    let mut keys: std::collections::HashSet<(usize, String, Option<usize>)> = Default::default();
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
            || !keys.insert((at, kind.to_string(), src))
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
    call_time: std::time::Duration,
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
                deadline: t0 + self.call_time,
            };
            let raced = tokio::time::timeout(self.call_time, self.caller.call(&c)).await.unwrap_or_else(|_| {
                let secs = self.call_time.as_secs_f64();
                let shown = if secs.fract() == 0.0 { format!("{}", secs as u64) } else { format!("{secs}") };
                Err(ModelCallError::timeout(format!("the sleep's model call ran past {shown} s")))
            });
            match raced {
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
        mut cut: Option<&mut bool>,
    ) -> Result<Option<Map<String, Value>>, ModelCallError> {
        for attempt in 1..=2 {
            let sent = if attempt == 1 { user.to_string() } else { format!("{user}{SLEEP_NOT_JSON}") };
            let (text, meta) = self.ask(system, &sent).await?;
            let at_length = meta.get("stop").and_then(Value::as_str) == Some("max_tokens");
            let mut entry = Map::new();
            entry.insert("call".into(), json!(key));
            entry.insert("attempt".into(), json!(attempt));
            entry.extend(meta);
            log.push(Value::Object(entry));
            if let Some(got) = sleep_parse_json(&text) {
                return Ok(Some(got));
            }
            seen(text);
            if at_length && attempt == 1 {
                if let Some(c) = cut.as_deref_mut() {
                    *c = true;
                    return Ok(None);
                }
            }
        }
        Ok(None)
    }

    async fn ask_json(&self, system: &str, user: &str, key: &str, log: &Log) -> Result<Option<Map<String, Value>>, ModelCallError> {
        self.ask_json_seeing(system, user, key, log, |_| {}, None).await
    }

    async fn ask_json_logged(
        &self,
        system: &str,
        user: &str,
        key: &str,
        log: &Log,
        cut: Option<&mut bool>,
    ) -> Result<Option<Map<String, Value>>, ModelCallError> {
        let mut last: Option<String> = None;
        let got = self.ask_json_seeing(system, user, key, log, |t| last = Some(t), cut).await?;
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
    let core = sleep_kept(core);
    let core = if core.is_empty() { "(empty)" } else { &core };
    let (mut found, mut dropped, mut mixed, mut failed) = (Vec::new(), 0, Vec::new(), None);
    // What is left to read, the next piece last: the piece, its name in the log, and how often it was halved.
    let mut left: Vec<(&[Line], String, usize)> = vec![(part, k.to_string(), 0)];
    while let Some((piece, name, depth)) = left.pop() {
        let user =
            format!("<core>\n{core}\n</core>\n<boxes>\n{titles}\n</boxes>\n<memories>\n{}\n</memories>", sleep_lines(piece));
        let mut got = None;
        let mut cut = false;
        let halves = piece.len() >= 4 && depth < SLEEP_SPLIT_DEPTH;
        for attempt in 1..=(if depth == 0 { 3 } else { 1 }) {
            let key = if attempt == 1 { format!("read {name}") } else { format!("read {name} again {attempt}") };
            let watch = if attempt == 1 && (halves || depth > 0) { Some(&mut cut) } else { None };
            got = n.read.ask_json_logged(&system, &user, &key, &n.log, watch).await?;
            if let Some(g) = got.as_mut() {
                sleep_renumbered(g, piece, &n.log, &name);
                break;
            }
            if cut {
                break;
            }
        }
        if got.is_none() && cut && halves {
            let at = sleep_half(piece);
            n.log.push(json!({"call": format!("read {name}"), "cut": true, "halves": [at, piece.len() - at]}));
            left.push((&piece[at..], format!("{name}b"), depth + 1));
            left.push((&piece[..at], format!("{name}a"), depth + 1));
            continue;
        }
        let known = piece.iter().map(|m| m.eid.clone()).collect();
        let said = piece.iter().filter(|m| m.mine).map(|m| m.eid.clone()).collect();
        let (items, lost) = sleep_checked_items(got.as_ref(), &known, &said);
        let (marks, mixed_dropped) = sleep_checked_mixed(got.as_ref(), piece);
        if mixed_dropped > 0 {
            n.log.push(json!({"call": format!("read {name}"), "mixed_dropped": mixed_dropped}));
        }
        found.extend(items);
        dropped += lost;
        mixed.extend(marks);
        if got.is_none() {
            failed = Some(k);
        }
    }
    Ok((found, dropped, mixed, failed))
}

/// Where a part of four memories or more is halved: after the memory that brings the first half to half the
/// tokens, with two memories at least on each side.
fn sleep_half(part: &[Line]) -> usize {
    let total = token_sum(part);
    let mut size = 0u64;
    for (i, m) in part.iter().enumerate() {
        size = size.saturating_add(m.tokens);
        if size >= total - size {
            return (i + 1).clamp(2, part.len() - 2);
        }
    }
    part.len() - 2
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

async fn sleep_settle(
    n: &Night<'_>,
    core: &str,
    sec: &str,
    text: &str,
    asks: &mut usize,
) -> Result<(Option<String>, String), ModelCallError> {
    let text = sleep_as_lines(text);
    let Some(old) = section_of(core, sec) else { return Ok((Some(text), "new".into())) };
    let mut held: Vec<String> = old.split('\n').map(strip).filter(|l| !l.is_empty()).map(str::to_string).collect();
    let tonight: Vec<&str> = text.split('\n').map(strip).filter(|l| !l.is_empty()).collect();
    if held.iter().filter(|ln| !tonight.contains(&ln.as_str())).count() <= SLEEP_OVERTURN {
        return Ok((Some(text), "kept".into()));
    }
    if *asks >= SLEEP_AFTER_ASKS {
        return Ok((None, "not asked".into()));
    }
    *asks += 1;
    let numbered = held.iter().enumerate().map(|(i, ln)| format!("{}. {}", i + 1, sleep_kept(ln))).collect::<Vec<_>>().join("\n");
    let system = fill(SLEEP_SETTLE, &[("being", Arg::Text(&n.run.being)), ("name", Arg::Text(&sleep_named(sec)))]);
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
    carried: Vec<(usize, usize, String, String)>,
}

fn sleep_put_back(new: &mut Files, files: &Files, k: usize, carried: &[(usize, usize, String, String)]) {
    new[k] = files[k].clone();
    for (src, dst, sec, text) in carried {
        if *dst == k && [*src, k].iter().all(|j| section_of(&new[*j].body, sec).is_none()) {
            new[*src].body = sleep_set_section(&new[*src].body, sec, text, true);
        }
    }
}

fn sleep_where(new: &Files, k: usize) -> String {
    if k == 0 { "core".into() } else { format!("box \"{}\"", sleep_named(&new[k].title)) }
}

fn sleep_cap(s: &SleepSettings, k: usize) -> u64 {
    if k == 0 { s.core_tokens } else { s.box_tokens }
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
    over: Option<&str>,
) -> Result<(), ModelCallError> {
    let count = &n.run.count;
    let where_ = sleep_where(new, k);
    let core_way = if k == 0 { ", unless you move a person who matters less to a box (say which, under \"moved\")" } else { "" };
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
    let user = format!("<{where_}>\n{}\n</{where_}>", sleep_kept(&new[k].body));
    let g = n.merge.ask_json(&system, &user, &format!("room {k}"), &n.log).await?.unwrap_or_default();
    let text = text_or_empty(g.get("text"));
    let mut body = strip(&text).to_string();
    body = if body.starts_with("## ") { sleep_join(&sleep_sections(&body)) } else { String::new() };
    let held = sleep_sections(&new[k].body);
    let given = sleep_sections(&body);
    let stored: Vec<String> = files
        .iter()
        .flat_map(|f| sleep_sections(&f.body))
        .filter(|(_, t)| !strip(t).is_empty())
        .map(|(h, _)| h)
        .collect();
    let moves: Vec<(usize, usize)> = if k == 0 && !body.is_empty() { sleep_room_moves(&held, g.get("moved")) } else { Vec::new() };
    let moved_at: Vec<usize> = moves.iter().map(|(_, j)| *j).collect();
    let (pair, doubt) = sleep_room_pairs(&held, &given, &moved_at, k == 0);
    // The file as the answer leaves it: (the section of the file it is, or None for a new one, name, text).
    let mut made: Vec<(Option<usize>, String, String)> = Vec::new();
    for ((name, text), j) in given.iter().zip(&pair) {
        if let Some(j) = *j {
            let mine = k == 0 && held[j].0 == SLEEP_ME;
            made.push((Some(j), held[j].0.clone(), if mine { held[j].1.clone() } else { text.clone() }));
        } else if k != 0 && !name.is_empty() {
            match made.iter_mut().rev().find(|(_, h, _)| h == name) {
                Some(same) => same.2 = strip(&format!("{}\n{text}", same.2)).to_string(),
                None => made.push((None, name.clone(), text.clone())),
            }
        }
    }
    if k == 0 && (!made.is_empty() || !moves.is_empty()) {
        for (j, (h, t)) in held.iter().enumerate() {
            if h == SLEEP_ME && !pair.contains(&Some(j)) {
                made.push((Some(j), SLEEP_ME.to_string(), t.clone()));
            }
        }
    }
    body = sleep_join(&made.iter().map(|(_, h, t)| (h.clone(), t.clone())).collect::<Vec<_>>());
    let mut boxes: std::collections::BTreeMap<usize, String> = Default::default();
    for (to, j) in &moves {
        let from = boxes.get(to).cloned().unwrap_or_else(|| new[*to].body.clone());
        boxes.insert(*to, sleep_set_section(&from, &held[*j].0, &held[*j].1, false));
    }
    let fits = !body.is_empty() && count.of(&body) <= cap && boxes.values().all(|b| count.of(b) <= n.run.s.box_tokens);
    let stays = |j: usize| {
        made.iter().any(|(at, _, t)| *at == Some(j) && !doubt.contains(&j) && !strip(t).is_empty())
            || moved_at.contains(&j)
            || (k == 0 && held[j].0 == SLEEP_ME)
    };
    let mut left: Vec<&String> = Vec::new();
    for (j, (h, t)) in held.iter().enumerate() {
        if !h.is_empty() && !strip(t).is_empty() && !stays(j) && stored.contains(h) && !left.contains(&h) {
            left.push(h);
        }
    }
    if fits && left.is_empty() {
        ledger.rooms.push(json!({"file": k, "before": used_now, "after": count.of(&body)}));
        new[k].body = body;
        for (to, b) in boxes {
            new[to].body = b;
        }
        ledger.carried.extend(moves.iter().map(|(to, j)| (k, *to, held[*j].0.clone(), held[*j].1.clone())));
    } else {
        let cause = if fits {
            let more = if left.len() > 1 { format!(" and {} more", left.len() - 1) } else { String::new() };
            format!("left out \"{}\"{more}", sleep_named(left[0]))
        } else {
            "came back unshaped or still too big".to_string()
        };
        let head = over.map_or_else(
            || format!("no room: {} of {} tokens", commas(used_now.into()), commas(cap.into())),
            str::to_string,
        );
        ledger.rooms.push(json!({"file": k, "before": used_now, "after": null}));
        ledger.refused.push(json!({
            "edit": {"file": k, "op": "room"},
            "why": format!("{head}, and the room made {cause}; the file stays as it was"),
        }));
        sleep_put_back(new, files, k, &ledger.carried);
    }
    Ok(())
}

fn sleep_boxes_fit(n: &Night<'_>, new: &mut Files, files: &Files, ledger: &mut Ledger) {
    for k in SLEEP_BOXES {
        if n.run.count.of(&new[k].body) > n.run.s.box_tokens {
            ledger.refused.push(json!({"edit": {"file": k, "op": "room"}, "why": "over its cap after a move; the box stays as it was"}));
            sleep_put_back(new, files, k, &ledger.carried);
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

fn sleep_dedupe(
    new: &mut Files,
    deduped: &mut Vec<Value>,
    carried: &mut Vec<(usize, usize, String, String)>,
    put_back: &[usize],
) {
    let in_core: Vec<String> = sleep_sections(&new[0].body).into_iter().map(|(h, _)| h).filter(|h| h != SLEEP_ME).collect();
    for k in SLEEP_BOXES {
        if put_back.contains(&k) {
            continue;
        }
        let mut names: Vec<String> = Vec::new();
        for (h, _) in sleep_sections(&new[k].body) {
            if !names.contains(&h) {
                names.push(h);
            }
        }
        for h in names {
            if in_core.contains(&h) {
                let (body, gone) = sleep_drop_section(&new[k].body, &h);
                new[k].body = body;
                deduped.push(json!({"file": k, "section": h}));
                carried.push((k, 0, h, gone.unwrap_or_default()));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn sleep_write_files(
    n: &Night<'_>,
    sleep_id: &str,
    files: &Files,
    new: &mut Files,
    ev: &[String],
    day: &str,
    ledger: &mut Ledger,
    deduped: &mut Vec<Value>,
) -> Result<std::collections::BTreeMap<i64, i64>, SleepError> {
    let s = n.run.s;
    let mut made_room: Vec<usize> = Vec::new();
    let mut put_back: Vec<usize> = Vec::new();
    let mut refusals = 0usize;
    loop {
        sleep_dedupe(new, deduped, &mut ledger.carried, &put_back);
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
        refusals += 1;
        if refusals > SLEEP_WRITE_REFUSALS {
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
        let k = match named.filter(|x| slots.contains(x)) {
            Some(x) => x,
            None => {
                let fill = |w: &Value| {
                    let slot = w["slot"].as_i64().unwrap_or(0);
                    n.run.count.of(w["body"].as_str().unwrap_or("")) as f64 / sleep_cap(&s, slot as usize) as f64
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
        let cap = sleep_cap(&s, ku);
        if made_room.contains(&ku) {
            ledger.refused.push(json!({"edit": {"file": k, "op": "room"},
                                       "why": "no room: the service counts it over its cap; the file stays as it was"}));
            sleep_put_back(new, files, ku, &ledger.carried);
            put_back.push(ku);
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
        sleep_room(n, new, files, ku, used_now, aim, ledger, Some("no room: the service counts it over its cap")).await?;
        sleep_boxes_fit(n, new, files, ledger);
    }
}

async fn sleep_done_flow(n: &Night<'_>, sleep_id: &str) -> Result<Value, WosError> {
    if !n.done {
        return Ok(Value::Null);
    }
    n.run.client.post_idem("/api/v1/memory/sleep/done", json!({"user_id": n.run.store, "sleep_id": sleep_id}), None).await
}

fn sleep_titles<'a>(title: impl Fn(usize) -> &'a String) -> String {
    SLEEP_BOXES
        .iter()
        .map(|&k| {
            let name = sleep_named(title(k));
            format!("{k}: {}", if name.is_empty() { "(empty)" } else { &name })
        })
        .collect::<Vec<_>>()
        .join("\n")
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
        body: sleep_core(view[0].body.as_deref().unwrap_or("")),
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
    let titles = sleep_titles(|k| &files[k].title);
    let held: Vec<Value> = notes["proposals"].as_array().cloned().unwrap_or_default();
    let mut proposals: Vec<Value> = held.iter().filter(|p| sleep_readable(p)).cloned().collect();
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
    let mut unmerged = false;
    if !items.is_empty() || !ripe.is_empty() {
        let blocks = SLEEP_BOXES
            .iter()
            .map(|&k| {
                let f = &files[k];
                format!(
                    "<box {k} name=\"{}\" size=\"{} of {} tokens\">\n{}\n</box {k}>",
                    if f.title.is_empty() { "(empty)".to_string() } else { sleep_named(&f.title) },
                    commas(count.of(&f.body).into()),
                    commas(s.box_tokens.into()),
                    if f.body.is_empty() { "(empty)".to_string() } else { sleep_kept(&f.body) }
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
                    sleep_named(&field(p, "id")),
                    sleep_shown(&field(p, "text")),
                    field(p, "sleeps"),
                    sleep_named(&field(p, "first_day"))
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let ripe_text = ripe
            .iter()
            .map(|p| format!("{}: {}", sleep_named(&field(p, "id")), sleep_shown(&field(p, "text"))))
            .collect::<Vec<_>>()
            .join("\n");
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
            if core.is_empty() { "(empty)".to_string() } else { sleep_kept(&core) },
            if prop_text.is_empty() { "(none)" } else { &prop_text },
            if ripe_text.is_empty() { "(none)" } else { &ripe_text },
            items_json(&items)
        );
        let mut got = n.merge.ask_json_logged(&system, &user, "merge", log, None).await?;
        if got.is_none() {
            got = n.merge.ask_json_logged(&system, &format!("{user}{SLEEP_SHORTER}"), "merge shorter", log, None).await?;
        }
        unmerged = got.is_none();
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
        let ripe_ids: Vec<Value> = ripe.iter().filter_map(|p| p.get("id").filter(|id| id.is_string()).cloned()).collect();
        let is_ripe_id = |v: Option<&Value>| ripe_ids.iter().any(|id| same(id, v.unwrap_or(&Value::Null)));
        let tonight: Vec<String> = items
            .iter()
            .filter(|it| it.kind == "person")
            .map(|it| strip(&it.topic).to_lowercase())
            .collect();
        let about_tonight = |name: &str| {
            let nm = strip(name).to_lowercase();
            tonight.iter().any(|t| sleep_speaks_of(t, &nm))
        };
        let own: Vec<String> =
            [me, being, assistant.as_str()].iter().map(|x| strip(x).to_lowercase()).filter(|x| !x.is_empty()).collect();
        let selfish = |name: &str| own.contains(&strip(name).to_lowercase());
        let mut asks = 0usize;
        let mut unasked = 0usize;
        let section_name = |v: Option<&Value>| sleep_heading(&text_or_empty(v)).to_string();
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
            let mut made_ripe: Option<Value> = None;
            match slot {
                _ if sleep_heading_broken(eo, op) => {
                    why = Some("a heading or a title holds a line break or a control character".into())
                }
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
                    let sec = if slot == 0 && selfish(&sec) { me.to_string() } else { sec };
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
                        let mut not_asked = false;
                        if slot == 0 && sec != me {
                            let (t, how) = sleep_settle(n, &core, &sec, text.as_deref().unwrap_or(""), &mut asks).await?;
                            not_asked = how == "not asked";
                            settled.push(json!({"section": sec, "how": how}));
                            text = t;
                        }
                        match text {
                            None if not_asked => {
                                why = Some(format!("{SLEEP_ASKED_OUT}; this rewrite was not settled, so it is not made"))
                            }
                            None => {
                                why = Some("one night overturns one line of a person; the sorting came back unshaped".into())
                            }
                            Some(text) => {
                                let raw = slot == 0 && sec == me;
                                new[slot].body = sleep_set_section(&new[slot].body, &sec, &text, raw);
                                touched.insert((slot, sec));
                                if raw {
                                    made_ripe = eo.get("ripe").cloned();
                                }
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
                        let mut here: Vec<(String, String)> = Vec::new();
                        for (src, dst, moved_sec, moved_text) in &ledger.carried {
                            if *dst == slot {
                                here.retain(|(h, _)| h != moved_sec);
                                here.push((moved_sec.clone(), moved_text.clone()));
                            } else if *src == slot {
                                here.retain(|(h, _)| h != moved_sec);
                            }
                        }
                        for (moved_sec, moved_text) in here {
                            if section_of(&new[slot].body, &moved_sec).is_none() {
                                new[slot].body = sleep_set_section(&new[slot].body, &moved_sec, &moved_text, false);
                            }
                        }
                        for (h, _) in sleep_sections(&new[slot].body) {
                            touched.insert((slot, h));
                        }
                    }
                }
                Some(slot) if op == "move_section" => {
                    let to = slot_of(eo.get("to"));
                    let sec = section_name(eo.get("section"));
                    match to {
                        Some(to) if to != slot && !((slot == 0 || to == 0) && selfish(&sec)) => {
                            let back = to == 0 && ledger.carried.iter().any(|(src, _, left, _)| *src == 0 && *left == sec);
                            let (body, text) = sleep_drop_section(&new[slot].body, &sec);
                            match text {
                                _ if back => why = Some("a section that left the core tonight is not moved back in the same sleep".into()),
                                None => why = Some("no such section".into()),
                                Some(_) if section_of(&new[to].body, &sec).is_some() => {
                                    why = Some("move needs a section the file does not hold".into())
                                }
                                Some(text) => {
                                    new[slot].body = body;
                                    new[to].body = sleep_set_section(&new[to].body, &sec, &text, false);
                                    ledger.carried.push((slot, to, sec.clone(), text));
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
                    if let Some(ripe_id) = &made_ripe {
                        for p in proposals.iter_mut() {
                            if same(p.get("id").unwrap_or(&Value::Null), ripe_id) {
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
        sleep_dedupe(&mut new, &mut deduped, &mut ledger.carried, &[]);
        for (k, h) in &touched {
            let k = *k;
            let Some(t) = section_of(&new[k].body, h).filter(|t| !t.is_empty()) else { continue };
            if k == 0 && h == me {
                continue;
            }
            if asks >= SLEEP_AFTER_ASKS {
                unasked += 1;
                continue;
            }
            asks += 1;
            let where_ = sleep_where(&new, k);
            let numbered = t
                .split('\n')
                .filter(|ln| !strip(ln).is_empty())
                .enumerate()
                .map(|(i, ln)| format!("{}: {}", i + 1, sleep_kept(ln)))
                .collect::<Vec<_>>()
                .join("\n");
            let system = fill(SLEEP_TIDY, &[("being", Arg::Text(being)), ("where", Arg::Text(&where_))]);
            let user = format!("<section heading=\"{}\">\n{numbered}\n</section>", sleep_named(h));
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
                if asks >= SLEEP_AFTER_ASKS {
                    unasked += 1;
                    continue;
                }
                asks += 1;
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
                let user = format!("<section heading=\"{}\">\n{}\n</section>", sleep_named(&h), sleep_kept(&t));
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
        if unasked > 0 {
            ledger.refused.push(json!({"edit": {"op": "fold"},
                                       "why": format!("{SLEEP_ASKED_OUT}; {unasked} more were not asked, and their sections stay as they are")}));
        }
        for k in SLEEP_SLOTS {
            let cap = sleep_cap(&s, k);
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
                .position(|q| q.get("id").is_some_and(|id| id.is_string() && id == same_as))
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
                    let pid = sleep_next_id(&[held.as_slice(), proposals.as_slice()].concat());
                    proposals.push(json!({"id": pid, "text": text, "sleeps": 1, "first": now,
                                          "first_day": today, "last_day": today, "span_end": span_end}));
                    seen_now.push(json!(pid));
                }
            }
        }
    }
    let mut worked = proposals.iter();
    let mut kept: Vec<Value> =
        held.iter().map(|p| if sleep_readable(p) { worked.next().cloned().unwrap_or(Value::Null) } else { p.clone() }).collect();
    kept.extend(worked.cloned());
    notes.insert("proposals".into(), Value::Array(kept));
    sleep_notes_fit(&mut notes);
    let ev: Vec<String> =
        used.iter().filter_map(|e: &String| by_eid.get(e.as_str()).map(|m| m.mid.clone())).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    let written = sleep_write_files(n, &sid, &files, &mut new, &ev, &day, &mut ledger, &mut deduped).await?;
    if notes != before {
        client
            .post_idem(
                "/api/v1/memory/self/notes/write",
                json!({"user_id": store, "base_version": notes_version, "notes": notes}),
                None,
            )
            .await?;
    }
    let ended = if unread.is_empty() && !unmerged { sleep_done_flow(n, &sid).await? } else { Value::Null };
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
    let titles = sleep_titles(|k| &view[k].title);
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
    Ok(Asker { caller: caller.clone(), model, max_tokens, call_time: SLEEP_CALL_TIME })
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
        if sleep_date(&day).is_none() || !clock || sleep_when(today).is_none() {
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
        None => Asker { caller: read.caller.clone(), model: read.model.clone(), max_tokens: tokens, call_time: read.call_time },
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
mod tests;
