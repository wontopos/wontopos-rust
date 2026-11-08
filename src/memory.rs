use super::*;

fn search_fields(mut obj: serde_json::Map<String, serde_json::Value>, opts: &SearchOpts) -> serde_json::Map<String, serde_json::Value> {
    if let Some(v) = opts.verify {
        obj.insert("verify".into(), serde_json::json!(v));
    }
    if let Some(n) = opts.max_images {
        obj.insert("max_images".into(), serde_json::json!(n));
    }
    obj
}

impl Client {
    /// The available models, each with its capabilities.
    pub async fn list_models(&self) -> Result<Vec<serde_json::Value>, WosError> {
        let v = self.request(reqwest::Method::GET, "/api/v1/models", None, None).await?;
        Ok(records(&v, "models"))
    }

    /// The engrams and delivery forms the selected model can run.
    pub async fn list_engrams(&self) -> Result<serde_json::Value, WosError> {
        let v = self.request(reqwest::Method::GET, "/api/v1/engram", None, None).await?;
        Ok(with_lists(v, &["engrams", "forms"]))
    }

    /// Store one memory.
    pub async fn add(&self, content: &str, user_id: impl Into<Option<&str>>, metadata: serde_json::Value) -> Result<serde_json::Value, WosError> {
        self.store_one(content, user_id, metadata, serde_json::json!({}), None, &WriteOpts::default()).await
    }

    /// Alias of `add`.
    pub async fn store(&self, content: &str, user_id: impl Into<Option<&str>>, metadata: serde_json::Value) -> Result<serde_json::Value, WosError> {
        self.add(content, user_id, metadata).await
    }

    /// [`Client::add`] with extra body fields merged in.
    pub async fn add_with(&self, content: &str, user_id: impl Into<Option<&str>>, metadata: serde_json::Value, extra: serde_json::Value) -> Result<serde_json::Value, WosError> {
        self.store_one(content, user_id, metadata, extra, None, &WriteOpts::default()).await
    }

    /// [`Client::add_with`] and [`Client::add_idempotent`] in one call.
    pub async fn add_with_idempotent(
        &self, content: &str, user_id: impl Into<Option<&str>>, metadata: serde_json::Value, extra: serde_json::Value, idempotency_key: &str,
    ) -> Result<serde_json::Value, WosError> {
        self.store_one(content, user_id, metadata, extra, Some(idempotency_key), &WriteOpts::default()).await
    }

    async fn store_one(
        &self, content: &str, user_id: impl Into<Option<&str>>, metadata: serde_json::Value, extra: serde_json::Value,
        idempotency_key: Option<&str>, opts: &WriteOpts,
    ) -> Result<serde_json::Value, WosError> {
        check_metadata_keys(&metadata)?;
        let mut metadata = metadata;
        if let Some(who) = metadata.get("speaker").and_then(|s| s.as_str()).map(|s| written_speaker(s, "speaker")).transpose()? {
            metadata["speaker"] = serde_json::json!(who);
        }
        refuse_turn_fields(opts)?;
        let user_id = self.uid(user_id.into())?;
        warn_on_unknown_metadata(&metadata);
        let mut obj = extra.as_object().cloned().unwrap_or_default();
        if let Some(image) = obj.get_mut("image") {
            normalize_image_field(image)?;
        }
        obj.insert("user_id".into(), serde_json::json!(user_id));
        obj.insert("content".into(), serde_json::json!(content));
        obj.insert("metadata".into(), if metadata.is_null() { serde_json::json!({}) } else { metadata });
        self.write_extras(&mut obj, opts)?;
        window_line(&mut obj, opts)?;
        self.post_idem("/api/v1/memory/store", serde_json::Value::Object(obj), idempotency_key)
            .await
    }

    /// [`Client::add`] with per-call options.
    pub async fn add_opts(
        &self, content: &str, user_id: impl Into<Option<&str>>, metadata: serde_json::Value, opts: &WriteOpts,
    ) -> Result<serde_json::Value, WosError> {
        self.store_one(content, user_id, metadata, serde_json::json!({}), None, opts).await
    }

    /// Store a conversation turn: the person's message and the assistant's reply.
    pub async fn add_turn(&self, user_msg: &str, assistant_msg: &str, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        self.store_turn(user_msg, assistant_msg, user_id, &WriteOpts::default(), None).await
    }

    /// [`Client::add_turn`] with per-call options.
    pub async fn add_turn_opts(
        &self, user_msg: &str, assistant_msg: &str, user_id: impl Into<Option<&str>>, opts: &WriteOpts,
    ) -> Result<serde_json::Value, WosError> {
        self.store_turn(user_msg, assistant_msg, user_id, opts, None).await
    }

    async fn store_turn(
        &self, user_msg: &str, assistant_msg: &str, user_id: impl Into<Option<&str>>, opts: &WriteOpts, idempotency_key: Option<&str>,
    ) -> Result<serde_json::Value, WosError> {
        if js_trim(user_msg).is_empty() && js_trim(assistant_msg).is_empty() {
            return Err(refused("user_msg and assistant_msg are both blank: there is no turn to store."));
        }
        let user_id = self.uid(user_id.into())?;
        let mut obj = serde_json::Map::new();
        obj.insert("user_id".into(), serde_json::json!(user_id));
        obj.insert("user_msg".into(), serde_json::json!(user_msg));
        obj.insert("assistant_msg".into(), serde_json::json!(assistant_msg));
        refuse_window(opts, "a turn always enters the window")?;
        turn_fields(&mut obj, opts)?;
        self.write_extras(&mut obj, opts)?;
        self.post_idem("/api/v1/memory/store-turn", serde_json::Value::Object(obj), idempotency_key).await
    }

    /// Bulk-ingest a large blob of text in one call.
    pub async fn add_bulk(&self, content: &str, user_id: impl Into<Option<&str>>, category: &str) -> Result<serde_json::Value, WosError> {
        self.add_bulk_with(content, user_id, category, serde_json::json!({})).await
    }

    /// [`Client::add_bulk`] with extra body fields merged in.
    pub async fn add_bulk_with(
        &self, content: &str, user_id: impl Into<Option<&str>>, category: &str, extra: serde_json::Value,
    ) -> Result<serde_json::Value, WosError> {
        self.bulk(Some(content), user_id, category, extra, &BulkOpts::default(), None).await
    }

    /// [`Client::add_bulk_with`] and [`Client::add_bulk_idempotent`] in one call.
    pub async fn add_bulk_with_idempotent(
        &self, content: &str, user_id: impl Into<Option<&str>>, category: &str, extra: serde_json::Value, idempotency_key: &str,
    ) -> Result<serde_json::Value, WosError> {
        self.bulk(Some(content), user_id, category, extra, &BulkOpts::default(), Some(idempotency_key)).await
    }

    /// [`Client::add_bulk`] with per-call options.
    pub async fn add_bulk_opts(
        &self, content: impl Into<Option<&str>>, user_id: impl Into<Option<&str>>, category: &str, opts: &BulkOpts,
    ) -> Result<serde_json::Value, WosError> {
        self.bulk(content.into(), user_id, category, serde_json::json!({}), opts, None).await
    }

    async fn bulk(
        &self, content: Option<&str>, user_id: impl Into<Option<&str>>, category: &str, extra: serde_json::Value, opts: &BulkOpts,
        idempotency_key: Option<&str>,
    ) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let parts = bulk_parts(content, opts)?;
        let mut obj = extra.as_object().cloned().unwrap_or_default();
        obj.insert("user_id".into(), serde_json::json!(user_id));
        obj.insert("category".into(), serde_json::json!(category));
        obj.extend(parts);
        self.post_idem("/api/v1/memory/bulk-store", serde_json::Value::Object(obj), idempotency_key)
            .await
    }

    /// Supersede an old memory with new content.
    pub async fn update(&self, old_memory_id: &str, new_content: &str, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        self.supersede(old_memory_id, new_content, user_id, &WriteOpts::default(), None).await
    }

    /// [`Client::update`] with per-call options.
    pub async fn update_opts(
        &self, old_memory_id: &str, new_content: &str, user_id: impl Into<Option<&str>>, opts: &WriteOpts,
    ) -> Result<serde_json::Value, WosError> {
        self.supersede(old_memory_id, new_content, user_id, opts, None).await
    }

    async fn supersede(
        &self, old_memory_id: &str, new_content: &str, user_id: impl Into<Option<&str>>, opts: &WriteOpts, idempotency_key: Option<&str>,
    ) -> Result<serde_json::Value, WosError> {
        if opts.work_id.is_some() {
            return Err(refused("work_id is for add and add_turn: a correction does not answer a work."));
        }
        refuse_window(opts, "a correction is not a new line in the window")?;
        refuse_turn_fields(opts)?;
        let user_id = self.uid(user_id.into())?;
        let mut obj = serde_json::Map::new();
        obj.insert("user_id".into(), serde_json::json!(user_id));
        obj.insert("old_memory_id".into(), serde_json::json!(check_id(old_memory_id, "old_memory_id")?));
        nonblank(new_content, "new_content is required.")?;
        obj.insert("new_content".into(), serde_json::json!(new_content));
        self.write_extras(&mut obj, opts)?;
        self.post_idem("/api/v1/memory/supersede", serde_json::Value::Object(obj), idempotency_key).await
    }

    /// [`Client::add`] with an idempotency key.
    pub async fn add_idempotent(&self, content: &str, user_id: impl Into<Option<&str>>, metadata: serde_json::Value, idempotency_key: &str) -> Result<serde_json::Value, WosError> {
        self.store_one(content, user_id, metadata, serde_json::json!({}), Some(idempotency_key), &WriteOpts::default())
            .await
    }

    /// [`Client::add_turn`] with an idempotency key.
    pub async fn add_turn_idempotent(&self, user_msg: &str, assistant_msg: &str, user_id: impl Into<Option<&str>>, idempotency_key: &str) -> Result<serde_json::Value, WosError> {
        self.store_turn(user_msg, assistant_msg, user_id, &WriteOpts::default(), Some(idempotency_key)).await
    }

    /// [`Client::add_bulk`] with an idempotency key.
    pub async fn add_bulk_idempotent(&self, content: &str, user_id: impl Into<Option<&str>>, category: &str, idempotency_key: &str) -> Result<serde_json::Value, WosError> {
        self.bulk(Some(content), user_id, category, serde_json::json!({}), &BulkOpts::default(), Some(idempotency_key))
            .await
    }

    /// [`Client::update`] with an idempotency key.
    pub async fn update_idempotent(&self, old_memory_id: &str, new_content: &str, user_id: impl Into<Option<&str>>, idempotency_key: &str) -> Result<serde_json::Value, WosError> {
        self.supersede(old_memory_id, new_content, user_id, &WriteOpts::default(), Some(idempotency_key)).await
    }

    /// Search a store's memories.
    pub async fn search(&self, query: &str, user_id: impl Into<Option<&str>>, limit: usize) -> Result<Vec<Memory>, WosError> {
        self.search_with(query, user_id, limit, serde_json::json!({})).await
    }

    /// [`Client::search`] with extra body fields merged in.
    pub async fn search_with(&self, query: &str, user_id: impl Into<Option<&str>>, limit: usize, extra: serde_json::Value) -> Result<Vec<Memory>, WosError> {
        let filtered = extra.get("filters").and_then(|f| f.as_object()).is_some_and(|f| !f.is_empty());
        let v = self.search_answer(query, user_id.into(), limit, extra.as_object().cloned().unwrap_or_default()).await?;
        Ok(merge_results(&v, filtered))
    }

    async fn search_answer(
        &self, query: &str, user_id: Option<&str>, limit: usize, mut obj: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value, WosError> {
        obj.insert("user_id".into(), serde_json::json!(self.uid(user_id)?));
        obj.insert("query".into(), serde_json::json!(query));
        check_search_limit(limit)?;
        obj.insert("max_results".into(), serde_json::json!(limit));
        let mut body = serde_json::Value::Object(obj);
        check_body_count(&body, "max_images", MAX_IMAGES_MAX)?;
        check_body_count(&body, "verify", VERIFY_MAX)?;
        check_filters(&body)?;
        check_body_tz(&body)?;
        self.default_tz(&mut body);
        self.post("/api/v1/memory/search", body).await
    }

    /// [`Client::search`] with typed options.
    pub async fn search_opts(&self, query: &str, user_id: impl Into<Option<&str>>, limit: usize, opts: &SearchOpts) -> Result<Vec<Memory>, WosError> {
        self.search_with(query, user_id, limit, serde_json::Value::Object(search_fields(serde_json::Map::new(), opts))).await
    }

    /// Search, and keep every field the answer came with.
    pub async fn search_full(&self, query: &str, user_id: impl Into<Option<&str>>, limit: usize, opts: &SearchOpts) -> Result<SearchFull, WosError> {
        self.search_full_with(query, user_id, limit, opts, serde_json::json!({})).await
    }

    /// [`Client::search_full`] with extra body fields merged in.
    pub async fn search_full_with(
        &self, query: &str, user_id: impl Into<Option<&str>>, limit: usize, opts: &SearchOpts, extra: serde_json::Value,
    ) -> Result<SearchFull, WosError> {
        let obj = search_fields(extra.as_object().cloned().unwrap_or_default(), opts);
        let v = self.search_answer(query, user_id.into(), limit, obj).await?;
        Ok(SearchFull {
            memories: memories_from(v.get("memories")),
            self_memories: memories_from(v.get("self_memories")),
            images: memories_from(v.get("images")),
            verify_used: v.get("verify_used").and_then(|n| n.as_u64()).map(|n| u8::try_from(n).unwrap_or(u8::MAX)),
            raw: v,
        })
    }

    /// Search, with the assistant's own words kept apart from the other memories.
    pub async fn search_self(&self, query: &str, user_id: impl Into<Option<&str>>, limit: usize) -> Result<SelfSearch, WosError> {
        self.search_self_with(query, user_id, limit, serde_json::json!({})).await
    }

    /// One-call context for an LLM: recent turns, long-term memories and the context around them.
    pub async fn recall(&self, query: &str, user_id: impl Into<Option<&str>>) -> Result<RecallResponse, WosError> {
        self.recall_with(query, user_id, serde_json::json!({})).await
    }

    /// [`Client::recall`] with typed options.
    pub async fn recall_opts(&self, query: &str, user_id: impl Into<Option<&str>>, opts: &RecallOpts) -> Result<RecallResponse, WosError> {
        let extra = self.recall_fields(None, opts)?;
        self.recall_with(query, user_id, serde_json::Value::Object(extra)).await
    }

    fn recall_fields(&self, rooms: Option<&Rooms>, opts: &RecallOpts) -> Result<serde_json::Map<String, serde_json::Value>, WosError> {
        let mut extra = recall_extra(opts)?;
        let mut named = opts.room.is_some();
        if let Some(rooms) = rooms {
            rooms.check()?;
            if *rooms == Rooms::None && named {
                return Err(refused("room names a room to look in, and Rooms::None looks in none: pass one or the other."));
            }
            if let Some(wire) = rooms.wire() {
                extra.insert("rooms".into(), wire);
                named = *rooms != Rooms::None;
            }
        }
        if named {
            if let Some(m) = self.clock(None)? {
                extra.insert("tz_offset_minutes".into(), serde_json::json!(m));
            }
        }
        Ok(extra)
    }

    /// [`Client::recall_opts`] that also looks in the rooms.
    pub async fn recall_rooms(
        &self, query: &str, user_id: impl Into<Option<&str>>, rooms: impl Into<Rooms>, opts: &RecallOpts,
    ) -> Result<RecallResponse, WosError> {
        let extra = self.recall_fields(Some(&rooms.into()), opts)?;
        self.recall_with(query, user_id, serde_json::Value::Object(extra)).await
    }

    /// Run a built-in engram.
    pub async fn engram(&self, name: &str, query: &str, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        self.engram_with(name, query, user_id, serde_json::json!({})).await
    }

    /// [`Client::recall`] with extra body fields merged in.
    pub async fn recall_with(&self, query: &str, user_id: impl Into<Option<&str>>, extra: serde_json::Value) -> Result<RecallResponse, WosError> {
        let user_id = self.uid(user_id.into())?;
        let mut obj = extra.as_object().cloned().unwrap_or_default();
        obj.insert("user_id".into(), serde_json::json!(user_id));
        obj.insert("query".into(), serde_json::json!(query));
        let mut body = serde_json::Value::Object(obj);
        check_body_count(&body, "max_images", MAX_IMAGES_MAX)?;
        check_filters(&body)?;
        check_body_tz(&body)?;
        self.default_tz(&mut body);
        let v = self.post("/api/v1/memory/recall", body).await?;
        serde_json::from_value(v).map_err(|e| WosError::Api { status: 200, message: format!("invalid recall response: {e}") })
    }

    /// [`Client::engram`] with extra body fields merged in.
    pub async fn engram_with(&self, name: &str, query: &str, user_id: impl Into<Option<&str>>, extra: serde_json::Value) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let mut obj = extra.as_object().cloned().unwrap_or_default();
        obj.insert("name".into(), serde_json::json!(check_id(name, "name")?));
        obj.insert("user_id".into(), serde_json::json!(user_id));
        obj.insert("query".into(), serde_json::json!(query));
        let mut body = serde_json::Value::Object(obj);
        check_body_tz(&body)?;
        self.default_tz(&mut body);
        self.post("/api/v1/engram/run", body).await
    }

    /// [`Client::search_self`] with extra body fields merged in.
    pub async fn search_self_with(&self, query: &str, user_id: impl Into<Option<&str>>, limit: usize, extra: serde_json::Value) -> Result<SelfSearch, WosError> {
        let v = self.search_answer(query, user_id.into(), limit, extra.as_object().cloned().unwrap_or_default()).await?;
        Ok(SelfSearch {
            memories: memories_from(v.get("memories")),
            self_memories: memories_from(v.get("self_memories")),
        })
    }

    /// What was said around a memory, counted from it.
    pub async fn around(&self, memory_id: &str, user_id: impl Into<Option<&str>>, opts: &AroundOpts) -> Result<serde_json::Value, WosError> {
        let mut obj = serde_json::Map::new();
        obj.insert("user_id".into(), serde_json::json!(self.uid(user_id.into())?));
        obj.insert("memory_id".into(), serde_json::json!(check_id(memory_id, "memory_id")?));
        for (k, v) in [("from", opts.from), ("to", opts.to), ("center", opts.center)] {
            if let Some(v) = v {
                obj.insert(k.into(), serde_json::json!(check_range(k, v, -200, 200)?));
            }
        }
        if let Some(held) = opts.held {
            check_range("held[0]", held[0], -200, 200)?;
            check_range("held[1]", held[1], -200, 200)?;
            obj.insert("held".into(), serde_json::json!(held));
        }
        let v = self.post("/api/v1/memory/around", serde_json::Value::Object(obj)).await?;
        Ok(keep_records(v, "pieces"))
    }

    /// How a memory was reached, when it answered a work of three steps or more.
    pub async fn why(&self, memory_id: &str, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let memory_id = check_id(memory_id, "memory_id")?;
        self.post("/api/v1/memory/why", serde_json::json!({"user_id": user_id, "memory_id": memory_id})).await
    }

    /// What this store handed out, newest first.
    pub async fn retrievals(&self, user_id: impl Into<Option<&str>>, opts: &RetrievalsOpts) -> Result<serde_json::Value, WosError> {
        let mut obj = serde_json::Map::new();
        obj.insert("user_id".into(), serde_json::json!(self.uid(user_id.into())?));
        if let Some(n) = opts.limit {
            obj.insert("limit".into(), serde_json::json!(check_range("limit", n, 1, 200)?));
        }
        if let Some(b) = &opts.before {
            obj.insert("before".into(), serde_json::json!(check_id(b, "before")?));
        }
        let v = self.post("/api/v1/memory/retrievals", serde_json::Value::Object(obj)).await?;
        Ok(keep_records(v, "entries"))
    }

    /// Recent conversation turns (short-term memory).
    pub async fn history(&self, user_id: impl Into<Option<&str>>) -> Result<Vec<serde_json::Value>, WosError> {
        let user_id = self.uid(user_id.into())?;
        let v = self.post("/api/v1/memory/history", serde_json::json!({"user_id": user_id})).await?;
        Ok(records(&v, "turns"))
    }

    /// The memories a line of the recent conversation had in hand when it was said.
    pub async fn seen(&self, at: &str, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let mut obj = serde_json::Map::new();
        obj.insert("user_id".into(), serde_json::json!(self.uid(user_id.into())?));
        obj.insert("at".into(), serde_json::json!(check_id(at, "at")?));
        let v = self.post("/api/v1/memory/seen", serde_json::Value::Object(obj)).await?;
        Ok(keep_records(v, "seen"))
    }

    /// Empty the recent conversation, or with `up_to` only the lines last seen at or before it.
    pub async fn clear_window(&self, user_id: impl Into<Option<&str>>, up_to: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let mut obj = serde_json::Map::new();
        obj.insert("user_id".into(), serde_json::json!(self.uid(user_id.into())?));
        if let Some(t) = up_to.into() {
            obj.insert("up_to".into(), serde_json::json!(check_id(t, "up_to")?));
        }
        self.post("/api/v1/memory/clear-window", serde_json::Value::Object(obj)).await
    }

    /// Memory counts for a store.
    pub async fn stats(&self, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/memory/stats", serde_json::json!({"user_id": user_id})).await
    }

    /// Fetch one memory by id. An unknown id is a 404 ([`ErrorKind::NotFound`]).
    pub async fn get(&self, user_id: impl Into<Option<&str>>, memory_id: &str) -> Result<serde_json::Value, WosError> {
        let memory_id = nonblank(memory_id, "memory_id is required — the id that add/store or list_memories returned.")?;
        let user_id = self.uid(user_id.into())?;
        let v = self.post("/api/v1/memory/get", serde_json::json!({"user_id": user_id, "memory_id": memory_id})).await?;
        Ok(memory_from_get(v))
    }

    /// One page of a store's memories. Pass `next_cursor` back as `cursor` for the next page.
    pub async fn list_memories(
        &self, user_id: impl Into<Option<&str>>, limit: impl Into<Option<usize>>, cursor: impl Into<Option<&str>>,
    ) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let limit = limit.into().unwrap_or(100);
        check_range("limit", limit, LIST_LIMIT_MIN, LIST_LIMIT_MAX)?;
        let mut body = serde_json::json!({ "user_id": user_id, "limit": limit });
        if let Some(c) = cursor.into().filter(|c| !c.is_empty()) {
            body["cursor"] = serde_json::json!(c);
        }
        Ok(memory_page(self.post("/api/v1/memory/list", body).await?))
    }

    /// How many of this store's memories were edited after they were written (storing new ones never edits an old one), out of all of them: 0 means every memory is as it was first stored.
    pub async fn revisions(&self, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        Ok(memory_rows(self.post("/api/v1/won/revisions", serde_json::json!({ "user_id": user_id })).await?))
    }

    /// What this key has spent, and what is left.
    pub async fn usage(&self, days: u32) -> Result<serde_json::Value, WosError> {
        check_range("days", days, 1, 365)?;
        self.request(reqwest::Method::GET, &format!("/api/v1/won/usage?days={days}"), None, None).await
    }

    /// One page of the memories behind a [`Client::revisions`] count.
    pub async fn revisions_page(
        &self, user_id: impl Into<Option<&str>>, include: &str, limit: impl Into<Option<usize>>, before: impl Into<Option<&str>>,
        skip_ids: Option<&[String]>,
    ) -> Result<serde_json::Value, WosError> {
        if include != "revised" && include != "unrevised" {
            return Err(refused(format!("include must be \"revised\" or \"unrevised\", got {include:?}.")));
        }
        let user_id = self.uid(user_id.into())?;
        let mut body = serde_json::json!({ "user_id": user_id, "include": include });
        page_fields(&mut body, limit.into(), before.into(), skip_ids)?;
        Ok(memory_rows(self.post("/api/v1/won/revisions", body).await?))
    }

    /// The edits that followed one memory, oldest first.
    pub async fn lineage(&self, user_id: impl Into<Option<&str>>, memory_id: &str) -> Result<serde_json::Value, WosError> {
        let memory_id = nonblank(memory_id, "memory_id is required — the id that add/store or list_memories returned.")?;
        let user_id = self.uid(user_id.into())?;
        let v = self.post("/api/v1/memory/lineage", serde_json::json!({ "user_id": user_id, "memory_id": memory_id })).await?;
        Ok(with_lists(v, &["chain"]))
    }

    /// What one person said, newest first.
    pub async fn by_speaker(
        &self, speaker: &str, user_id: impl Into<Option<&str>>, limit: impl Into<Option<usize>>, before: impl Into<Option<&str>>,
        skip_ids: Option<&[String]>,
    ) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let speaker = nonblank(speaker, "speaker is required — \"me\" for the assistant, or a person's name")?;
        let mut body = serde_json::json!({ "user_id": user_id, "speaker": speaker });
        page_fields(&mut body, limit.into(), before.into(), skip_ids)?;
        Ok(with_lists(self.post("/api/v1/memory/by-speaker", body).await?, &["memories"]))
    }

    /// Every memory in a store, as a list.
    pub async fn list_all_memories(&self, user_id: impl Into<Option<&str>>) -> Result<Vec<serde_json::Value>, WosError> {
        let user_id = self.uid(user_id.into())?;
        let mut out: Vec<serde_json::Value> = Vec::new();
        let mut cursor: Option<serde_json::Value> = None;
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut ended = false;
        for _ in 0..MAX_PAGES {
            let mut body = serde_json::json!({ "user_id": user_id, "limit": 100 });
            if let Some(c) = &cursor {
                body["cursor"] = c.clone();
            }
            let page = self.post("/api/v1/memory/list", body).await?;
            let rows = records(&page, "memories");
            let had_rows = !rows.is_empty();
            out.extend(rows);
            let next = page.get("next_cursor");
            match next.filter(|c| !(c.is_null() || c.as_bool() == Some(false) || c.as_str() == Some("") || c.as_f64() == Some(0.0))) {
                Some(c) if seen.insert(c.to_string()) => cursor = Some(c.clone()),
                Some(_) if had_rows => return Err(repeated_cursor()),
                _ => {
                    ended = true;
                    break;
                }
            }
        }
        if !ended {
            return Err(page_ceiling());
        }
        Ok(out)
    }

    /// All of a store's memories, as a list.
    pub async fn export_memories(&self, user_id: impl Into<Option<&str>>) -> Result<Vec<serde_json::Value>, WosError> {
        self.list_all_memories(user_id).await
    }

    /// Check that the service is reachable and the API key works.
    pub async fn ping(&self) -> Result<bool, WosError> {
        self.request(reqwest::Method::GET, "/api/v1/memory/ping", None, None).await?;
        Ok(true)
    }

    /// What this API key may do: `"full"`, `"read"` or `"admin"`.
    pub async fn key_scope(&self) -> Result<String, WosError> {
        let v = self.request(reqwest::Method::GET, "/api/v1/memory/ping", None, None).await?;
        match v.get("scope").and_then(|s| s.as_str()) {
            Some(s) if !s.is_empty() => Ok(s.to_string()),
            _ => Err(WosError::Api { status: 200, message: "the answer carried no key scope".into() }),
        }
    }

    /// Create a store. A store must exist before it is written to or searched.
    pub async fn create_store(&self, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/memory/collection", serde_json::json!({ "user_id": user_id })).await
    }

    /// List your stores.
    pub async fn list_stores(&self) -> Result<Vec<serde_json::Value>, WosError> {
        let v = self.request(reqwest::Method::GET, "/api/v1/memory/collections", None, None).await?;
        if v.get("admin_only").and_then(|a| a.as_bool()) == Some(true) {
            return Err(WosError::Api { status: 403, message: ADMIN_ONLY.into() });
        }
        Ok(records(&v, "collections"))
    }

    /// Delete a store and all its memories.
    pub async fn delete_store(&self, user_id: &str) -> Result<serde_json::Value, WosError> {
        nonblank(user_id, "user_id is required (non-blank) — delete_store never falls back to the default store.")?;
        warn_if_store_id_collapses(user_id);
        let body = serde_json::json!({ "user_id": user_id });
        self.request(reqwest::Method::DELETE, "/api/v1/memory/collection", Some(&body), None).await
    }

    /// Delete one memory by id.
    pub async fn delete(&self, user_id: impl Into<Option<&str>>, memory_id: &str) -> Result<serde_json::Value, WosError> {
        let memory_id = nonblank(memory_id, "memory_id is required (non-blank). To delete every memory in a store, call delete_all(user_id) explicitly.")?;
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/memory/forget", serde_json::json!({"user_id": user_id, "memory_id": memory_id})).await
    }

    /// Delete all memories in a store; it also clears the store's time zone and speakers.
    /// On Scroll 2, set them again with [`Client::set_settings`] and [`Client::add_speaker`] before the next write.
    pub async fn delete_all(&self, user_id: &str) -> Result<serde_json::Value, WosError> {
        nonblank(user_id, "user_id is required (non-blank) for delete_all — a blank/whitespace id would wipe the default store.")?;
        warn_if_store_id_collapses(user_id);
        self.post("/api/v1/memory/forget", serde_json::json!({"user_id": user_id})).await
    }
}

pub(crate) fn with_lists(v: serde_json::Value, keys: &[&str]) -> serde_json::Value {
    let v = if v.is_object() { v } else { serde_json::json!({}) };
    keys.iter().fold(v, |v, k| keep_records(v, k))
}

pub(crate) fn memory_page(v: serde_json::Value) -> serde_json::Value {
    let mut v = with_lists(v, &["memories"]);
    if let Some(o) = v.as_object_mut() {
        let n = o.get("memories").and_then(|m| m.as_array()).map_or(0, Vec::len);
        if !o.get("count").map_or(false, serde_json::Value::is_number) {
            o.insert("count".into(), serde_json::json!(n));
        }
        if o.get("next_cursor").map_or(true, |c| c.is_null() || c.as_str() == Some("")) {
            o.insert("next_cursor".into(), serde_json::Value::Null);
        }
    }
    v
}

pub(crate) fn memory_rows(v: serde_json::Value) -> serde_json::Value {
    if v.get("memories").is_some() { keep_records(v, "memories") } else { v }
}

pub(crate) fn merge_results(v: &serde_json::Value, filtered: bool) -> Vec<Memory> {
    let mut out = memories_from(v.get("memories"));
    let mut seen: std::collections::HashSet<String> = out.iter().filter_map(|m| m.id.clone()).collect();
    let own = if filtered { Vec::new() } else { memories_from(v.get("self_memories")) };
    for m in own.into_iter().chain(memories_from(v.get("images"))) {
        if m.id.as_ref().is_none_or(|id| seen.insert(id.clone())) {
            out.push(m);
        }
    }
    out
}

pub(crate) fn memories_from(v: Option<&serde_json::Value>) -> Vec<Memory> {
    let rows = v.and_then(|x| x.as_array()).into_iter().flatten();
    rows.filter_map(|el| serde_json::from_value::<Memory>(el.clone()).ok()).collect()
}

pub(crate) fn records(v: &serde_json::Value, key: &str) -> Vec<serde_json::Value> {
    v.get(key).and_then(|a| a.as_array()).map(|a| a.iter().filter(|x| x.is_object()).cloned().collect()).unwrap_or_default()
}

pub(crate) async fn store_assistant(client: &Client, store: &str) -> Result<Option<String>, WosError> {
    let settings = client.settings(store).await?;
    let name = settings.get("default_speakers").and_then(|d| d.get("assistant")).and_then(|a| a.as_str());
    Ok(name.map(js_trim).filter(|a| !a.is_empty()).map(str::to_string))
}

pub(crate) fn page_ceiling() -> WosError {
    truncated_walk(&format!("stopped after {MAX_PAGES} pages — the store did not end"))
}

pub(crate) fn repeated_cursor() -> WosError {
    truncated_walk("the service handed back a cursor it had already given, so the store did not end")
}

pub(crate) fn memory_from_get(v: serde_json::Value) -> serde_json::Value {
    if let Some(m) = v.get("memory").filter(|m| m.is_object()) {
        return m.clone();
    }
    if v.get("id").is_some_and(|id| id.is_string()) { v } else { serde_json::json!({}) }
}

#[cfg(test)] mod add_bulk_tests;
#[cfg(test)] mod page_size_tests;
#[cfg(test)] mod list_shape_tests;
#[cfg(test)] mod delete_retry_tests;
#[cfg(test)] mod metadata_warning_tests;
#[cfg(test)] mod search_full_with_tests;
#[cfg(test)] mod admin_key_tests;
#[cfg(test)] mod settings_speakers_tests;
