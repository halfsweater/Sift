//! Shared plumbing for commands: fetcher setup, progress UI, Ctrl-C handling, output.

use crate::{FetchArgs, OutputArgs};
use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use sift_core::output::{self, OutputFormat, Pager};
use sift_core::ResultSet;
use sift_engine::{CrawlStats, Engine, ProgressFn, RunOptions};
use sift_fetcher::FetcherConfig;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use url::Url;

pub fn fetcher_config(a: &FetchArgs) -> FetcherConfig {
    let mut cfg = FetcherConfig {
        timeout: Duration::from_secs(a.timeout.max(1)),
        per_domain_delay: Duration::from_millis(a.delay_ms),
        max_retries: a.retries,
        max_body_bytes: a.max_body_mb.max(1) * 1024 * 1024,
        ..FetcherConfig::default()
    };
    if let Some(ua) = &a.user_agent {
        cfg.user_agent = ua.clone();
    }
    cfg
}

/// Cancel `token` on Ctrl-C so the engine can return partial results.
pub fn cancel_on_ctrl_c() -> CancellationToken {
    let token = CancellationToken::new();
    let t = token.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\nInterrupted — finishing in-flight requests and returning partial results…");
            t.cancel();
        }
    });
    token
}

fn human_bytes(b: u64) -> String {
    match b {
        0..=1023 => format!("{b} B"),
        1024..=1_048_575 => format!("{:.1} KiB", b as f64 / 1024.0),
        _ => format!("{:.1} MiB", b as f64 / 1_048_576.0),
    }
}

pub fn progress_line(s: &CrawlStats) -> String {
    format!(
        "{} pages · {} ok · {} failed · {} robots-blocked · {} queued · {} in flight · {} · {:.1}s",
        s.pages_fetched,
        s.pages_ok,
        s.pages_failed,
        s.blocked_by_robots,
        s.queued,
        s.in_flight,
        human_bytes(s.bytes_downloaded),
        s.elapsed_ms as f64 / 1000.0
    )
}

/// Run the engine with a live progress spinner on stderr (hidden when not a terminal or `quiet`).
pub async fn run_with_progress(
    engine: &Engine,
    seeds: Vec<Url>,
    spec: sift_core::ExtractionSpec,
    opts: RunOptions,
    quiet: bool,
) -> ResultSet {
    let pb = if quiet { ProgressBar::hidden() } else { ProgressBar::new_spinner() };
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg}").unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_message(format!("crawling {} site(s)…", seeds.len()));
    let pb2 = pb.clone();
    let progress: ProgressFn = Arc::new(move |s| pb2.set_message(progress_line(s)));
    let rs = engine.run(seeds, spec, opts, cancel_on_ctrl_c(), Some(progress)).await;
    pb.finish_and_clear();
    rs
}

/// Render and emit results. Machine formats on stdout keep stdout clean: the summary goes to stderr.
pub fn emit(rs: &ResultSet, out: &OutputArgs, quiet: bool) -> Result<()> {
    let format = match (&out.format, &out.output) {
        (Some(f), _) => f.parse::<OutputFormat>().map_err(anyhow::Error::msg)?,
        (None, Some(p)) => OutputFormat::from_path(&p.to_string_lossy()).unwrap_or(OutputFormat::Json),
        (None, None) => OutputFormat::Table,
    };
    write_output(rs, format, out.output.as_deref(), Pager { page: out.page, page_size: out.page_size }, quiet)
}

pub fn write_output(
    rs: &ResultSet,
    format: OutputFormat,
    path: Option<&std::path::Path>,
    pager: Pager,
    quiet: bool,
) -> Result<()> {
    let text = output::render(rs, format, pager)?;
    match path {
        Some(p) => {
            if let Some(dir) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
            }
            std::fs::write(p, &text).with_context(|| format!("cannot write {}", p.display()))?;
            if !quiet {
                eprint!("{}", output::summary(rs));
                eprintln!("Wrote {} record(s) to {}", rs.results.len(), p.display());
            }
        }
        None => {
            print!("{text}");
            if format != OutputFormat::Table && !quiet {
                eprint!("{}", output::summary(rs));
            }
        }
    }
    Ok(())
}

/// Fail (after output) when nothing could be collected and there were errors to explain why.
pub fn check_outcome(rs: &ResultSet) -> Result<()> {
    if rs.results.is_empty() && !rs.metadata.cancelled {
        let why = rs.metadata.errors.first().cloned().unwrap_or_else(|| "no pages could be analysed".into());
        bail!("no records collected — {why}");
    }
    Ok(())
}
