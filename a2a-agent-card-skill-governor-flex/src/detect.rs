// Copyright 2026 Salesforce, Inc. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Card-surface / A2A-variant classification. Pure — no PDK deps.

use crate::a2a::{EXT_CARD_HTTPJSON_PATH, EXT_CARD_LEGACY, EXT_CARD_V1, WELL_KNOWN_PATH};
use crate::governor::Surface;

/// Which wire binding produced the card-fetch request. Drives how the response
/// filter (Task 6) locates and rewrites the card payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Variant {
    /// Legacy JSON-RPC `agent/getAuthenticatedExtendedCard` (also the fallback
    /// when the V1 method name is seen without an `A2A-Version: 1.0` signal).
    Legacy,
    /// V1 JSON-RPC `GetExtendedAgentCard`.
    V1JsonRpc,
    /// V1 HTTP+JSON binding — `GET /extendedAgentCard`.
    V1HttpJson,
    /// Unauthenticated public well-known card — `GET /.well-known/agent-card.json`.
    PublicGet,
}

/// Strip a query string, returning just the path portion.
fn path_no_query(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

/// Returns `(surface, variant)` for a card-fetch request, or `None` if the
/// request is not an Agent Card fetch (the response filter then passes through).
///
/// Pure over its inputs so it is exhaustively unit-testable without a gateway:
/// - `method`     — HTTP method (`"GET"`, `"POST"`, ...).
/// - `path`       — request path (query string tolerated; stripped here).
/// - `is_v1`      — whether `A2A-Version: 1.0` was signalled (header or query).
/// - `jsonrpc_method` — the JSON-RPC `method` string when the body is a
///   JSON-RPC envelope, else `None`.
pub fn classify_surface(
    method: &str,
    path: &str,
    is_v1: bool,
    jsonrpc_method: Option<&str>,
) -> Option<(Surface, Variant)> {
    let p = path_no_query(path);

    // Public well-known card — unauthenticated GET.
    if method == "GET" && p.ends_with(WELL_KNOWN_PATH) {
        return Some((Surface::Public, Variant::PublicGet));
    }

    // Extended card via HTTP+JSON binding — GET /extendedAgentCard (V1 only).
    if method == "GET" && is_v1 && p.ends_with(EXT_CARD_HTTPJSON_PATH) {
        return Some((Surface::Extended, Variant::V1HttpJson));
    }

    // Extended card via JSON-RPC — the method string names it.
    match jsonrpc_method {
        Some(EXT_CARD_LEGACY) => Some((Surface::Extended, Variant::Legacy)),
        Some(EXT_CARD_V1) => Some((
            Surface::Extended,
            if is_v1 {
                Variant::V1JsonRpc
            } else {
                Variant::Legacy
            },
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_get_is_public() {
        let c = classify_surface("GET", "/.well-known/agent-card.json", false, None).unwrap();
        assert!(matches!(c, (Surface::Public, Variant::PublicGet)));
    }
    #[test]
    fn legacy_rpc_ext_card_is_extended() {
        let c = classify_surface(
            "POST",
            "/",
            false,
            Some("agent/getAuthenticatedExtendedCard"),
        )
        .unwrap();
        assert!(matches!(c, (Surface::Extended, Variant::Legacy)));
    }
    #[test]
    fn v1_rpc_ext_card_is_extended() {
        let c = classify_surface("POST", "/", true, Some("GetExtendedAgentCard")).unwrap();
        assert!(matches!(c, (Surface::Extended, Variant::V1JsonRpc)));
    }
    #[test]
    fn v1_httpjson_ext_card_is_extended() {
        let c = classify_surface("GET", "/extendedAgentCard", true, None).unwrap();
        assert!(matches!(c, (Surface::Extended, Variant::V1HttpJson)));
    }
    #[test]
    fn unrelated_request_is_skip() {
        assert!(classify_surface("POST", "/", false, Some("message/send")).is_none());
        assert!(classify_surface("GET", "/health", false, None).is_none());
    }
}
