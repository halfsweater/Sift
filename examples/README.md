# Examples

Everything here runs offline against a fictional demo website.

```bash
# terminal 1 – serve the demo site on http://127.0.0.1:8099
cargo run -p sift-engine --example demo_site

# terminal 2 – use the CLI
cargo run -p sift-cli -- scrape http://127.0.0.1:8099 --fields name,email,phone,address,socials
cargo run -p sift-cli -- scrape http://127.0.0.1:8099 -r "Get the email and phone number of every agency." --format json
cargo run -p sift-cli -- crawl  http://127.0.0.1:8099 --depth 2
cargo run -p sift-cli -- inspect http://127.0.0.1:8099/contact

# …or the HTTP API (--allow-private-hosts is needed because the demo site is on loopback)
cargo run -p sift-cli -- serve --allow-private-hosts &
curl -s localhost:8080/api/v1/scrape -H 'content-type: application/json' -d @examples/api/scrape.json
```

* `api/scrape.json`, `api/natural-language.json` – request bodies for `POST /api/v1/scrape`.
* `extraction-spec.json` – what the natural-language interpreter produces (`ExtractionSpec`), for reference.
* `crates/sift-engine/examples/demo_site.rs` – the demo site itself.
