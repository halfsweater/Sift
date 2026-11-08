//! `sift` — request-driven web data extraction.

mod commands;
mod run;

use clap::{Args, Parser, Subcommand};
use std::net::SocketAddr;
use std::path::PathBuf;

const AFTER_HELP: &str = "\
FIELDS (comma separated, aliases in parentheses):
  contact   email  phone (tel, mobile)  fax  whatsapp  contact_form
  business  company_name (name)  website (url)  description  category  address
            city  region  country  hours  team  services
  web       title  meta_description  headings  links  canonical
            instagram  facebook  linkedin  twitter  youtube  github  tiktok  (or `socials` for all)
  technical status_code  response_time  content_type  redirect_chain  technologies  sitemaps

Sift obeys robots.txt, rate-limits per domain, and never bypasses CAPTCHAs, logins or paywalls.
Only collect data you are permitted to collect, and respect privacy and data-protection law.";

#[derive(Parser)]
#[command(name = "sift", version, about = "Tell Sift what data you need. It finds it, extracts it, cleans it, and returns it structured.", after_help = AFTER_HELP)]
struct Cli {
    /// More logging (-v debug, -vv trace). `RUST_LOG` also works.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,
    /// Hide the progress indicator and the summary printed to stderr.
    #[arg(short, long, global = true)]
    quiet: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scrape one or more websites for specific fields (follows only relevant pages)
    Scrape(ScrapeArgs),
    /// Discover websites for a natural-language request, then scrape them
    Search(SearchArgs),
    /// Crawl a site (all in-scope pages, most relevant first) and report per-page data
    Crawl(CrawlCmdArgs),
    /// Extract fields from exactly one page, without crawling
    Extract(ExtractArgs),
    /// Convert a saved JSON result file to another format
    Export(ExportArgs),
    /// Show technical and SEO facts about a page (status, redirects, tech, sitemaps, ...)
    Inspect(InspectArgs),
    /// Run the HTTP API
    Serve(ServeArgs),
}

/// How to talk to websites.
#[derive(Args, Clone)]
pub struct FetchArgs {
    /// Parallel requests
    #[arg(short, long, default_value_t = 8)]
    pub concurrency: usize,
    /// Per-request timeout in seconds
    #[arg(long, default_value_t = 15)]
    pub timeout: u64,
    /// Minimum delay between requests to the same domain, in milliseconds
    #[arg(long, default_value_t = 250)]
    pub delay_ms: u64,
    /// Retries on timeouts, 429 and 5xx (exponential backoff)
    #[arg(long, default_value_t = 2)]
    pub retries: u32,
    /// Maximum response size in MiB (larger bodies are truncated)
    #[arg(long, default_value_t = 5)]
    pub max_body_mb: usize,
    /// User-Agent header
    #[arg(long)]
    pub user_agent: Option<String>,
}

/// What to extract and how to clean it.
#[derive(Args, Clone)]
pub struct SpecArgs {
    /// Fields to extract, e.g. `name,email,phone,socials`
    #[arg(short, long)]
    pub fields: Option<String>,
    /// Natural-language request; used to pick fields when --fields is absent
    #[arg(short, long)]
    pub request: Option<String>,
    /// Keep duplicate values
    #[arg(long)]
    pub no_dedupe: bool,
    /// Skip validation (every value stays `unverified`)
    #[arg(long)]
    pub no_validate: bool,
    /// Keep values that fail validation, flagged `invalid`, instead of dropping them
    #[arg(long)]
    pub keep_invalid: bool,
    /// Omit source URLs from the output
    #[arg(long)]
    pub no_source: bool,
}

/// How far to crawl.
#[derive(Args, Clone)]
pub struct DepthArgs {
    /// Link depth from the start URL
    #[arg(short, long)]
    pub depth: Option<u32>,
    /// Maximum pages per site
    #[arg(short = 'm', long)]
    pub max_pages: Option<usize>,
    /// Also follow links to subdomains
    #[arg(long)]
    pub subdomains: bool,
    /// Don't use sitemap.xml to find pages
    #[arg(long)]
    pub no_sitemap: bool,
}

/// Where results go.
#[derive(Args, Clone)]
pub struct OutputArgs {
    /// json, jsonl, csv or table (default: inferred from --output, else table)
    #[arg(long)]
    pub format: Option<String>,
    /// Write to a file instead of stdout
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Table page to show
    #[arg(long, default_value_t = 1)]
    pub page: usize,
    /// Rows per table page
    #[arg(long, default_value_t = 25)]
    pub page_size: usize,
}

#[derive(Args)]
pub struct ScrapeArgs {
    /// Website URLs (https:// is assumed when missing)
    #[arg(required = true)]
    pub urls: Vec<String>,
    #[command(flatten)]
    pub spec: SpecArgs,
    #[command(flatten)]
    pub depth: DepthArgs,
    #[command(flatten)]
    pub fetch: FetchArgs,
    #[command(flatten)]
    pub out: OutputArgs,
}

#[derive(Args)]
pub struct SearchArgs {
    /// What to look for, e.g. "software agencies in Kathmandu"
    pub query: String,
    /// Start from these URLs instead of (or in addition to) searching; repeatable
    #[arg(long = "seed")]
    pub seeds: Vec<String>,
    /// File with one seed URL per line
    #[arg(long)]
    pub seeds_file: Option<PathBuf>,
    /// SearxNG instance (JSON output enabled) used to discover websites
    #[arg(long, env = "SIFT_SEARCH_ENDPOINT")]
    pub search_endpoint: Option<String>,
    /// Maximum number of discovered websites
    #[arg(long, default_value_t = 10)]
    pub limit: usize,
    #[command(flatten)]
    pub spec: SpecArgs,
    #[command(flatten)]
    pub depth: DepthArgs,
    #[command(flatten)]
    pub fetch: FetchArgs,
    #[command(flatten)]
    pub out: OutputArgs,
}

#[derive(Args)]
pub struct CrawlCmdArgs {
    pub url: String,
    /// Fields per page (default: title,status_code,response_time)
    #[arg(short, long)]
    pub fields: Option<String>,
    /// Only follow links likely to contain the requested fields
    #[arg(long)]
    pub relevant_only: bool,
    #[arg(long)]
    pub no_dedupe: bool,
    #[arg(long)]
    pub no_validate: bool,
    #[arg(long)]
    pub no_source: bool,
    #[command(flatten)]
    pub depth: DepthArgs,
    #[command(flatten)]
    pub fetch: FetchArgs,
    #[command(flatten)]
    pub out: OutputArgs,
}

#[derive(Args)]
pub struct ExtractArgs {
    /// Page URL
    pub url: String,
    #[command(flatten)]
    pub spec: SpecArgs,
    #[command(flatten)]
    pub fetch: FetchArgs,
    #[command(flatten)]
    pub out: OutputArgs,
}

#[derive(Args)]
pub struct ExportArgs {
    /// A result file produced with `--format json`
    pub file: PathBuf,
    /// json, jsonl, csv or table
    #[arg(long)]
    pub format: Option<String>,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    #[arg(long, default_value_t = 1)]
    pub page: usize,
    #[arg(long, default_value_t = 25)]
    pub page_size: usize,
}

#[derive(Args)]
pub struct InspectArgs {
    pub url: String,
    #[command(flatten)]
    pub fetch: FetchArgs,
}

#[derive(Args)]
pub struct ServeArgs {
    #[arg(long, default_value = "127.0.0.1:8080")]
    pub addr: SocketAddr,
    /// Allow scraping loopback/private addresses. Only for local testing: this disables SSRF protection.
    #[arg(long)]
    pub allow_private_hosts: bool,
    #[command(flatten)]
    pub fetch: FetchArgs,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let level = match cli.verbose {
        0 => "warn",
        1 => "info,sift=debug",
        _ => "trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| level.into());
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).init();

    let quiet = cli.quiet;
    let result = match cli.command {
        Command::Scrape(a) => commands::scrape(a, quiet).await,
        Command::Search(a) => commands::search(a, quiet).await,
        Command::Crawl(a) => commands::crawl(a, quiet).await,
        Command::Extract(a) => commands::extract(a, quiet).await,
        Command::Export(a) => commands::export(a),
        Command::Inspect(a) => commands::inspect(a, quiet).await,
        Command::Serve(a) => commands::serve(a).await,
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
