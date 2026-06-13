//! End-to-end test of `#[tork::lifespan]`, resources, and the serve lifecycle.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tork::{get, App, LifespanContext, Resources, Router};

static STOPPED: AtomicBool = AtomicBool::new(false);

/// A resource produced by the lifespan.
#[derive(Clone)]
struct Marker(i64);

#[derive(Clone, Resources)]
struct BootState {
    #[resource]
    marker: Marker,
}

#[tork::lifespan]
impl BootState {
    async fn startup(_ctx: LifespanContext) -> tork::Result<Self> {
        Ok(BootState {
            marker: Marker(123),
        })
    }

    async fn shutdown(self) -> tork::Result<()> {
        STOPPED.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[get("/marker")]
async fn read_marker(marker: Marker) -> tork::Result<i64> {
    Ok(marker.0)
}

#[tokio::test]
async fn lifespan_registers_resources_and_shuts_down() {
    let (addr_tx, addr_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(addr_tx)));

    let app = App::new()
        .lifespan::<BootState>()
        .on_ready(move |ctx| {
            let sender = sender.clone();
            async move {
                if let Some(tx) = sender.lock().unwrap().take() {
                    let _ = tx.send(ctx.addr());
                }
                Ok(())
            }
        })
        .include_router(Router::new().route(__tork_route_read_marker()));

    let server = tokio::spawn(app.serve_with_shutdown("127.0.0.1:0", async move {
        let _ = shutdown_rx.await;
    }));

    let addr = addr_rx.await.unwrap();
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /marker HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();

    // The lifespan registered the Marker resource; the handler injected it.
    assert!(response.contains("123"), "unexpected response: {response}");

    let _ = shutdown_tx.send(());
    let _ = server.await;

    assert!(STOPPED.load(Ordering::SeqCst), "shutdown should have run");
}

// --- Multiple lifespans: startup in order, shutdown in reverse ---

static ORDER: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

#[derive(Clone, Resources)]
struct First;

#[derive(Clone, Resources)]
struct Second;

#[tork::lifespan]
impl First {
    async fn startup(_ctx: LifespanContext) -> tork::Result<Self> {
        ORDER.lock().unwrap().push("first");
        Ok(First)
    }
    async fn shutdown(self) -> tork::Result<()> {
        ORDER.lock().unwrap().push("first-stop");
        Ok(())
    }
}

#[tork::lifespan]
impl Second {
    async fn startup(_ctx: LifespanContext) -> tork::Result<Self> {
        ORDER.lock().unwrap().push("second");
        Ok(Second)
    }
    async fn shutdown(self) -> tork::Result<()> {
        ORDER.lock().unwrap().push("second-stop");
        Ok(())
    }
}

#[tokio::test]
async fn multiple_lifespans_start_in_order_and_stop_in_reverse() {
    ORDER.lock().unwrap().clear();

    App::new()
        .lifespan::<First>()
        .lifespan::<Second>()
        .serve_with_shutdown("127.0.0.1:0", async {})
        .await
        .unwrap();

    assert_eq!(
        *ORDER.lock().unwrap(),
        ["first", "second", "second-stop", "first-stop"]
    );
}

#[test]
fn lifespan_with_event_hooks_is_rejected() {
    let error = App::new()
        .lifespan::<First>()
        .on_shutdown(|| async { Ok(()) })
        .build()
        .err()
        .expect("lifespan plus on_shutdown should conflict");
    assert_eq!(error.code(), "LIFECYCLE_CONFLICT");
}
