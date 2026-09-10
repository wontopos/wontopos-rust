// Live test: run with `WOS_KEY=... cargo run --release --example live_test`
use wontopos::{Client, WosError};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let key = std::env::var("WOS_KEY").expect("set WOS_KEY env var");
    let mem = Client::new(&key).with_user("sdk-rusttest");

    println!("== create_store (stores are explicit) ==");
    println!("  {}", mem.create_store(None).await?);

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
    let r_en = mem.search("what does she drink?", None, 3).await?;
    println!("  top: {}", r_en.first().map(|m| m.content.clone()).unwrap_or("(none)".into()));

    println!("== search (ES) ==");
    let r_es = mem.search("donde vive?", None, 3).await?;
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
    let bad = Client::new("wos-live-INVALID");
    match bad.search("x", "sdk-rusttest", 3).await {
        Err(WosError::Api { status, message }) => println!("  WosError({status}): {message}"),
        other => println!("  unexpected: {:?}", other),
    }

    println!("\n== delete_store (cleanup) ==");
    println!("  {}", mem.delete_store("sdk-rusttest").await?);

    Ok(())
}
