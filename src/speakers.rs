use super::*;

impl Client {
    /// Register a person for this store, before storing what they said.
    pub async fn add_speaker(&self, speaker: &str, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let speaker = written_speaker(speaker, "speaker")?;
        let user_id = self.uid(user_id.into())?;
        self.post("/api/v1/memory/speakers", serde_json::json!({"user_id": user_id, "speaker": speaker})).await
    }

    /// The store's registered people, each with its memory count.
    pub async fn list_speakers(&self, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let query = [("user_id", user_id)];
        let v = self.request(reqwest::Method::GET, "/api/v1/memory/speakers", None, Some(&query)).await?;
        Ok(with_lists(v, &["speakers"]))
    }

    /// Unregister a person; their memories stay.
    pub async fn remove_speaker(&self, speaker: &str, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let body = serde_json::json!({"user_id": user_id, "speaker": check_id(speaker, "speaker")?});
        self.request(reqwest::Method::DELETE, "/api/v1/memory/speakers", Some(&body), None).await
    }

    /// Move memories from one speaker to another.
    pub async fn relabel_speaker(
        &self, from_speaker: &str, to_speaker: &str, user_id: impl Into<Option<&str>>, memory_ids: Option<&[String]>,
    ) -> Result<serde_json::Value, WosError> {
        let mut obj = serde_json::Map::new();
        obj.insert("user_id".into(), serde_json::json!(self.uid(user_id.into())?));
        obj.insert("from".into(), serde_json::json!(check_id(from_speaker, "from_speaker")?));
        obj.insert("to".into(), serde_json::json!(written_speaker(to_speaker, "to_speaker")?));
        if let Some(ids) = memory_ids {
            if !(1..=1000).contains(&ids.len()) {
                return Err(refused(format!("memory_ids must hold between 1 and 1000 ids, got {}.", ids.len())));
            }
            let ids = ids.iter().enumerate().map(|(i, m)| check_id(m, &format!("memory_ids[{i}]"))).collect::<Result<Vec<_>, _>>()?;
            obj.insert("memory_ids".into(), serde_json::json!(ids));
        }
        self.post_until_complete("/api/v1/memory/speakers/relabel", serde_json::Value::Object(obj)).await
    }

    /// Delete everything a person said in this store (from Tablet 2 on).
    pub async fn forget_speaker(
        &self, speakers: impl IntoIterator<Item = impl AsRef<str>>, user_id: &str, preview: bool,
    ) -> Result<serde_json::Value, WosError> {
        nonblank(user_id, "user_id is required (non-blank) for forget_speaker — it never falls back to the default store.")?;
        warn_if_store_id_collapses(user_id);
        let mut names = Vec::new();
        for (i, s) in speakers.into_iter().enumerate() {
            names.push(check_id(s.as_ref(), &format!("speakers[{i}]"))?);
        }
        if names.is_empty() {
            return Err(refused("forget_speaker needs at least one speaker name."));
        }
        let mut body = serde_json::json!({"user_id": user_id, "speakers": names});
        if preview {
            body["preview"] = serde_json::json!(true);
            return self.post("/api/v1/memory/forget", body).await;
        }
        self.post_until_complete("/api/v1/memory/forget", body).await
    }

    /// The store's settings: its time zone, its speakers and every change to them.
    pub async fn settings(&self, user_id: impl Into<Option<&str>>) -> Result<serde_json::Value, WosError> {
        let query = [("user_id", self.uid(user_id.into())?)];
        self.request(reqwest::Method::GET, "/api/v1/memory/settings", None, Some(&query)).await
    }

    /// Set the store's time zone, its default speakers, or both.
    pub async fn set_settings(&self, user_id: impl Into<Option<&str>>, opts: &SettingsOpts) -> Result<serde_json::Value, WosError> {
        let mut obj = serde_json::Map::new();
        obj.insert("user_id".into(), serde_json::json!(self.uid(user_id.into())?));
        if let Some(tz) = &opts.time_zone {
            obj.insert("time_zone".into(), serde_json::json!(check_id(tz, "time_zone")?));
        }
        if let Some(d) = &opts.default_speakers {
            let mut sides = serde_json::Map::new();
            for (side, v) in [("user", &d.user), ("assistant", &d.assistant)] {
                let value = match v {
                    None => continue,
                    Some(None) => serde_json::Value::Null,
                    Some(Some(name)) if js_trim(name).is_empty() => {
                        return Err(refused(format!("default_speakers.{side} must be a name, or Some(None) to clear it.")));
                    }
                    Some(Some(name)) => serde_json::json!(written_speaker(name, &format!("default_speakers.{side}"))?),
                };
                sides.insert(side.into(), value);
            }
            if sides.is_empty() {
                return Err(refused("default_speakers must name user, assistant or both."));
            }
            obj.insert("default_speakers".into(), serde_json::Value::Object(sides));
        }
        if !obj.contains_key("time_zone") && !obj.contains_key("default_speakers") {
            return Err(refused("set_settings needs time_zone or default_speakers: there is nothing to change."));
        }
        if let Some(c) = opts.confirmed_by_user {
            obj.insert("confirmed_by_user".into(), serde_json::json!(c));
        }
        self.post("/api/v1/memory/settings", serde_json::Value::Object(obj)).await
    }
}
