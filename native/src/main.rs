use observatory_usage_core::api::UsageBackend;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::sync::Arc;
#[tokio::main]
async fn main() {
    let backend = Arc::new(match UsageBackend::open() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    });
    backend.start_background();
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            match line {
                Ok(v) => {
                    if tx.blocking_send(v).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let mut tasks = tokio::task::JoinSet::new();
    while let Some(line) = rx.recv().await {
        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                println!("{}", json!({"id":null,"error":"Invalid request"}));
                continue;
            }
        };
        let backend = backend.clone();
        tasks.spawn(async move {
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            let result = backend
                .call(
                    request.get("op").and_then(Value::as_str).unwrap_or(""),
                    request.get("args").cloned().unwrap_or(json!({})),
                )
                .await;
            let response = match result {
                Ok(value) => json!({"id":id,"result":value}),
                Err(error) => json!({"id":id,"error":error.to_string()}),
            };
            let mut output = std::io::stdout().lock();
            let _ = writeln!(output, "{response}");
            let _ = output.flush();
        });
        while tasks.try_join_next().is_some() {}
    }
    while tasks.join_next().await.is_some() {}
}
