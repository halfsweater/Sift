# Architecture

```
URL input / seeds ─► (discovery) ─► crawl ─► fetch ─► parse & extract ─► validate ─► normalize ─► dedupe ─► records
 sift-cli, sift-api   sift-engine   sift-crawler  sift-fetcher   sift-extract     └────────── sift-normalize ──────────┘   sift-engine
                      ::search                                                                                    shared types: sift-core
```

Each stage is a separate crate (or module) with its own tests; dependencies only point downwards.

| Crate | Responsibility | Depends on |
|---|---|---|
| `sift-core` | `FieldKind` vocabulary, `ExtractionSpec`, the rule-based request interpreter, the data model (`Extracted`, `Record`, `ResultSet`), JSON/JSONL/CSV/table renderers | – |
| `sift-fetcher` | HTTP fetching: manual redirects with chain, robots.txt (cached, `Crawl-delay`), per-domain rate limiter, retry with exponential backoff + jitter, streamed size cap, charset decoding, SSRF guard (IP literals **and** DNS results), cancellation | – |
| `sift-normalize` | URL normalisation / keys, **validation**, value **normalization**, **deduplication** | `sift-core` |
| `sift-extract` | Parses one page (`scraper`) and runs only the extractors needed for the requested fields; returns owned data + followable links | `sift-core`, `sift-fetcher`, `sift-normalize` |
| `sift-crawler` | Priority queue of URLs scored by relevance, bounded concurrency (`JoinSet`), scope/depth/page limits, sitemap discovery, progress callback, graceful cancel | the four above |
| `sift-engine` | Wires everything: crawl → group → validate → normalize → dedupe → `ResultSet`; optional SearxNG discovery | all of the above |
| `sift-api` | Axum REST API with async in-memory jobs, input validation, hard limits | `sift-engine` |
| `sift-cli` | The `sift` binary (clap, progress UI, Ctrl-C → partial results) | `sift-engine`, `sift-api` |

## Design decisions

**Extractors propose, validators decide.** An extractor reports a value, how it found it (`Method`) and a
confidence. It never claims validity. The validation stage sets the status:

* syntax fails → `invalid` (dropped and counted, or kept and flagged with `keep_invalid`)
* syntax ok **and** the method is explicit markup (`mailto:`, `tel:`, JSON-LD, microdata, meta tag, a dedicated
  element, an anchor, HTTP data) → `valid`
* syntax ok but pulled from free text / de-obfuscated / guessed from layout → `unverified`

"Missing" is an empty list. Nothing is ever fabricated, and nothing is ever "verified" in the sense of contacting a
mail server or dialling a number.

**Only extract what was asked.** `analyze()` receives the requested fields; contact, business, social and technical
extractors are skipped when unneeded. The same field list drives crawl prioritisation
(`sift-crawler/src/priority.rs`): asking for `email` favours `/contact`, `/about`, `/team`, `/support`; asking for
`title` alone favours nothing, so only the seed is fetched. In `relevant_only` mode links scoring ≤ 0 are not followed.

**Politeness is not optional.** robots.txt is checked for every hop of every request (4xx → allow, 2xx → parse,
5xx/unreachable → refuse), `Crawl-delay` raises the per-domain spacing, and there is no switch to ignore robots.txt in
the CLI or API. Sift implements no CAPTCHA, login or paywall workarounds.

**The DOM is `!Send`.** `analyze()` is synchronous and returns owned data; the crawler runs it in `spawn_blocking`.

**Ordering & determinism.** Pages are processed in (depth, URL) order, records are sorted by website, and values
within a field are sorted best-first (status, confidence) with first-appearance as the tie-break.

**Dedup keys.** Emails: lower-cased. Phones: last 9 digits (so `+977 980-123-4567` equals `9801234567` without a
country-code table). URLs: scheme/`www`/fragment/tracking params/query order/trailing slash ignored. Text:
case- and whitespace-insensitive. The best representative wins (validated > unverified, then confidence, then
international phone format).

## Testing strategy

* Unit tests sit next to the code (`#[cfg(test)]`) for pure logic: extractors, validators, normalisation, dedupe, URL
  keys, the interpreter, renderers.
* Integration tests start **local axum servers on `127.0.0.1:0`** — no test touches the internet:
  `sift-fetcher/tests` (retries, redirects, robots, timeouts, size cap, cancellation, SSRF),
  `sift-crawler/tests` (relevance, robots, sitemap, depth/page limits, dedupe, concurrency, ordering, cancellation),
  `sift-engine/tests` (full pipeline, multi-site, discovery via a mock SearxNG), `sift-api/tests` (job lifecycle,
  validation, limits, eviction).
* Benchmarks: `cargo bench -p sift-extract` (page analysis) and `cargo bench -p sift-normalize` (URL keys, dedupe).

## Known limitations

* No JavaScript rendering: content injected client-side is invisible to Sift.
* No bundled search engine; discovery needs seed URLs or a SearxNG endpoint.
* Phone numbers without a visible country code stay in national form; there is no libphonenumber-style region data.
* Layout-based extraction (team members, services, `<address>`, name from `<title>`) is heuristic and always
  `unverified` with low confidence.
* Jobs in the API are in memory only. `sift-storage` (SQLite) from the original plan was deliberately not built: it
  was optional and would have been a stub.
* Terminal tables count characters, not display width, so wide (CJK/emoji) text may misalign.
