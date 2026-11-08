# Sift

**Tell Sift what data you need. Sift finds it, extracts it, cleans it, and gives it back in a structured format.**

Sift is a request-driven web data extraction engine written in Rust. You describe the fields you want
("email, phone, address and Instagram of every agency"); Sift crawls only the pages likely to hold them, extracts
the values, validates and normalises them, removes duplicates and returns **structured records** — never a dump of
page content.

```text
$ sift scrape http://127.0.0.1:8099 --fields name,email,phone,address,socials
┌───────────────────────┬────────────────────────────────┬───────────────────────┬──────────────┬───────────────────────────────┬────────────────────────────────────┬────────────────────────────────────────────┐
│ website               │ company_name                   │ email                 │ phone        │ address                       │ instagram                          │ linkedin                                   │
├───────────────────────┼────────────────────────────────┼───────────────────────┼──────────────┼───────────────────────────────┼────────────────────────────────────┼────────────────────────────────────────────┤
│ http://127.0.0.1:8099 │ Demo Soft Pvt. Ltd.; Demo Soft │ hello@demosoft.com.np │ +97714000000 │ 1 Example Marg, Kathmandu, NP │ https://www.instagram.com/demosoft │ https://www.linkedin.com/company/demo-soft │
└───────────────────────┴────────────────────────────────┴───────────────────────┴──────────────┴───────────────────────────────┴────────────────────────────────────┴────────────────────────────────────────────┘
Showing 1-1 of 1 records (page 1/1)
No values found for: facebook, twitter, youtube, github, tiktok
Summary: 1 record(s) · 3 page(s) crawled · 0 failed · 0 blocked by robots.txt · 4 duplicate(s) removed · 0 invalid value(s) discarded · 1.0s
Coverage: company_name 1/1 · email 1/1 · phone 1/1 · address 1/1 · instagram 1/1 · facebook 0/1 · linkedin 1/1 · twitter 0/1 · youtube 0/1 · github 0/1 · tiktok 0/1
```

(That is the bundled offline demo site — see [`examples/`](examples/README.md).)

## Features

* **Request-driven.** `--fields email,phone` or a natural-language request. The built-in interpreter is
  rule-based and offline; no LLM is required anywhere, and `RequestInterpreter` is a trait if you want to plug one in.
* **Only what you ask for.** Extractors for fields you did not request do not run, and the crawler only follows
  links that look relevant to the requested fields (`/contact`, `/about`, `/team`, `/services`, …; blog/login/cart
  paths are avoided).
* **Honest data.** Every value carries its field, source URL, extraction method, validation status and confidence.
  Missing values are empty lists — never invented. Values from free text or layout guesses are `unverified`.
* **Extractors:** emails (mailto, text, `name [at] host [dot] com`, Cloudflare-protected), phones and fax
  (tel links, labelled text, international), WhatsApp links, contact forms, company names, descriptions, categories,
  addresses (JSON-LD, microdata, `<address>`), city/region/country, opening hours, team and services (structured data
  first, layout heuristics second), titles, meta descriptions, headings, links, canonical URLs, social profiles
  (Instagram, Facebook, LinkedIn, X/Twitter, YouTube, GitHub, TikTok — share buttons are rejected), HTTP status,
  response time, content type, redirect chains, detectable technologies and sitemaps.
* **Polite async crawler.** Configurable concurrency, depth and page limits; priority queue; URL normalisation and
  duplicate detection; domain scoping; sitemap discovery; **robots.txt always honoured** (including `Crawl-delay`);
  per-domain rate limiting; retries with exponential backoff; response-size cap; Ctrl-C returns partial results.
  No CAPTCHA, login, paywall or anti-bot circumvention.
* **Validate → normalise → dedupe.** Emails lower-cased and checked (placeholder/asset/telemetry addresses rejected),
  phones to E.164 where a country code is visible, URLs cleaned of tracking parameters, duplicates merged keeping the
  best-evidenced value.
* **Outputs:** JSON, JSONL, CSV, terminal tables (paginated, with coverage summary).
* **HTTP API** (Axum) with asynchronous jobs, input validation, hard limits and SSRF protection.

## Installation

Requires a recent stable Rust toolchain (developed and tested with 1.99).

```bash
git clone <this repo> && cd Sift
cargo install --path crates/sift-cli      # installs the `sift` binary
# or run without installing:
cargo run -p sift-cli -- --help
```

## Usage

```bash
# Scrape specific fields from a site (follows /contact, /about, … only)
sift scrape https://example.com --fields email,phone,address

# Several sites, JSON to a file (format inferred from the extension)
sift scrape site-a.com site-b.com -f name,email,socials -o results.json

# Describe what you want in plain language
sift scrape https://example.com -r "Get the email, phone number and Instagram account of every agency."

# Discover websites first (needs a SearxNG instance with JSON output, or pass --seed URLs)
export SIFT_SEARCH_ENDPOINT=http://localhost:8888
sift search "software agencies in Kathmandu" --fields name,email,phone,website -o agencies.csv
sift search "dentists in Pokhara" --seed https://clinic-a.example --seeds-file more-sites.txt -f name,phone

# Crawl a whole site and report per-page data (title, status, timing by default)
sift crawl https://example.com --depth 3 --max-pages 100 -o pages.jsonl

# One page only, no crawling
sift extract https://example.com/contact --fields email,phone

# Re-export a saved result
sift export results.json --format csv -o results.csv

# Technical report for a URL
sift inspect https://example.com
```

Progress and the extraction summary go to **stderr**, data to **stdout** (or `--output`), so
`sift scrape … --format json | jq` just works.

### CLI reference

| Command | Purpose | Defaults |
|---|---|---|
| `scrape <urls…>` | Fields from one or more sites, **one record per site**; follows only relevant links | depth 2, 15 pages/site |
| `search "<request>"` | Discover sites (SearxNG and/or `--seed`/`--seeds-file`), then scrape them | `--limit 10` sites |
| `crawl <url>` | All in-scope pages (most relevant first), **one record per page** | depth 3, 100 pages, fields `title,status_code,response_time` |
| `extract <url>` | One page, no crawling | – |
| `export <file>` | Convert a saved JSON result to `json`/`jsonl`/`csv`/`table` | – |
| `inspect <url>` | Status, timing, redirects, technologies, sitemaps, title/meta, contact & social on that page | – |
| `serve` | Run the HTTP API | `--addr 127.0.0.1:8080` |

Common options (see `sift <command> --help`):

| Option | Meaning |
|---|---|
| `-f, --fields a,b,c` | Fields to extract (`socials` expands to all networks) |
| `-r, --request "…"` | Natural-language request (picks fields if `--fields` is absent; supplies subject/location for `search`) |
| `-d, --depth`, `-m, --max-pages` | Crawl limits (pages are counted **per site**) |
| `-c, --concurrency` | Parallel requests (default 8) |
| `--delay-ms`, `--timeout`, `--retries`, `--max-body-mb`, `--user-agent` | Politeness and robustness knobs |
| `--subdomains`, `--no-sitemap` | Scope and discovery |
| `--no-dedupe`, `--no-validate`, `--keep-invalid`, `--no-source` | Pipeline switches |
| `--format json\|jsonl\|csv\|table`, `-o, --output`, `--page`, `--page-size` | Output |
| `-q, --quiet`, `-v/-vv` | Less / more output (`RUST_LOG` also honoured) |

Available fields: `email phone fax whatsapp contact_form company_name website description category address city
region country hours team services title meta_description headings links canonical instagram facebook linkedin
twitter youtube github tiktok status_code response_time content_type redirect_chain technologies sitemaps`.

## Output formats

**JSON** — one result set. Every requested field is present on every record; no value means `[]`.

```json
{
  "query": null,
  "spec": { "fields": ["company_name", "email", "phone"], "deduplicate": true, "validate": true,
            "keep_invalid": false, "include_source": true },
  "results": [{
    "website": "http://127.0.0.1:8099",
    "pages_visited": 3,
    "fields": {
      "email": [{ "field": "email", "value": "hello@demosoft.com.np", "source": "http://127.0.0.1:8099/contact",
                  "method": "mailto_link", "status": "valid", "confidence": 0.95 }],
      "phone": [{ "field": "phone", "value": "+97714000000", "source": "http://127.0.0.1:8099/contact",
                  "method": "tel_link", "status": "valid", "confidence": 0.95 }],
      "company_name": [
        { "field": "company_name", "value": "Demo Soft Pvt. Ltd.", "source": "http://127.0.0.1:8099/",
          "method": "json_ld", "status": "valid", "confidence": 0.9 },
        { "field": "company_name", "value": "Demo Soft", "source": "http://127.0.0.1:8099/",
          "method": "heuristic", "status": "unverified", "confidence": 0.4 }
      ]
    }
  }],
  "metadata": { "pages_crawled": 3, "pages_failed": 0, "pages_blocked_by_robots": 0, "records_found": 1,
                "duplicates_removed": 4, "invalid_values_discarded": 0, "elapsed_ms": 1022, "cancelled": false }
}
```

* `status` — `valid`: syntactically valid **and** found in explicit markup (`mailto:`, `tel:`, JSON-LD, meta tag, …);
  `unverified`: plausible but taken from free text or inferred; `invalid`: failed validation (dropped unless
  `--keep-invalid`). Sift never contacts mail servers or dials numbers, so `valid` does **not** mean deliverable.
* Values in a field are ordered best-first. `method` tells you how a value was found.

**JSONL** — one record per line (stream-friendly). **CSV** — `website`, then one column per requested field
(multiple values joined with `; `) plus `<field>_source` columns unless `--no-source`; column order follows `--fields`.
**Table** — columns with no data anywhere are summarised in a footer line; a coverage line shows how many records
have each field.

## HTTP API

```bash
sift serve --addr 127.0.0.1:8080
curl -s localhost:8080/api/v1/scrape -H 'content-type: application/json' \
     -d '{"urls":["https://example.com"],"fields":["email","phone"]}'
# → 202 {"id":"…","status":"running","links":{…}}
curl -s localhost:8080/api/v1/jobs/<id>            # status + live crawl stats
curl -s localhost:8080/api/v1/jobs/<id>/results    # 200 when finished, 409 while running
curl -s -X DELETE localhost:8080/api/v1/jobs/<id>  # cancel + remove
```

| Endpoint | |
|---|---|
| `POST /api/v1/scrape` | async job, one record per site |
| `POST /api/v1/crawl` | async job, one record per page |
| `POST /api/v1/extract` | synchronous, single page |
| `GET /api/v1/jobs/{id}` · `GET /api/v1/jobs/{id}/results` · `DELETE /api/v1/jobs/{id}` | job lifecycle |
| `GET /api/v1/health` · `GET /api/v1/docs` | liveness; this API reference as Markdown |

Full reference: [`docs/api.md`](docs/api.md). Private/loopback/link-local targets are refused by default, requests that
exceed limits are rejected (not clamped), and jobs live in memory. There is **no authentication** — put the server
behind a reverse proxy or keep it on a trusted network.

## Architecture

Eight small crates, each independently testable (`sift-core`, `sift-fetcher`, `sift-normalize`, `sift-extract`,
`sift-crawler`, `sift-engine`, `sift-api`, `sift-cli`). The pipeline is
*URL input → discovery → fetch → parse → extract → validate → normalise → dedupe → structured output*.
See [`docs/architecture.md`](docs/architecture.md) for the crate map, design decisions and known limitations.

Using the engine as a library:

```rust
use sift_core::{ExtractionSpec, FieldKind};
use sift_engine::{parse_seeds, Engine, RunOptions};
use sift_fetcher::FetcherConfig;
use tokio_util::sync::CancellationToken;

let engine = Engine::new(FetcherConfig::default())?;
let spec = ExtractionSpec::new(vec![FieldKind::CompanyName, FieldKind::Email, FieldKind::Phone]);
let results = engine
    .run(parse_seeds(&["https://example.com"])?, spec, RunOptions::default(), CancellationToken::new(), None)
    .await;
println!("{}", sift_core::output::to_json(&results)?);
```

## Configuration

There is no config file: everything is a CLI flag or an API request field. Library users configure
`sift_fetcher::FetcherConfig` (user agent, timeout, body cap, retries/backoff, per-domain delay, redirect limit,
`block_private_hosts`) and `sift_engine::RunOptions` (depth, pages per site, concurrency, relevance filtering,
sitemaps, subdomains, record scope). `SIFT_SEARCH_ENDPOINT` sets the default discovery endpoint; `RUST_LOG` controls
logging.

## Performance

Async-first (Tokio), bounded concurrency, streamed and size-capped bodies, one HTML parse per page (done on the
blocking pool), no URL is fetched twice per run, robots.txt fetched once per origin. Criterion benchmarks:

```bash
cargo bench -p sift-extract      # parse + extract a 10 KB / 200 KB page
cargo bench -p sift-normalize    # URL keys, validate + dedupe 5,000 values
```

On the development machine (Windows 11, release build) analysing a ~200 KB page for contact fields took ≈1.6 ms,
1,000 URL keys ≈1.3 ms, and validating + deduplicating 5,000 values ≈8 ms. These are measurements on one machine, not
guarantees; real crawls are dominated by network latency and the politeness delay.

## Testing

```bash
cargo test --workspace                       # 108 tests
cargo clippy --workspace --all-targets -- -D warnings
```

No test touches the internet: integration tests spin up local axum servers on `127.0.0.1`. Tests live inside their
crates (`crates/*/tests`, `#[cfg(test)]`) rather than a top-level `tests/` directory, following Cargo convention.

## Limitations

* **No JavaScript rendering** — content injected by client-side scripts is not seen.
* **No built-in search engine** — website discovery needs seed URLs or your own SearxNG instance.
* Heuristic extraction (team, services, `<address>`, name-from-title) is best-effort and marked `unverified`.
* Phone numbers without a visible country code are kept in national form.
* Optional SQLite persistence (`sift-storage`) is **not implemented**; API jobs are in memory.

## Responsible use

Sift honours robots.txt and rate-limits requests, but legality depends on what you collect and how you use it. Respect
site terms, privacy and data-protection law (GDPR and similar), and anti-spam rules before contacting anyone whose
details you extract.

## Contributing

Issues and pull requests are welcome. Before submitting: `cargo fmt`, `cargo clippy --workspace --all-targets
-- -D warnings`, `cargo test --workspace`. Add a unit test for every extractor or validation rule you change, and use
a local test server (never a real website) for anything that needs HTTP. New extractors belong in `sift-extract`
(register the field in `sift-core/src/field.rs`, and add crawl keywords in `sift-crawler/src/priority.rs` if the
field lives on specific pages).

## License

MIT — see [`LICENSE`](LICENSE).
