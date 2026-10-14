# Wontopos — long-term memory for AI agents

```toml
[dependencies]
wontopos = "2"
tokio = { version = "1", features = ["full"] }   # the examples below are async
serde_json = "1"                                 # metadata is passed as JSON
```

Get an API key in the [console](https://wontopos.com). Keys look like `wos-live-...`;
`Client::from_env()` reads `WONTOPOS_API_KEY` from the environment instead.

```rust
use wontopos::{Client, WosError};

#[tokio::main]
async fn main() -> Result<(), WosError> {
    let mem = Client::new("wos-live-...");

    // Each end-user / agent / topic gets its own store — create it once.
    // (A "default" store already exists, so you can skip this and omit the id.)
    mem.create_store("alice").await?;
    mem.add("she prefers tea over coffee", "alice", serde_json::json!({})).await?;

    // one call → short-term + long-term + context, ready for your LLM prompt
    let ctx = mem.recall("what does alice drink?", "alice").await?;
    println!("{:?}", ctx);
    Ok(())
}
```

## Why

- **The same in every language** — identical recall whichever language a memory was written in (Korean · Japanese · Chinese · English).
- **No LLM reads your memories** — no language model reads your stored memories, on any model. You pay retrieval, not generation.
- **Bounded retrieval** — `recall()` returns the recent turns, at most `limit` long-term memories (default 10, up to 20) and at most `context_limit` rows of the surrounding conversation (default 10, up to 20), however much you've stored.

## Methods

| Method | What it does |
|---|---|
| **Write** | |
| `add(content, user_id, metadata)` · `add_with` | Store one memory. Alias `store`. `add_with` takes an `image`. |
| `add_turn(user_msg, assistant_msg, user_id)` | Store one exchange of a conversation. |
| `add_bulk(content, user_id, category)` · `add_bulk_with` | Backfill a long history in one call. |
| `update(old_memory_id, new_content, user_id)` | Replace a fact that changed. The old version stays in its history. |
| `*_idempotent` | The same writes with an idempotency key of your own. |
| **Read** | |
| `recall(query, user_id)` · `recall_opts` · `recall_with` | The recent turns, the memories that match and the conversation around the best one, in one call. |
| `search(query, user_id, limit)` · `search_opts` · `search_with` | The memories that match, best first. |
| `search_full` · `search_full_with` | The same search with each part kept apart, `images` and `verify_used` included. |
| `search_self` · `search_self_with` | The assistant's own words, kept apart from what others said. |
| `history(user_id)` | The recent conversation. |
| `get(user_id, memory_id)` | One memory by id. |
| `list_memories` · `export_memories` | Every memory in a store, a page at a time or all at once. |
| `lineage(user_id, memory_id)` | The edits that followed one memory. |
| `revisions(user_id)` · `revisions_page` | How many of a store's memories were edited, and which. |
| `stats(user_id)` | Counts for a store. |
| `engram` · `engram_with` · `list_engrams()` | Built-in retrieval routines, and the ones a model can run. |
| **Photos** | |
| `list_images` · `export_images` | A store's image memories, newest first: a page at a time, or all at once. For a large store, walk `list_images` with its cursors. |
| `get_image(user_id, memory_id)` | The bytes of one image, and its content type. |
| `forget_image(user_id, memory_id, preview)` | Remove the photo from a memory. `preview` first says whether the memory itself stays. |
| **People** | |
| `add_speaker` · `list_speakers` · `remove_speaker` | Register, list and unregister the people a store hears from. |
| `by_speaker(speaker, user_id, ...)` | What one person said, newest first. |
| **Stores** | |
| `create_store` · `list_stores` · `delete_store` | One isolated memory space per end-user, agent or topic. |
| `delete(user_id, memory_id)` · `delete_all(user_id)` | Delete one memory, or every memory in a store. |
| **Account** | |
| `list_models()` | The models you can use, and what each can do. |
| `usage(days)` | What this key, its workspace and its stores have spent. Free. |
| `ping()` | Check the key and the connection. |
| **Client** | |
| `with_model` · `with_user` · `with_deadline` · `with_timeout` · `with_retries` | A copy of the client with another model, store, budget, timeout or retry count. |
| `Client::from_env()` · `with_base_url` · `rate_limit()` | A client with the key from the environment, or another API host; the last call's rate-limit numbers. |

Every method that works on a store takes a `user_id` — it names the **store**: one isolated memory space per end-user, agent, or topic, then per workspace. WHO said each memory inside a store is the `speaker` tag below — storing the assistant's own words never needs a separate id.

## Putting recall into a prompt

```rust
let ctx = mem.recall(&user_message, "alice").await?;

let mut system = String::from("What you remember about this user, most relevant first:\n");
for m in ctx.memories() {
    system += &format!("- {}\n", m.content);
}
system += "\nAround the most relevant memory:\n";
for row in ctx.context_rows() {
    system += &format!("{row}\n");
}
system += "\nThe conversation so far:\n";
for t in ctx.turns() {
    system += &format!("{}: {}\n", t.role, t.content);
}

// Call your language model with `system` and the user's message, then store the exchange:
mem.add_turn(&user_message, &reply, "alice").await?;
```

Each row of `context_rows()` starts with `[before] `, `[match] ` or `[after] `: where it
sits next to the best memory. Keep every list in the order it arrives.

## Who said it (speakers)

Every memory can carry a speaker: `"me"` for the assistant's own words, or a
person's name. Speakers are explicit, like stores: register a person once,
then store under their name — a typo can never silently become a new person.
`search_with` accepts a speaker too, to recall one person only.

```rust
mem.add_speaker("Bob", "alice").await?;      // once per person
mem.add("I promised to send the report on Friday", "alice", serde_json::json!({"speaker": "me"})).await?;
mem.add("Bob said the deadline moved to Tuesday", "alice", serde_json::json!({"speaker": "Bob"})).await?;
mem.search_with("what did Bob say about deadlines?", "alice", 10, serde_json::json!({"speaker": "Bob"})).await?;
```

Memories arrive best-first, and the assistant's own words and image memories, when there
are any, follow them in their own order. Take the slice in the order given. `similarity` is a raw
closeness score, not the ranking key: what produces the order is internal and is not
returned, so sorting by it makes results worse. There is no `score` field.

`list_speakers` shows the registered people with per-person memory counts;
`remove_speaker` unregisters (memories stay; the tag goes, except on `tablet-2`, which keeps it on memories already stored). A store registers
up to 50 people to start (a limit we plan to raise); `"me"` never needs
registration and never counts against it.

## Photos

On a model whose `list_models()` capabilities include `images`, a memory can carry a photo.
The caption is still required.

```rust
use base64::Engine as _; // the base64 crate

let data = base64::engine::general_purpose::STANDARD.encode(std::fs::read("beach.jpg").expect("read"));
let stored = mem
    .add_with("At the beach with Bob", "alice", serde_json::json!({}),
              serde_json::json!({"image": {"data": data, "taken_at": "2026-08-14"}}))
    .await?;

let (bytes, content_type) = mem.get_image("alice", stored["id"].as_str().unwrap_or_default()).await?;
```

Both edges must be 700px or more, the long edge at most 2500px and the file at most 7MB;
anything else is refused (400) with the reason. A long edge over 1568px is stored at
1568px, so keep your own copy of the original. `search` returns image memories after
the others, and `list_images` pages through them newest first. `forget_image` removes the
photo, and it can remove the whole memory with it: call it with `preview` set first, and
`memory_kept` says whether the memory stays.

## Export, edit history and usage

- **Export:** `export_memories(user_id)` returns every memory in a store, the text you
  stored and its metadata; `export_images(user_id, None)` every image memory.
- **Edit history:** `update` replaces a fact that changed. `lineage(user_id, memory_id)`
  lists the versions from the id you pass forward, oldest first, and marks the one in force
  with `is_current`; pass the first version's id to see the whole history.
  `revisions(user_id)` counts how many of a store's memories were edited.
- **Usage:** `usage(days)` reports this key's spend, the workspace's spend over the window,
  the stores that spent the most, and the balance. It is free, and works at a zero balance.

## Recall caching

Opt in per search and a repeated query reuses the previous result at 10% of the
normal rate; an extended one reuses part of it (Tablet and Scroll models).

It is not free to turn on: the FIRST call writes the cache and bills the query
tokens at 2x for a `5m` TTL, 3x for `1h`. The same query again inside the TTL bills
its query and its results at 0.1x. A query that extends the cached one (its text plus
more) bills the cached part at 0.1x, the new part at the write rate and its results
at the normal rate. A store keeps one cached query per model and set of options, so a
different query replaces it. So it pays for a query you repeat or extend, and costs
more for one you issue once — do not switch it on globally. Any write to the store invalidates its cache
at once, so a hit can never predate a new memory.

```rust
let hits = mem.search_with("...the conversation so far...", "alice", 10,
                           serde_json::json!({"cache_control": {"ttl": "5m"}})).await?;  // or "1h"
```

## Reliability

Built in, no configuration needed:

- **Automatic retries** — 429 and a 409 for a write already in flight always, and 408 /
  502 / 503 / 504 or a connection error only when a retry cannot double-process a write.
  The calls that are safe to send again retry all four: the reads sent as POST (`search`,
  `recall`, `get`, `list_memories`, `stats`, ...), the GET and DELETE calls (`ping`,
  `list_stores`, `list_speakers`, `list_models`, `list_engrams`, `usage`, `delete_store`,
  `remove_speaker`, `forget_image`), and the memory writes (`add`, `add_turn`, `add_bulk`,
  `update` and their variants). Those carry an idempotency key, so the service answers a
  repeat with the first attempt's result instead of storing again. The other writes sent
  as POST (`create_store`, `add_speaker`, `delete`, `delete_all`) get 429, that 409 and
  connect-level failures only. Twice, with exponential backoff + jitter, honoring the
  server's `Retry-After` up to 30s. Tune with `client.with_retries(n)`; `0` disables.
- **Your own retries** — the `*_idempotent` methods take a key of yours, for a retry that
  is yours (a job that is re-run, a queue that redelivers). The service keeps a key for 24
  hours and answers a repeat with the first response, marked `"replayed": true`. When the
  first attempt did not come back with an answer, so whether it was applied is not known,
  a repeat is refused with a 409 rather than stored twice: check before sending
  it again under a new key. Derive the key from what you store
  (`format!("import:{}", row.id)`), never a constant.
- **Redirects refused** — the API key never follows a 3xx to another host.
- **Timeouts** — 30s per attempt, 10s connect (`client.with_timeout(secs)`), and a
  total budget for the whole call across every retry with
  `client.with_deadline(Duration)`. At the defaults one call can hold for 30s +
  backoff + 30s + backoff + 30s, which a handler with five seconds cannot use.
- **Key never in logs** — `{:?}` on a `Client` masks the API key.
- **Wipe guard** — `delete` with an empty `memory_id` errors instead of silently
  meaning "delete everything"; wiping a store is only ever the explicit
  `delete_all` / `delete_store`.

## Security

Built in, none of it configurable off:

- **TLS 1.2 floor** and certificate verification that cannot be disabled.
- **Redirects refused** — a 3xx is an error, so the key never follows one to
  another host.
- **Response size cap** — anything over 64MB is refused instead of buffered.
- **Key hygiene** — keys are trimmed (a stray newline from a file otherwise
  becomes a mystery 401); model names are validated before they reach a header.
- **`Client::from_env()`** reads `WONTOPOS_API_KEY` (or `WOS_API_KEY`) — keep
  keys out of source code.
- Plain-HTTP base URLs on non-local hosts warn.

## Errors

Any non-2xx response is `WosError::Api { status, message }`. When the server sent
a request id, `message` ends with `(request_id: ...)` — include it when
contacting support.

```rust
match mem.search("...", "alice", 10).await {
    Ok(memories) => { /* ... */ }
    Err(WosError::Api { status: 401, .. }) => eprintln!("API key invalid or revoked"),
    Err(WosError::Api { status: 429, .. }) => eprintln!("Rate limited"),  // already retried twice by then
    Err(WosError::Api { status, message }) => eprintln!("HTTP {status}: {message}"),
    Err(WosError::Network(e)) => eprintln!("network: {e}"),
}
```

A retired model answers `410`, and `e.is_gone()` is true: retrying cannot succeed, so
name a live model (`list_models()` lists them, with `retires_at` on one that is
scheduled to retire). Before the day, the client prints a warning once with the date the
model you use retires. `delete_store` still works under a retired model.

## A different API host

Point the client somewhere other than the default endpoint - a dedicated region,
a proxy of your own, or a local test server:

```rust
let mem = Client::with_base_url("wos-live-...", "https://api.example.com");
```

## Links

- Homepage: <https://wontopos.com>
- API reference: <https://wontopos.com/en/why> (Developers tab)

## Reporting a bug

Found something wrong, or something that looks unsafe? Tell us — every report gets read.

- Bugs: <https://wontopos.com/contact?topic=bug>
- Security: <https://wontopos.com/contact?topic=security> (also published at
  [`/.well-known/security.txt`](https://wontopos.com/.well-known/security.txt))

Include the SDK version (the one in Cargo.toml) and the language. If it involves a store id or a
memory, describe the shape rather than pasting the contents — we do not need your
data to fix it.

## Changelog

The three clients release in lockstep — same version, same surface, same day. Patch
releases are additive. Ten inside 2.2 were not, deliberately and each with its
reason; the changelog lists them.

See [CHANGELOG.md](https://github.com/wontopos/wontopos-rust/blob/main/CHANGELOG.md).
