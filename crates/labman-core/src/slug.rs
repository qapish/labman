//! Slug encoding helpers for tenant/endpoint/model identifiers.
//!
//! This module provides a small, well-defined helper for generating opaque
//! model slugs that can be used as the `model` field in OpenAI-compatible
//! requests when routing decisions are delegated to an external control plane.
//!
//! The intended usage is:
//!
//! - For each discovered `(tenant, endpoint_slug, model_id)` triple on a
//!   labman node, compute a stable slug via `encode_model_slug`.
//! - Expose these slugs (alongside the underlying triples) to the control
//!   plane so it can:
//!     - Attribute usage and compensation per tenant/endpoint/model.
//!     - Schedule work by selecting a specific slug and sending it back as the
//!       `model` field in OpenAI-compatible calls.
//! - On inbound requests, labman treats the `model` field as an opaque slug
//!   and resolves it back to `(tenant, endpoint_name, model_id)` via a
//!   registry mapping.
//!
//! The exact scheme implemented here is:
//!
//! ```text
//! slug_input = tenant + "\n" + endpoint_slug + "\n" + model_id
//! slug_hash  = SHA-256(slug_input)
//! slug_bytes = first 8 bytes of slug_hash
//! slug       = base62(slug_bytes)
//! ```
//!
//! This yields a reasonably short, collision-resistant identifier that is:
//!
//! - Stable for a given (tenant, endpoint_slug, model_id) triple.
//! - Opaque to clients (no direct leakage of the underlying strings).
//! - Easy for both the control plane and labman to reproduce.
//!
//! Note: this is not intended as a security primitive. It is a convenient
//! identifier for scheduling and accounting logic in a distributed, partially
//! trustless network.

use sha2::{Digest, Sha256};

/// Encode a `(tenant, endpoint_slug, model_id)` triple into an opaque model
/// slug suitable for use as the OpenAI `model` field.
///
/// - `tenant`:
///     - Logical tenant identifier as seen by the control plane.
///     - Use `""` (empty string) for the operator's default tenant.
/// - `endpoint_slug`:
///     - Schema-stripped endpoint identifier, e.g.:
///       - `"10.6.0.213:11434/v1"` derived from
///         `"http://10.6.0.213:11434/v1"`.
/// - `model_id`:
///     - Concrete model identifier on the endpoint, e.g. `"mistral-nemo:12b"`.
///
/// The returned slug is stable and URL-safe, and can be used as an opaque
/// routing key by the control plane.
pub fn encode_model_slug(tenant: &str, endpoint_slug: &str, model_id: &str) -> String {
    // Construct the canonical input string.
    let mut input =
        String::with_capacity(tenant.len() + 1 + endpoint_slug.len() + 1 + model_id.len());
    input.push_str(tenant);
    input.push('\n');
    input.push_str(endpoint_slug);
    input.push('\n');
    input.push_str(model_id);

    // Compute SHA-256 hash of the concatenated input.
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();

    // Take the first 8 bytes to keep the slug short while still providing
    // sufficient collision resistance for realistic deployments.
    let prefix = &digest[..8];

    // Interpret the prefix as a big-endian u64.
    let mut buf = [0u8; 8];
    buf.copy_from_slice(prefix);
    let mut value = u64::from_be_bytes(buf);

    // Base62-encode the u64 to get a compact, URL-safe slug.
    base62_encode_u64(value)
}

/// Base62-encode a u64 value.
///
/// This is sufficient for the 8-byte prefix of the SHA-256 hash used above and
/// keeps the slug short and URL-safe.
fn base62_encode_u64(mut value: u64) -> String {
    const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

    if value == 0 {
        return "0".to_string();
    }

    let mut chars = Vec::new();

    while value > 0 {
        let idx = (value % 62) as usize;
        value /= 62;
        chars.push(ALPHABET[idx] as char);
    }

    chars.iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_stable_for_same_input() {
        let s1 = encode_model_slug("tenantA", "10.6.0.213:11434/v1", "mistral-nemo:12b");
        let s2 = encode_model_slug("tenantA", "10.6.0.213:11434/v1", "mistral-nemo:12b");
        assert_eq!(s1, s2);
    }

    #[test]
    fn slug_changes_when_any_component_differs() {
        let base = encode_model_slug("tenantA", "10.6.0.213:11434/v1", "mistral-nemo:12b");

        let diff_tenant = encode_model_slug("tenantB", "10.6.0.213:11434/v1", "mistral-nemo:12b");
        let diff_endpoint = encode_model_slug("tenantA", "10.6.0.214:11434/v1", "mistral-nemo:12b");
        let diff_model = encode_model_slug("tenantA", "10.6.0.213:11434/v1", "llama3.1:70b");

        assert_ne!(base, diff_tenant);
        assert_ne!(base, diff_endpoint);
        assert_ne!(base, diff_model);
    }

    #[test]
    fn slug_is_reasonably_short() {
        let s = encode_model_slug("tenantA", "10.6.0.213:11434/v1", "mistral-nemo:12b");
        // 8 bytes in base-62 => at most 11 chars (ceil(log_62(2^64))).
        assert!(s.len() <= 11);
        assert!(!s.is_empty());
    }
}
