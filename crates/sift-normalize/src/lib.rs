//! Validation, normalization and deduplication stages, plus URL normalisation.

mod dedupe;
mod normalize;
mod urls;
mod validate;

pub use dedupe::dedupe;
pub use normalize::{collapse_ws, normalize, normalize_phone, normalize_value};
pub use urls::{display_url, host_key, normalize_url, parse_lenient, url_key};
pub use validate::{is_valid_email, is_valid_phone, phone_digits, validate, validate_all};
