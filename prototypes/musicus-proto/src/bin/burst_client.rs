//! Load generator: N musicians press "next song" at the same instant.
//!
//! Usage: burst_client <base-url> <clients> <get|save|revalidate> [annotation-bytes]

use std::{sync::Arc, time::{Duration, Instant}};
use tokio::sync::Barrier;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let base = args[1].clone();
    let clients: usize = args[2].parse().unwrap();
    let op = args[3].clone();
    let payload: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(8_000);

    let barrier = Arc::new(Barrier::new(clients));
    let mut tasks = Vec::new();
    for u in 0..clients {
        let (base, op, barrier) = (base.clone(), op.clone(), barrier.clone());
        tasks.push(tokio::spawn(async move {
            // One client per musician: own TCP connection, like separate tablets.
            let http = reqwest::Client::builder().pool_max_idle_per_host(1).build().unwrap();
            let auth = format!("Bearer tok{u}");
            let url = format!("{base}/api/parts/0/file");
            // Warm the connection so that we measure the request, not the TCP handshake.
            let etag = http.get(&url).header("Authorization", &auth).send().await.unwrap()
                .headers().get("etag").map(|v| v.to_str().unwrap().to_owned());
            barrier.wait().await;
            let start = Instant::now();
            let bytes = match op.as_str() {
                "get" => {
                    let r = http.get(&url).header("Authorization", &auth).send().await.unwrap();
                    assert!(r.status().is_success(), "{}", r.status());
                    r.bytes().await.unwrap().len()
                }
                "revalidate" => {
                    let r = http.get(&url).header("Authorization", &auth)
                        .header("If-None-Match", etag.unwrap()).send().await.unwrap();
                    assert_eq!(r.status(), 304);
                    0
                }
                "save" => {
                    let body = vec![(u % 251) as u8; payload];
                    let r = http
                        .post(format!("{base}/api/parts/0/annotations/default/pages/{}", u % 4))
                        .header("Authorization", &auth)
                        .body(body)
                        .send()
                        .await
                        .unwrap();
                    assert!(r.status().is_success(), "{}", r.status());
                    payload
                }
                _ => panic!("unknown op"),
            };
            (start.elapsed(), bytes, start)
        }));
    }
    let mut results = Vec::new();
    for t in tasks {
        results.push(t.await.unwrap());
    }
    let first_start = results.iter().map(|r| r.2).min().unwrap();
    let wall = results.iter().map(|r| r.2 + r.0).max().unwrap() - first_start;
    let mut lat: Vec<Duration> = results.iter().map(|r| r.0).collect();
    lat.sort();
    let total: usize = results.iter().map(|r| r.1).sum();
    let pct = |p: f64| lat[((lat.len() as f64 * p).ceil() as usize).saturating_sub(1)];
    println!(
        "{op:>10} clients={clients:>3}  p50={:>9.2?}  p95={:>9.2?}  max={:>9.2?}  wall={:>9.2?}  throughput={:.0} MB/s",
        pct(0.50), pct(0.95), lat[lat.len() - 1], wall,
        total as f64 / 1e6 / wall.as_secs_f64()
    );
    if let Ok(r) = reqwest::get(format!("{base}/stats")).await {
        println!("{}", r.text().await.unwrap().replace('\n', "  "));
    }
}
