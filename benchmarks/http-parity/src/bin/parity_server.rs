use std::str::FromStr;

use http_parity::{build_axum_app, build_tork_app, Backend, Scenario};
use tokio::net::TcpListener;

fn parse_args() -> Result<(Backend, Scenario, String), String> {
    let mut args = std::env::args().skip(1);
    let backend = Backend::from_str(&args.next().ok_or("missing backend")?)?;
    let scenario = Scenario::from_str(&args.next().ok_or("missing scenario")?)?;
    let addr = args.next().unwrap_or_else(|| "127.0.0.1:3000".to_owned());
    Ok((backend, scenario, addr))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (backend, scenario, addr) = parse_args().map_err(std::io::Error::other)?;
    match backend {
        Backend::Tork => build_tork_app(scenario).serve(addr).await?,
        Backend::Axum => {
            let listener = TcpListener::bind(&addr).await?;
            let local_addr = listener.local_addr()?;
            eprintln!("axum server listening on http://{local_addr}");
            axum::serve(listener, build_axum_app(scenario))
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await?;
        }
    }
    Ok(())
}
