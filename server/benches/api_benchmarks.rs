//! API 性能基准测试

use std::sync::Arc;

use axum::{routing::get, Router};
use criterion::{criterion_group, criterion_main, Criterion};
use serde_json::json;
use tokio::runtime::Runtime;

use tianyan::config::TianyanConfig;

fn build_test_router() -> Router {
    let config = Arc::new(tokio::sync::RwLock::new(TianyanConfig::default()));

    Router::new()
        .route(
            "/health",
            get(|| async { axum::Json(json!({"status": "ok"})) }),
        )
        .route(
            "/api/v1/config",
            get({
                let s = config.clone();
                move || {
                    let s = s.clone();
                    async move {
                        let c = s.read().await;
                        axum::Json(json!({"config": *c}))
                    }
                }
            }),
        )
}

fn bench_health_endpoint(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let router = build_test_router();
    let listener =
        rt.block_on(async { tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap() });
    let addr = listener.local_addr().unwrap();

    rt.spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    std::thread::sleep(std::time::Duration::from_millis(100));

    let url = format!("http://{}/health", addr);
    let client = reqwest::blocking::Client::new();

    c.bench_function("health_endpoint", |b| {
        b.iter(|| {
            let resp = client.get(&url).send().unwrap();
            assert_eq!(resp.status(), 200);
        })
    });
}

fn bench_config_endpoint(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let router = build_test_router();
    let listener =
        rt.block_on(async { tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap() });
    let addr = listener.local_addr().unwrap();

    rt.spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    std::thread::sleep(std::time::Duration::from_millis(100));

    let url = format!("http://{}/api/v1/config", addr);
    let client = reqwest::blocking::Client::new();

    c.bench_function("config_endpoint", |b| {
        b.iter(|| {
            let resp = client.get(&url).send().unwrap();
            assert_eq!(resp.status(), 200);
        })
    });
}

fn bench_config_deserialize(c: &mut Criterion) {
    let config = TianyanConfig::default();
    let json_str = serde_json::to_string(&json!({"config": config})).unwrap();

    c.bench_function("config_deserialize", |b| {
        b.iter(|| {
            let _value: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        })
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(50);
    targets = bench_health_endpoint, bench_config_endpoint, bench_config_deserialize
}
criterion_main!(benches);
