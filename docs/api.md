# Sift HTTP API

Start the server with `sift serve --addr 127.0.0.1:8080`. All endpoints are under `/api/v1`, speak JSON and
return errors in one shape:

```json
{ "error": { "code": "invalid_request", "message": "`max_pages` must be between 1 and 200" } }
```

| Code | HTTP | Meaning |
|---|---|---|
| `invalid_request` | 400 | Malformed JSON, unknown fields, bad URL/field names, or a limit was exceeded |
| `job_not_found` | 404 | Unknown (or already deleted / evicted) job id |
| `job_not_finished` | 409 | Results requested while the job is still running |
| `too_many_jobs` | 429 | Too many jobs running at once, or the job store is full |
| `job_failed` | 500 | The job crashed internally (should not happen; please report it) |

## Safety defaults

* robots.txt is always honoured, with per-domain rate limiting and retries with backoff.
* Targets on loopback, private, link-local and carrier-grade-NAT ranges are **refused** (SSRF protection),
  including hostnames that resolve only to such addresses. `localhost`, `*.local` and `*.internal` are refused too.
* Limits (rejected, never silently clamped): at most 20 `urls`, `depth` ≤ 4, `max_pages` ≤ 200 per site,
  `concurrency` ≤ 16, 8 concurrently running jobs, 200 stored jobs (oldest finished job is evicted first).
* Jobs live in memory and are lost when the server restarts. There is no authentication: run Sift behind a
  reverse proxy or on a trusted network.

## Request body

`POST /scrape` and `POST /crawl` take:

| Field | Type | Default | Notes |
|---|---|---|---|
| `urls` | string[] | – (required) | 1–20 sites. A missing scheme means `https://`. Duplicates collapse. |
| `fields` | string[] | – | e.g. `["company_name","email","phone","instagram"]`. `"socials"` expands to all social networks. |
| `query` | string | – | Natural-language request, interpreted by the built-in rule-based parser when `fields` is absent. |
| `depth` | integer | 2 | Link depth from each URL. |
| `max_pages` | integer | 15 (`scrape`), 50 (`crawl`) | Page budget **per site**. |
| `concurrency` | integer | 8 | Parallel requests (per-domain rate limits still apply). |
| `include_subdomains` | boolean | false | Follow links to subdomains of the given hosts. |
| `deduplicate` | boolean | true | |
| `validate` | boolean | true | Invalid values are dropped and counted in `metadata.invalid_values_discarded`. |
| `keep_invalid` | boolean | false | Keep values that fail validation, flagged `"status": "invalid"`, instead of dropping them. |
| `include_source` | boolean | true | Include the source URL of every value. |

Either `fields` or `query` is required. Unknown properties are rejected.

* **`/scrape`** follows only links likely to hold the requested fields (contact/about/team/...) and returns
  **one record per website**.
* **`/crawl`** follows all in-scope links (still ordered by relevance) and returns **one record per page**.

`POST /extract` takes `url` (a single string) plus `query`/`fields`/`deduplicate`/`validate`/`include_source`.
It fetches exactly one page, synchronously.

## Endpoints

### `POST /api/v1/scrape` · `POST /api/v1/crawl` → `202 Accepted`

```bash
curl -s localhost:8080/api/v1/scrape -H 'content-type: application/json' -d '{
  "urls": ["https://agency.example"],
  "fields": ["company_name", "email", "phone", "instagram"],
  "depth": 2, "max_pages": 20
}'
```

```json
{ "id": "6f1c…", "kind": "scrape", "status": "running",
  "links": { "self": "/api/v1/jobs/6f1c…", "results": "/api/v1/jobs/6f1c…/results" } }
```

### `POST /api/v1/extract` → `200 OK`

Returns a result set (see below) directly.

### `GET /api/v1/jobs/{id}` → `200 OK`

```json
{ "id": "6f1c…", "kind": "scrape", "status": "running", "created_at": 1767225600, "finished_at": null,
  "stats": { "pages_fetched": 4, "pages_ok": 4, "pages_failed": 0, "blocked_by_robots": 0, "queued": 3,
             "in_flight": 2, "bytes_downloaded": 81234, "elapsed_ms": 1432, "…": "…" },
  "error": null }
```

`status` is one of `running`, `completed`, `cancelled`, `failed`.

### `GET /api/v1/jobs/{id}/results` → `200 OK` | `409`

The result set once the job is finished:

```json
{
  "query": null,
  "spec": { "fields": ["company_name", "email"], "deduplicate": true, "validate": true, "include_source": true },
  "results": [
    {
      "website": "https://agency.example",
      "pages_visited": 3,
      "fields": {
        "company_name": [{ "field": "company_name", "value": "Agency Ltd.", "source": "https://agency.example/",
                           "method": "json_ld", "status": "valid", "confidence": 0.9 }],
        "email": []
      }
    }
  ],
  "metadata": { "pages_crawled": 3, "pages_failed": 0, "pages_blocked_by_robots": 0, "records_found": 1,
                "duplicates_removed": 2, "invalid_values_discarded": 1, "elapsed_ms": 1432, "cancelled": false }
}
```

Reading the data:

* A requested field with **no** value is an empty array — "missing" is never replaced by a guess.
* `status`: `valid` = syntactically valid **and** found in explicit markup (`mailto:`, `tel:`, JSON-LD, meta tags,
  …); `unverified` = plausible but pulled from free text or inferred; `invalid` = failed validation (dropped and
  counted in `metadata.invalid_values_discarded`, unless `keep_invalid` is `true`, which keeps them flagged).
  With `validate: false` nothing is checked and every value is `unverified`. Sift never
  contacts mail servers or dials numbers, so even `valid` does not mean *deliverable*.
* Values within a field are ordered best first (status, then confidence).

### `DELETE /api/v1/jobs/{id}` → `204 No Content`

Cancels the job if it is running and removes it and its results.

### `GET /api/v1/health` · `GET /api/v1/docs`

Liveness probe, and this document as `text/markdown`.
