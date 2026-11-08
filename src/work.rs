use super::*;

impl Client {
    /// Open a working memory for one question.
    pub async fn open_work(&self, question: &str, user_id: impl Into<Option<&str>>, opts: &LookOpts) -> Result<serde_json::Value, WosError> {
        let mut obj = serde_json::Map::new();
        obj.insert("user_id".into(), serde_json::json!(self.uid(user_id.into())?));
        obj.insert("question".into(), serde_json::json!(check_id(question, "question")?));
        self.look(&mut obj, opts.rooms.as_ref(), opts.tz)?;
        if let Some(n) = opts.max_images {
            obj.insert("max_images".into(), serde_json::json!(check_range("max_images", n, 0, MAX_IMAGES_MAX)?));
        }
        if opts.own_words == Some(false) {
            obj.insert("filters".into(), serde_json::json!({"source_types": NOT_ASSISTANT_SOURCES}));
        }
        if let Some(d) = &opts.diversity {
            obj.insert("diversity".into(), serde_json::json!(check_diversity(d)?));
        }
        self.work("open", obj).await
    }

    /// A step: search again with a question of your own.
    pub async fn work_search(
        &self, work_id: &str, query: &str, user_id: impl Into<Option<&str>>, opts: &LookOpts,
    ) -> Result<serde_json::Value, WosError> {
        for (set, name) in [(opts.max_images.is_some(), "max_images"), (opts.own_words.is_some(), "own_words"), (opts.diversity.is_some(), "diversity")] {
            if set {
                return Err(refused(format!("{name} goes on open_work: the work keeps it for every look after.")));
            }
        }
        let mut obj = self.work_body(work_id, user_id.into())?;
        obj.insert("query".into(), serde_json::json!(check_id(query, "query")?));
        self.look(&mut obj, opts.rooms.as_ref(), opts.tz)?;
        self.work("search", obj).await
    }

    /// The next 20 memories for the work's question, at most three times.
    pub async fn work_deeper(&self, work_id: &str, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let obj = self.work_body(work_id, user_id.into())?;
        self.work("deeper", obj).await
    }

    /// A step: what was said around memory `[n]` that this work does not have yet.
    pub async fn work_around(
        &self, work_id: &str, n: u32, user_id: impl Into<Option<&str>>, opts: &WorkAroundOpts,
    ) -> Result<serde_json::Value, WosError> {
        let mut obj = self.work_body(work_id, user_id.into())?;
        obj.insert("n".into(), serde_json::json!(check_range("n", n, 1, 200)?));
        for (k, v) in [("from", opts.from), ("to", opts.to), ("center", opts.center)] {
            if let Some(v) = v {
                obj.insert(k.into(), serde_json::json!(check_range(k, v, -200, 200)?));
            }
        }
        let v = self.work("around", obj).await?;
        Ok(keep_records(v, "pieces"))
    }

    /// Mark the work as waiting on the person. Pass their reply to [`Client::work_confirm`].
    pub async fn work_ask(&self, work_id: &str, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let obj = self.work_body(work_id, user_id.into())?;
        self.work("ask", obj).await
    }

    /// Answer a work waiting on the person with their yes or no.
    pub async fn work_confirm(&self, work_id: &str, confirmed: bool, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let mut obj = self.work_body(work_id, user_id.into())?;
        obj.insert("confirmed".into(), serde_json::json!(confirmed));
        self.work("confirm", obj).await
    }

    /// Close a work, with the memory that answered if one did.
    pub async fn close_work(
        &self, work_id: &str, user_id: impl Into<Option<&str>>, answer_id: impl Into<Option<&str>>,
    ) -> Result<serde_json::Value, WosError> {
        let mut obj = self.work_body(work_id, user_id.into())?;
        if let Some(a) = answer_id.into() {
            obj.insert("answer_id".into(), serde_json::json!(check_id(a, "answer_id")?));
        }
        self.work("close", obj).await
    }

    /// The open works on this store, and the traces of recent ones.
    pub async fn work_state(&self, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/memory/work/state", serde_json::json!({"user_id": user_id})).await
    }

    fn work_body(&self, work_id: &str, user_id: Option<&str>) -> Result<serde_json::Map<String, serde_json::Value>, WosError> {
        let mut obj = serde_json::Map::new();
        obj.insert("user_id".into(), serde_json::json!(self.uid(user_id)?));
        obj.insert("work_id".into(), serde_json::json!(check_id(work_id, "work_id")?));
        Ok(obj)
    }

    async fn work(&self, op: &str, obj: serde_json::Map<String, serde_json::Value>) -> Result<serde_json::Value, WosError> {
        Ok(memory_rows(self.post(&format!("/api/v1/memory/work/{op}"), serde_json::Value::Object(obj)).await?))
    }
}
