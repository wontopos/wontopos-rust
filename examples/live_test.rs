// Live test against the hosted service. The key comes from the environment, the
// way `Client::from_env` reads it:
//
//   WONTOPOS_API_KEY=... cargo run --release --example live_test
//
// WONTOPOS_BASE_URL, when set, points the run at another server. It works in a
// store of its own, created for this run, and deletes that store on every path. A
// store that already existed is never touched.
use wontopos::{Client, WosError};

fn base_url() -> Option<String> {
    let base = std::env::var("WONTOPOS_BASE_URL").unwrap_or_default();
    (!base.trim().is_empty()).then(|| base.trim().to_string())
}

fn client_with_key(key: &str) -> Client {
    match base_url() {
        Some(base) => Client::with_base_url(key, &base),
        None => Client::new(key),
    }
}

fn client_from_env() -> Result<Client, WosError> {
    let key = ["WONTOPOS_API_KEY", "WOS_API_KEY"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|k| !k.trim().is_empty()));
    match (key, base_url()) {
        (Some(key), Some(_)) => Ok(client_with_key(&key)),
        _ => Client::from_env(),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = client_from_env()?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let store = format!("sdk_rusttest_{}_{nanos}", std::process::id());

    println!("== create_store {store} (stores are explicit) ==");
    let created = client.create_store(store.as_str()).await?;
    println!("  {created}");
    if created["status"] != "created" {
        return Err(format!("store {store} already existed; this run does not touch it").into());
    }

    let mem = client.with_user(&store);
    let outcome = exercise(&mem, &store).await;

    println!("\n== delete_store (cleanup) ==");
    let cleanup = mem.delete_store(&store).await;
    match &cleanup {
        Ok(v) => println!("  {v}"),
        Err(e) => println!("  cleanup failed, delete {store} by hand: {e}"),
    }
    outcome?;
    cleanup?;
    Ok(())
}

async fn exercise(mem: &Client, store: &str) -> Result<(), WosError> {
    println!("== add (EN) ==");
    println!("  {}", mem.add("she prefers tea over coffee", None, serde_json::json!({})).await?);
    println!("== add (ES) ==");
    println!("  {}", mem.add("vive en el barrio de Chueca, en Madrid", None, serde_json::json!({})).await?);

    println!("== add_turn (payload first, user last) ==");
    println!("  {}", mem.add_turn("hi", "hello!", None).await?);

    println!("== speakers: register, tag, filter ==");
    println!("  {}", mem.add_speaker("Bob", None).await?);
    println!("  {}", mem.add("I promised the summary by Friday", None, serde_json::json!({"speaker": "me"})).await?);
    println!("  {}", mem.add("Bob said the deadline moved to Tuesday", None, serde_json::json!({"speaker": "Bob"})).await?);
    let bob = mem.search_with("what did Bob say?", None, 5, serde_json::json!({"speaker": "Bob"})).await?;
    println!("  bob hits: {}", bob.len());

    println!("== search (EN) ==");
    let r_en = mem.search("what does she drink?", None, 5).await?;
    println!("  top: {}", r_en.first().map(|m| m.content.clone()).unwrap_or("(none)".into()));

    println!("== search (ES) ==");
    let r_es = mem.search("donde vive?", None, 5).await?;
    println!("  top: {}", r_es.first().map(|m| m.content.clone()).unwrap_or("(none)".into()));

    println!("== recall ==");
    let rc = mem.recall("preferences", None).await?;
    println!(
        "  short_term keys: {:?}",
        rc.short_term.as_object().map(|o| o.keys().collect::<Vec<_>>())
    );

    println!("== history ==");
    println!("  turns: {}", mem.history(None).await?.len());

    println!("== stats ==");
    println!("  {}", mem.stats(None).await?);

    println!("\n== error path ==");
    let bad = client_with_key("wos-live-INVALID");
    match bad.search("x", store, 5).await {
        Err(e) => println!("  {:?}: {e}", e.kind()),
        Ok(v) => println!("  unexpected: {} results", v.len()),
    }
    Ok(())
}
