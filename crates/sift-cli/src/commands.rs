//! Command implementations.

use crate::run::{check_outcome, emit, fetcher_config, run_with_progress, write_output};
use crate::{
    CrawlCmdArgs, DepthArgs, ExportArgs, ExtractArgs, InspectArgs, ScrapeArgs, SearchArgs, ServeArgs, SpecArgs,
};
use anyhow::{anyhow, bail, Context, Result};
use sift_core::output::{OutputFormat, Pager};
use sift_core::{ExtractionSpec, FieldKind, RequestInterpreter, ResultSet, RuleBasedInterpreter};
use sift_engine::{build_search_query, parse_seeds, Engine, RecordScope, RunOptions, SearxngProvider};

fn field_help() -> &'static str {
    "run `sift --help` to see the available fields"
}

/// Turn `--fields` / `--request` into a spec. `--fields` wins; `--request` supplies subject/location.
fn build_spec(a: &SpecArgs, fallback_request: Option<&str>) -> Result<ExtractionSpec> {
    let request = a.request.as_deref().or(fallback_request);
    let mut spec = match (&a.fields, request) {
        (Some(f), r) => {
            let fields = FieldKind::parse_list(f).map_err(|e| anyhow!("{e}"))?;
            if fields.is_empty() {
                bail!("--fields is empty; {}", field_help());
            }
            let mut s = r.map_or_else(|| ExtractionSpec::new(fields.clone()), |r| RuleBasedInterpreter.interpret(r));
            s.fields = fields;
            s
        }
        (None, Some(r)) => RuleBasedInterpreter.interpret(r),
        (None, None) => bail!(
            "tell Sift what to collect with --fields (e.g. --fields name,email,phone) or --request \"…\"; {}",
            field_help()
        ),
    };
    spec.deduplicate = !a.no_dedupe;
    spec.validate = !a.no_validate;
    spec.keep_invalid = a.keep_invalid;
    spec.include_source = !a.no_source;
    Ok(spec)
}

fn options(
    d: &DepthArgs,
    concurrency: usize,
    default_depth: u32,
    default_pages: usize,
    relevant_only: bool,
    scope: RecordScope,
) -> RunOptions {
    RunOptions {
        max_depth: d.depth.unwrap_or(default_depth),
        max_pages: d.max_pages.unwrap_or(default_pages).max(1),
        concurrency: concurrency.max(1),
        relevant_only,
        use_sitemaps: !d.no_sitemap,
        include_subdomains: d.subdomains,
        scope,
    }
}

fn engine(f: &crate::FetchArgs) -> Result<Engine> {
    Engine::new(fetcher_config(f)).context("failed to initialise the HTTP client")
}

pub async fn scrape(a: ScrapeArgs, quiet: bool) -> Result<()> {
    let spec = build_spec(&a.spec, None)?;
    let seeds = parse_seeds(&a.urls)?;
    let opts = options(&a.depth, a.fetch.concurrency, 2, 15, true, RecordScope::Site);
    let rs = run_with_progress(&engine(&a.fetch)?, seeds, spec, opts, quiet).await;
    emit(&rs, &a.out, quiet)?;
    check_outcome(&rs)
}

pub async fn search(a: SearchArgs, quiet: bool) -> Result<()> {
    let spec = build_spec(&a.spec, Some(&a.query))?;
    let mut inputs = a.seeds.clone();
    if let Some(file) = &a.seeds_file {
        let text = std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
        inputs.extend(text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from));
    }
    let mut seeds = if inputs.is_empty() { Vec::new() } else { parse_seeds(&inputs)? };

    if let Some(endpoint) = &a.search_endpoint {
        let q = build_search_query(&spec).unwrap_or_else(|| a.query.clone());
        if !quiet {
            eprintln!("Searching for “{q}” via {endpoint}");
        }
        let provider = SearxngProvider::new(endpoint, &fetcher_config(&a.fetch).user_agent)?;
        let found = provider.search(&q, a.limit).await?;
        if !quiet {
            eprintln!("Discovered {} website(s)", found.len());
        }
        seeds.extend(found);
    }
    if seeds.is_empty() {
        bail!(
            "no websites to scrape. Sift has no built-in search engine: pass --seed <url> (repeatable) or --seeds-file, \
             or point --search-endpoint (env SIFT_SEARCH_ENDPOINT) at a SearxNG instance with JSON output enabled"
        );
    }
    let opts = options(&a.depth, a.fetch.concurrency, 2, 15, true, RecordScope::Site);
    let rs = run_with_progress(&engine(&a.fetch)?, seeds, spec, opts, quiet).await;
    emit(&rs, &a.out, quiet)?;
    check_outcome(&rs)
}

pub async fn crawl(a: CrawlCmdArgs, quiet: bool) -> Result<()> {
    let spec_args = SpecArgs {
        fields: Some(a.fields.clone().unwrap_or_else(|| "title,status_code,response_time".into())),
        request: None,
        no_dedupe: a.no_dedupe,
        no_validate: a.no_validate,
        keep_invalid: false,
        no_source: a.no_source,
    };
    let spec = build_spec(&spec_args, None)?;
    let seeds = parse_seeds(std::slice::from_ref(&a.url))?;
    let opts = options(&a.depth, a.fetch.concurrency, 3, 100, a.relevant_only, RecordScope::Page);
    let rs = run_with_progress(&engine(&a.fetch)?, seeds, spec, opts, quiet).await;
    emit(&rs, &a.out, quiet)?;
    check_outcome(&rs)
}

pub async fn extract(a: ExtractArgs, quiet: bool) -> Result<()> {
    let spec = build_spec(&a.spec, None)?;
    let seeds = parse_seeds(std::slice::from_ref(&a.url))?;
    let opts = RunOptions { max_depth: 0, max_pages: 1, concurrency: 1, use_sitemaps: false, ..RunOptions::default() };
    let rs = run_with_progress(&engine(&a.fetch)?, seeds, spec, opts, quiet).await;
    emit(&rs, &a.out, quiet)?;
    check_outcome(&rs)
}

pub fn export(a: ExportArgs) -> Result<()> {
    let text = std::fs::read_to_string(&a.file).with_context(|| format!("cannot read {}", a.file.display()))?;
    let rs: ResultSet = serde_json::from_str(&text).map_err(|e| {
        anyhow!(
            "{} is not a Sift JSON result file ({e}). Create one with `--format json --output results.json`",
            a.file.display()
        )
    })?;
    let format = match (&a.format, &a.output) {
        (Some(f), _) => f.parse::<OutputFormat>().map_err(anyhow::Error::msg)?,
        (None, Some(p)) => OutputFormat::from_path(&p.to_string_lossy())
            .ok_or_else(|| anyhow!("cannot infer a format from the output path; pass --format"))?,
        (None, None) => bail!("choose a target with --format csv|jsonl|json|table (and optionally --output <file>)"),
    };
    write_output(&rs, format, a.output.as_deref(), Pager { page: a.page, page_size: a.page_size }, false)
}

pub async fn inspect(a: InspectArgs, quiet: bool) -> Result<()> {
    use FieldKind as F;
    let fields = [
        F::StatusCode,
        F::ResponseTime,
        F::ContentType,
        F::RedirectChain,
        F::Technologies,
        F::Sitemaps,
        F::Title,
        F::MetaDescription,
        F::Canonical,
        F::Headings,
        F::CompanyName,
        F::Email,
        F::Phone,
        F::Whatsapp,
        F::ContactForm,
        F::Instagram,
        F::Facebook,
        F::Linkedin,
        F::Twitter,
        F::Youtube,
        F::Github,
        F::Tiktok,
    ];
    let spec = ExtractionSpec::new(fields.to_vec());
    let seeds = parse_seeds(std::slice::from_ref(&a.url))?;
    let opts = RunOptions { max_depth: 0, max_pages: 1, concurrency: 1, use_sitemaps: true, ..RunOptions::default() };
    let rs = run_with_progress(&engine(&a.fetch)?, seeds.clone(), spec, opts, quiet).await;

    println!("Inspecting {}", seeds[0]);
    let Some(rec) = rs.results.first() else {
        if rs.metadata.pages_blocked_by_robots > 0 {
            println!("  robots.txt   DISALLOWS this URL for Sift — nothing was fetched");
        }
        check_outcome(&rs)?;
        return Ok(());
    };
    println!("  robots.txt   allowed");
    let show = |label: &str, f: F, max: usize| {
        let vals: Vec<&str> = rec.values(f).take(max).collect();
        let total = rec.fields.get(&f).map_or(0, Vec::len);
        let text = if vals.is_empty() {
            "—".to_string()
        } else if total > vals.len() {
            format!("{}  (+{} more)", vals.join(" · "), total - vals.len())
        } else {
            vals.join(" · ")
        };
        println!("  {label:<14}{text}");
    };
    println!("\nHTTP");
    show("status", F::StatusCode, 1);
    show("response time", F::ResponseTime, 1);
    show("content type", F::ContentType, 1);
    let redirects = rec.values(F::RedirectChain).next().unwrap_or("none");
    println!("  {:<14}{redirects}", "redirects");
    println!("\nPage");
    show("title", F::Title, 1);
    show("description", F::MetaDescription, 1);
    show("canonical", F::Canonical, 1);
    show("headings", F::Headings, 6);
    show("technologies", F::Technologies, 12);
    show("sitemaps", F::Sitemaps, 5);
    println!("\nContact & social (from this page only; use `sift scrape` to crawl for more)");
    show("name", F::CompanyName, 1);
    show("email", F::Email, 5);
    show("phone", F::Phone, 5);
    show("whatsapp", F::Whatsapp, 2);
    show("contact form", F::ContactForm, 1);
    for (label, f) in [
        ("instagram", F::Instagram),
        ("facebook", F::Facebook),
        ("linkedin", F::Linkedin),
        ("twitter/x", F::Twitter),
        ("youtube", F::Youtube),
        ("github", F::Github),
        ("tiktok", F::Tiktok),
    ] {
        show(label, f, 1);
    }
    if !rs.metadata.errors.is_empty() {
        println!();
        rs.metadata.errors.iter().take(3).for_each(|e| println!("  note: {e}"));
    }
    Ok(())
}

pub async fn serve(a: ServeArgs) -> Result<()> {
    let mut cfg = sift_api::ApiConfig::default();
    let user_agent = a.fetch.user_agent.clone();
    cfg.fetcher = fetcher_config(&a.fetch);
    cfg.fetcher.block_private_hosts = !a.allow_private_hosts;
    if let Some(ua) = user_agent {
        cfg.fetcher.user_agent = ua;
    }
    if a.allow_private_hosts {
        eprintln!(
            "warning: --allow-private-hosts disables SSRF protection; do not expose this server to untrusted clients"
        );
    }
    eprintln!("Sift API listening on http://{}  (docs: /api/v1/docs)", a.addr);
    sift_api::serve(a.addr, cfg, async {
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("shutting down…");
    })
    .await
    .with_context(|| format!("could not serve on {}", a.addr))
}
