mod console;
mod image_test;
mod json_test;
mod unpack;
mod util;

use crate::json_test::event_id_at;
use crate::unpack::{exe_dir, unpack, Unpacked};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;

struct Args {
    n: u64,
    unpack_only: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse(&raw) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("error: {err}");
            eprintln!("usage: expyandex-rs [-n N] [unpack]");
            return ExitCode::FAILURE;
        }
    };

    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<(), String> {
    let unpacked = unpack(&exe_dir(), args.unpack_only)?;
    let (log, worker) = console::start();
    let (stop_tx, stop_rx) = watch::channel(false);
    {
        let log = log.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                let _ = stop_tx.send(true);
                log.line_wait("stopping").await;
            }
        });
    }

    for line in notes(&unpacked) {
        log.line_wait(&line).await;
    }
    if args.unpack_only {
        log.shutdown().await;
        let _ = worker.await;
        return Ok(());
    }

    let n_line = if args.n == 0 {
        "n=0 until ctrl+c".to_string()
    } else {
        format!("n={}", args.n)
    };
    log.line_wait(&n_line).await;
    log.line_wait(&format!("concurrency {} each", util::concurrency())).await;

    let prepared = load_samples(&unpacked);
    let (client, json_bytes, image_bytes, event_at) = match prepared {
        Ok(ready) => ready,
        Err(err) => {
            log.shutdown().await;
            let _ = worker.await;
            return Err(err);
        }
    };

    let started = Instant::now();
    let json_template = Arc::new(json_bytes);
    let image_file = Arc::new(image_bytes);
    let (json_summary, image_summary) = tokio::join!(
        json_test::run(
            client.clone(),
            json_template,
            event_at,
            args.n,
            stop_rx.clone(),
            log.clone(),
        ),
        image_test::run(client, image_file, args.n, stop_rx, log.clone(),),
    );

    let secs = started.elapsed().as_secs_f64();
    log.line_wait(&format!("elapsed {secs:.1}s")).await;
    log.line_wait(&format!(
        "json ok={} fail={}",
        json_summary.ok, json_summary.fail
    ))
    .await;
    log.line_wait(&format!(
        "image ok={} fail={}",
        image_summary.ok, image_summary.fail
    ))
    .await;
    let dropped = log.dropped_count();
    if dropped > 0 {
        log.line_wait(&format!("console dropped {dropped}")).await;
    }
    log.shutdown().await;
    let _ = worker.await;
    Ok(())
}

fn load_samples(unpacked: &Unpacked) -> Result<(reqwest::Client, Vec<u8>, Vec<u8>, usize), String> {
    let client = util::build_client()?;
    let json_bytes = std::fs::read(&unpacked.json_path)
        .map_err(|err| format!("{}: {err}", unpacked.json_path.display()))?;
    let image_bytes = std::fs::read(&unpacked.image_path)
        .map_err(|err| format!("{}: {err}", unpacked.image_path.display()))?;
    let event_at = event_id_at(&json_bytes)
        .ok_or_else(|| format!("{} has no event_id", unpacked.json_path.display()))?;
    if !image_bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(format!("{} is not a png", unpacked.image_path.display()));
    }
    Ok((client, json_bytes, image_bytes, event_at))
}

fn notes(unpacked: &Unpacked) -> [String; 2] {
    let json_word = if unpacked.json_wrote { "unpack" } else { "keep" };
    let image_word = if unpacked.image_wrote { "unpack" } else { "keep" };
    [
        format!(
            "{json_word} {} {}",
            unpacked.json_path.display(),
            unpacked.json_len
        ),
        format!(
            "{image_word} {} {}",
            unpacked.image_path.display(),
            unpacked.image_len
        ),
    ]
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut n = 0u64;
    let mut unpack_only = false;
    let mut saw_n = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "unpack" {
            unpack_only = true;
            i += 1;
            continue;
        }
        if a == "-n" || a == "--n" {
            let v = args
                .get(i + 1)
                .ok_or_else(|| "missing value for -n".to_string())?;
            n = parse_n(v)?;
            saw_n = true;
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("-n=").or_else(|| a.strip_prefix("--n=")) {
            n = parse_n(v)?;
            saw_n = true;
            i += 1;
            continue;
        }
        return Err(format!("unknown argument: {a}"));
    }
    if unpack_only && saw_n {
        return Err("unpack does not take -n".into());
    }
    Ok(Args { n, unpack_only })
}

fn parse_n(v: &str) -> Result<u64, String> {
    v.parse::<u64>()
        .map_err(|_| format!("bad -n value: {v}"))
}
