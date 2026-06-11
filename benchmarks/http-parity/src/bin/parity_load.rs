use std::str::FromStr;
use std::time::Duration;

use http_parity::{markdown_report, run_load, Backend, LoadConfig, Scenario};

fn parse_flag(args: &[String], name: &str, default: u64) -> Result<u64, String> {
    if let Some(index) = args.iter().position(|arg| arg == name) {
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for `{name}`"))?;
        value
            .parse::<u64>()
            .map_err(|error| format!("invalid value for `{name}`: {error}"))
    } else {
        Ok(default)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        return Err(std::io::Error::other(
            "usage: parity_load <backend> <scenario> [--concurrency N] [--duration S] [--warmup S]",
        )
        .into());
    }

    let backend = Backend::from_str(&args[0]).map_err(std::io::Error::other)?;
    let scenario = Scenario::from_str(&args[1]).map_err(std::io::Error::other)?;
    let concurrency = parse_flag(&args, "--concurrency", 16)? as usize;
    let duration = parse_flag(&args, "--duration", 20)?;
    let warmup = parse_flag(&args, "--warmup", 5)?;

    let report = run_load(
        backend,
        scenario,
        LoadConfig {
            warmup: Duration::from_secs(warmup),
            measure: Duration::from_secs(duration),
            concurrency,
        },
    )
    .await?;

    println!("{}", markdown_report(&report));
    Ok(())
}
