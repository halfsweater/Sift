//! Request bodies and their conversion into an [`ExtractionSpec`].

use crate::error::ApiError;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use sift_core::{ExtractionSpec, FieldKind, RequestInterpreter, RuleBasedInterpreter};

/// Parse JSON ourselves so malformed bodies produce the same error shape as everything else.
pub fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|e| ApiError::invalid(format!("invalid JSON body: {e}")))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobRequest {
    pub urls: Vec<String>,
    pub query: Option<String>,
    pub fields: Option<Vec<String>>,
    pub depth: Option<u32>,
    pub max_pages: Option<usize>,
    pub concurrency: Option<usize>,
    pub include_subdomains: Option<bool>,
    pub deduplicate: Option<bool>,
    pub validate: Option<bool>,
    pub keep_invalid: Option<bool>,
    pub include_source: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractRequest {
    pub url: String,
    pub query: Option<String>,
    pub fields: Option<Vec<String>>,
    pub deduplicate: Option<bool>,
    pub validate: Option<bool>,
    pub keep_invalid: Option<bool>,
    pub include_source: Option<bool>,
}

fn build_spec(
    query: &Option<String>,
    fields: &Option<Vec<String>>,
    dedupe: Option<bool>,
    validate: Option<bool>,
    keep_invalid: Option<bool>,
    source: Option<bool>,
) -> Result<ExtractionSpec, ApiError> {
    let query = query.as_deref().map(str::trim).filter(|q| !q.is_empty());
    if query.is_some_and(|q| q.len() > 2000) {
        return Err(ApiError::invalid("`query` must be at most 2000 characters"));
    }
    let mut spec = match (fields, query) {
        (Some(f), q) if !f.is_empty() => {
            let parsed = FieldKind::parse_list(&f.join(",")).map_err(|e| ApiError::invalid(e.to_string()))?;
            let mut s = ExtractionSpec::new(parsed);
            s.query = q.map(str::to_string);
            s
        }
        (_, Some(q)) => RuleBasedInterpreter.interpret(q),
        _ => {
            return Err(ApiError::invalid(
                "provide `fields` (e.g. [\"email\", \"phone\"]) or a natural-language `query`",
            ))
        }
    };
    spec.deduplicate = dedupe.unwrap_or(true);
    spec.validate = validate.unwrap_or(true);
    spec.keep_invalid = keep_invalid.unwrap_or(false);
    spec.include_source = source.unwrap_or(true);
    Ok(spec)
}

impl JobRequest {
    pub fn spec(&self) -> Result<ExtractionSpec, ApiError> {
        build_spec(&self.query, &self.fields, self.deduplicate, self.validate, self.keep_invalid, self.include_source)
    }
}

impl ExtractRequest {
    pub fn spec(&self) -> Result<ExtractionSpec, ApiError> {
        build_spec(&self.query, &self.fields, self.deduplicate, self.validate, self.keep_invalid, self.include_source)
    }
}
