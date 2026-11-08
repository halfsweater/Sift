//! Core types for Sift: the vocabulary of extractable fields, the extraction
//! specification, the shared data model and output renderers.

pub mod field;
pub mod model;
pub mod output;
pub mod spec;

pub use field::{FieldGroup, FieldKind, UnknownField, SOCIALS};
pub use model::{Extracted, Metadata, Method, Record, ResultSet, Validation};
pub use output::{OutputFormat, Pager};
pub use spec::{ExtractionSpec, RequestInterpreter, RuleBasedInterpreter, DEFAULT_FIELDS};
