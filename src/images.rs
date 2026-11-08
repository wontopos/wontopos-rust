use super::*;

impl Client {
    /// Fetch the bytes of an image memory.
    pub async fn get_image(&self, user_id: impl Into<Option<&str>>, memory_id: &str) -> Result<(Vec<u8>, String), WosError> {
        let memory_id = nonblank(memory_id, "memory_id is required — the id that add/store or list_images returned.")?;
        let user_id = self.uid(user_id.into())?;
        let body = serde_json::json!({ "user_id": user_id, "memory_id": memory_id });
        self.request_bytes("/api/v1/memory/image", &body).await
    }

    /// Remove the photo from a memory; it can take the whole memory with it.
    pub async fn forget_image(&self, user_id: impl Into<Option<&str>>, memory_id: &str, preview: bool) -> Result<serde_json::Value, WosError> {
        let memory_id = nonblank(memory_id, "memory_id is required — the id of the memory whose image you want removed.")?;
        let user_id = self.uid(user_id.into())?;
        let mut body = serde_json::json!({ "user_id": user_id, "memory_id": memory_id });
        if preview {
            body["preview"] = serde_json::json!(true);
        }
        self.send(reqwest::Method::DELETE, "/api/v1/memory/image", Some(&body), None, None, !preview, false).await
    }

    /// One page of image memories, newest first, with the store's total count.
    pub async fn list_images(
        &self, user_id: impl Into<Option<&str>>, limit: impl Into<Option<usize>>, before: impl Into<Option<&str>>, skip_ids: Option<&[String]>,
    ) -> Result<serde_json::Value, WosError> {
        let user_id = self.uid(user_id.into())?;
        let mut body = serde_json::json!({ "user_id": user_id });
        page_fields(&mut body, limit.into(), before.into(), skip_ids)?;
        Ok(with_lists(self.post("/api/v1/memory/images", body).await?, &["images"]))
    }

    /// Every image memory in a store, as a list.
    pub async fn list_all_images(&self, user_id: impl Into<Option<&str>>, page_size: impl Into<Option<usize>>) -> Result<Vec<serde_json::Value>, WosError> {
        let user_id = self.uid(user_id.into())?;
        let page_size = page_size.into();
        if let Some(n) = page_size {
            check_range("page_size", n, PAGE_LIMIT_MIN, PAGE_LIMIT_MAX)?;
        }
        let mut out: Vec<serde_json::Value> = Vec::new();
        let mut before: Option<String> = None;
        let mut skip_ids: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut ended = false;
        for _ in 0..MAX_PAGES {
            let mut body = serde_json::json!({ "user_id": user_id });
            if let Some(l) = page_size {
                body["limit"] = serde_json::json!(l);
            }
            if let Some(b) = &before {
                body["before"] = serde_json::json!(b);
            }
            if !skip_ids.is_empty() {
                body["skip_ids"] = serde_json::json!(skip_ids);
            }
            let page = self.post("/api/v1/memory/images", body).await?;
            let rows = records(&page, "images");
            let had_rows = !rows.is_empty();
            out.extend(rows);
            if !page.get("has_more").and_then(|v| v.as_bool()).unwrap_or(false) {
                ended = true;
                break;
            }
            let Some(next_before) = page.get("next_before").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else {
                ended = true;
                break;
            };
            let next_skip = page.get("next_skip_ids").and_then(|v| v.as_array()).into_iter().flatten();
            let next_skip: Vec<String> = next_skip.filter_map(|x| x.as_str().map(String::from)).collect();
            let key = format!("{next_before}|{}", next_skip.join(","));
            if !seen.insert(key) {
                if had_rows {
                    return Err(repeated_cursor());
                }
                ended = true;
                break;
            }
            before = Some(next_before.to_string());
            skip_ids = next_skip;
        }
        if !ended {
            return Err(page_ceiling());
        }
        Ok(out)
    }

    /// All of a store's image memories, as a list.
    pub async fn export_images(&self, user_id: impl Into<Option<&str>>, page_size: impl Into<Option<usize>>) -> Result<Vec<serde_json::Value>, WosError> {
        self.list_all_images(user_id, page_size).await
    }

    /// The same as [`Client::export_images`].
    pub async fn iter_images(&self, user_id: impl Into<Option<&str>>, page_size: impl Into<Option<usize>>) -> Result<Vec<serde_json::Value>, WosError> {
        self.export_images(user_id, page_size).await
    }
}
