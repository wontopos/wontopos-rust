# Wontopos — long-term memory for AI agents

```toml
[dependencies]
wontopos = "2"
tokio = { version = "1", features = ["full"] }   # the examples below are async
serde_json = "1"                                 # metadata is passed as JSON
```

Get an API key in the [console](https://wontopos.com). Keys look like `wos-live-...`;
the client also reads `WONTOPOS_API_KEY` from the environment.

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
- **No LLM in the loop** — `store` / `search` / `recall` never call a language model. You pay retrieval, not generation.
- **Bounded retrieval** — `recall()` returns a small, fixed-size slice regardless of how much you've stored (~1,000 tokens on `tablet-2`, the default engine).

## Methods

`add` · `add_turn` · `add_bulk` · `update` · `search` · `search_full` · `recall` · `history` · `stats` · `get` · `list_memories` · `delete` · `delete_all`

All methods take a `user_id` — it names the **store**: one isolated memory space per end-user, agent, or topic, then per account. WHO said each memory inside a store is the `speaker` tag below — storing the assistant's own words never needs a separate id.

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

Results arrive best-first — take the slice in the order given. `similarity` is a raw
closeness score, not the ranking key: what produces the order is internal and is not
returned, so sorting by it makes results worse. There is no `score` field.

`list_speakers` shows the registered people with per-person memory counts;
`remove_speaker` unregisters (memories stay, the tag goes). A store registers
up to 50 people to start (a limit we plan to raise); `"me"` never needs
registration and never counts against it.

## Recall caching

Opt in per search and repeated or extended queries reuse the previous result
at 10% of the normal rate (Tablet and Scroll models).

It is not free to turn on: the FIRST call writes the cache and bills the query
tokens at 2x for a `5m` TTL, 3x for `1h`. Only hits inside the TTL bill at 0.1x.
So it pays for a query you repeat or extend, and costs more for one you issue
once — do not switch it on globally. Any write to the store invalidates its cache
at once, so a hit can never predate a new memory.

```rust
let hits = mem.search_with("...the conversation so far...", "alice", 10,
                           serde_json::json!({"cache_control": {"ttl": "5m"}})).await?;  // or "1h"
```

## Reliability

Built in, no configuration needed:

- **Automatic retries** — 429 always, and 502 / 503 or a connect error only when a
  retry cannot double-process a write. The writes and the searches are POSTs, and a
  502 on one of those may have been returned *after* the service already stored and
  billed it, so those get 429 and connect-level failures only. The reads and the
  deletes that address a whole store — `ping`, `list_stores`, `list_speakers`,
  `list_models`, `list_engrams`, `delete_store`, `remove_speaker`, `forget_image` —
  are GET or DELETE and do retry a 502. Twice, with exponential backoff + jitter,
  honoring the server's `Retry-After`. Tune with `client.with_retries(n)`;
  `0` disables.
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

Include the SDK version (`the version in Cargo.toml`) and the language. If it involves a store id or a
memory, describe the shape rather than pasting the contents — we do not need your
data to fix it.

## Changelog

The three clients release in lockstep — same version, same surface, same day. Patch
releases are additive: nothing is removed or reordered within a minor line, and a
release that bends that says so at the top of its own entry.

See [CHANGELOG.md](https://github.com/wontopos/wontopos-rust/blob/main/CHANGELOG.md).
