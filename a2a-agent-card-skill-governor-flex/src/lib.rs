// Copyright 2026 Salesforce, Inc. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A2A Agent Card Skill Governor policy for MuleSoft Omni Gateway.
//!
//! Reshapes the `skills[]` array of an A2A Agent Card in-flight, per caller
//! identity (extended card) or globally (public card).
//!
//! The request filter classifies the incoming request into a card surface and
//! A2A variant (see [`detect`]), reads caller identity for the extended
//! surface, and threads a [`CardContext`] forward to the response filter via
//! `RequestData`. It never blocks — non-card requests continue with `None`.

mod a2a;
mod detect;
mod generated;
mod governor;

use anyhow::{anyhow, Result};
use pdk::authentication::{Authentication, AuthenticationHandler};
use pdk::hl::*;
use pdk::logger;
use pdk::script::PayloadBinding;
use serde::Deserialize;
use serde_json::Value;

use crate::a2a::{is_agent_card, Skill};
use crate::detect::{classify_surface, Variant};
use crate::generated::config::Config;
use crate::governor::{GovernorRules, Identity, Surface};

const POST_METHOD: &str = "POST";
const CONTENT_TYPE_HEADER: &str = "content-type";
const A2A_VERSION_HEADER: &str = "A2A-Version";
const A2A_VERSION_QUERY_KEY: &str = "a2a-version";
const A2A_VERSION_V1: &str = "1.0";
const PATH_PSEUDO_HEADER: &str = ":path";
const CONTENT_LENGTH_HEADER: &str = "content-length";
/// JSON-RPC internal-error code (spec §7): fail-closed shaping errors on the
/// JSON-RPC bindings surface here, at HTTP 200 (JSON-RPC in-band convention).
const JSONRPC_INTERNAL_ERROR: i32 = -32603;

/// Everything the response filter needs to reshape the card: which surface it
/// is (public vs extended), which wire binding produced it, and — for the
/// extended surface — who the caller is. Derives `Clone` because it is threaded
/// through `Flow::Continue(Option<CardContext>)` and read back as
/// `RequestData<Option<CardContext>>`.
#[derive(Clone)]
pub struct CardContext {
    pub surface: Surface,
    pub variant: Variant,
    pub identity: Identity,
}

/// Minimal JSON-RPC envelope — only the `method` field is needed to name the
/// A2A operation. Borrowed to avoid copying the body.
#[derive(Debug, Deserialize)]
struct JsonRpcMethodOnly<'a> {
    #[serde(borrow)]
    method: Option<&'a str>,
}

/// True when the request signals A2A protocol v1.0 via the `A2A-Version: 1.0`
/// header (case-insensitive value) or the `?A2A-Version=1.0` query parameter
/// (case-insensitive key). Header name matching is delegated to the gateway's
/// case-insensitive header lookup.
fn a2a_version_is_v1(header_handler: &dyn HeadersHandler, path: &str) -> bool {
    if header_handler
        .header(A2A_VERSION_HEADER)
        .map(|v| v.trim() == A2A_VERSION_V1)
        .unwrap_or(false)
    {
        return true;
    }
    path.split_once('?')
        .map(|(_, query)| {
            query.split('&').any(|kv| {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                k.eq_ignore_ascii_case(A2A_VERSION_QUERY_KEY) && v == A2A_VERSION_V1
            })
        })
        .unwrap_or(false)
}

/// Parse just the JSON-RPC `method` field from a request body. Returns `None`
/// when the body is not a JSON object with a string `method` — the caller then
/// treats the request as non-JSON-RPC (fail-open).
fn read_jsonrpc_method(body: &[u8]) -> Option<String> {
    let parsed: JsonRpcMethodOnly<'_> = serde_json::from_slice(body).ok()?;
    parsed.method.map(str::to_owned)
}

/// Split a scope string into individual scopes on whitespace and commas,
/// dropping empties. Handles the common OAuth `scope` claim shapes
/// (`"a b c"`, `"a,b,c"`, mixed).
fn split_scopes(raw: &str) -> Vec<String> {
    raw.split([' ', '\t', '\n', '\r', ','])
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Build a caller [`Identity`] from the `Authentication` injectable. Reads
/// `client_id` / `client_name` / `principal`, the scope custom property under
/// `scope_claim_key`, and a `tier` custom property when present.
///
/// Sourced entirely from authentication data set by an upstream auth policy;
/// missing fields degrade to `None` / empty rather than failing.
fn read_identity(auth: &Authentication, scope_claim_key: &str) -> Identity {
    let data = match auth.authentication() {
        Some(d) => d,
        None => return Identity::default(),
    };

    // `properties` is a `pdk::script::Value` (not `serde_json::Value`).
    let props = data.properties.as_object();

    // Scope: string, or array of strings, under the operator-configured key.
    let scopes = props
        .and_then(|p| p.get(scope_claim_key))
        .map(|v| {
            if let Some(s) = v.as_str() {
                split_scopes(s)
            } else if let Some(items) = v.as_slice() {
                items
                    .iter()
                    .filter_map(|i| i.as_str())
                    .flat_map(split_scopes)
                    .collect()
            } else {
                Vec::new()
            }
        })
        .unwrap_or_default();

    // Tier: read from the auth custom properties (SLA tier is conventionally
    // propagated here by the contract/SLA policy). Accept a small set of common
    // key spellings.
    let tier = props.and_then(|p| {
        ["tier", "sla_tier", "slaTier"]
            .iter()
            .find_map(|k| p.get(*k).and_then(|v| v.as_str()).map(str::to_owned))
    });

    Identity {
        client_id: data.client_id,
        client_name: data.client_name,
        tier,
        scopes,
    }
}

/// Request filter: classify the request into a card surface/variant, read
/// identity for the extended surface, and thread a [`CardContext`] to the
/// response side. Never blocks — always `Flow::Continue`.
async fn request_filter(
    request_state: RequestState,
    auth: Authentication,
    _rules: &GovernorRules,
    scope_claim_key: &str,
) -> Flow<Option<CardContext>> {
    let headers_state = request_state.into_headers_state().await;
    let handler = headers_state.handler();

    let method = headers_state.method().as_str().to_owned();
    let path = handler.header(PATH_PSEUDO_HEADER).unwrap_or_default();
    let is_v1 = a2a_version_is_v1(handler, &path);

    // Peek the JSON-RPC method only for POST requests carrying a JSON body.
    let jsonrpc_method = if method == POST_METHOD
        && handler
            .header(CONTENT_TYPE_HEADER)
            .map(|c| c.contains("json"))
            .unwrap_or(false)
    {
        let body_state = headers_state.into_body_state().await;
        let body_bytes = body_state.as_bytes();
        read_jsonrpc_method(body_bytes.as_slice())
    } else {
        None
    };

    let Some((surface, variant)) =
        classify_surface(&method, &path, is_v1, jsonrpc_method.as_deref())
    else {
        // Not a card fetch → response filter passes through.
        return Flow::Continue(None);
    };

    // Identity is only meaningful on the extended surface; identity-scoped
    // rules no-op on the public card anyway.
    let identity = if surface == Surface::Extended {
        read_identity(&auth, scope_claim_key)
    } else {
        Identity::default()
    };

    Flow::Continue(Some(CardContext {
        surface,
        variant,
        identity,
    }))
}

/// Pure card transform. Locates the AgentCard in a response body, governs its
/// `skills[]`, and returns the reshaped body.
///
/// Return contract:
/// - `Ok(None)`       — pass the response through untouched. Covers non-JSON
///   bodies, JSON-RPC error envelopes, and confirmed non-card payloads.
/// - `Ok(Some(bytes))` — the governed body to write back.
/// - `Err(_)`         — a failure *after* the body was confirmed to be a card
///   (missing `skills[]`, unparseable skills, serialize failure). The caller
///   MUST fail closed rather than ship an ungoverned card.
///
/// Pure over its inputs (no PDK I/O), so it is exhaustively unit-testable
/// without a gateway harness. All gateway I/O lives in [`response_filter`].
fn transform_card_body(
    body: &[u8],
    variant: &Variant,
    rules: &GovernorRules,
    surface: Surface,
    id: &Identity,
    warn: &mut dyn FnMut(String),
) -> Result<Option<Vec<u8>>> {
    let mut root: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return Ok(None), // not JSON → not our target
    };

    // JSON-RPC bindings wrap the card in `result`; an `error` envelope means the
    // upstream already failed — pass it through untouched. HTTP+JSON / public
    // bindings carry the card as the top-level object.
    let is_rpc = matches!(variant, Variant::Legacy | Variant::V1JsonRpc);

    // Decide, on an immutable borrow, whether this response even holds a card.
    {
        let card_ref = if is_rpc {
            if root.get("error").is_some() {
                return Ok(None); // upstream JSON-RPC error → pass through
            }
            match root.get("result") {
                Some(r) => r,
                None => return Ok(None), // no result to shape
            }
        } else {
            &root
        };
        if !is_agent_card(card_ref) {
            return Ok(None); // confirmed non-card → pass through
        }
    }

    // From here the body IS a card: any failure is fail-closed (Err).
    // Take a mutable handle to the card object without `unwrap()`.
    let card = if is_rpc {
        root.get_mut("result")
            .ok_or_else(|| anyhow!("card result vanished after detection"))?
    } else {
        &mut root
    };

    let skills_val = card
        .get_mut("skills")
        .ok_or_else(|| anyhow!("card lost skills[]"))?;
    let skills: Vec<Skill> = serde_json::from_value(skills_val.take())
        .map_err(|e| anyhow!("skills[] not a skill array: {}", e))?;
    let governed = rules.govern(skills, surface, id, warn);
    *skills_val =
        serde_json::to_value(&governed).map_err(|e| anyhow!("serialize skills: {}", e))?;

    let out = serde_json::to_vec(&root).map_err(|e| anyhow!("serialize card: {}", e))?;
    Ok(Some(out))
}

/// Build the surface-appropriate fail-closed error BODY (spec §7).
///
/// This is a body-only contract: the HTTP status is deliberately NOT returned
/// and NOT rewritten. On the split headers→body flow the `:status` header is
/// committed in the headers phase, *before* the body is read — but the
/// fail-closed decision can only be made after reading the body and confirming
/// a card failed to shape. A body-content-dependent status change is therefore
/// impossible on the flow the runtime actually supports (the combined
/// headers+body state that would allow it hangs on the response leg on
/// Flex 1.12.1). The security property holds regardless: the card body is
/// replaced with an error envelope, so ungoverned skills are never leaked. Only
/// status-code fidelity for the two bare-HTTP surfaces is given up; the upstream
/// status (normally 200) is preserved.
///
/// Body shapes:
/// - JSON-RPC bindings (`Legacy`/`V1JsonRpc`): a JSON-RPC `-32603 INTERNAL`
///   envelope (HTTP 200 was always the JSON-RPC in-band convention).
/// - `V1HttpJson`: a `google.rpc.Status`-shaped error body. The `error.code: 500`
///   is a payload field, not the HTTP status.
/// - `PublicGet`: a plain JSON error body.
///
/// Never blocks on serialization: falls back to a static minimal body if the
/// (fixed, tiny) error object somehow fails to serialize, so the fail-closed
/// path itself cannot panic.
fn fail_closed_body(variant: &Variant) -> Vec<u8> {
    const MESSAGE: &str = "Agent Card could not be governed";
    match variant {
        Variant::Legacy | Variant::V1JsonRpc => {
            let body = serde_json::json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": { "code": JSONRPC_INTERNAL_ERROR, "message": MESSAGE }
            });
            serde_json::to_vec(&body).unwrap_or_else(|_| {
                br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"Agent Card could not be governed"}}"#.to_vec()
            })
        }
        Variant::V1HttpJson => {
            let body = serde_json::json!({
                "error": {
                    "code": 500,
                    "message": MESSAGE,
                    "details": [{
                        "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                        "reason": "INTERNAL",
                        "domain": "a2a-protocol.org"
                    }]
                }
            });
            serde_json::to_vec(&body).unwrap_or_else(|_| {
                br#"{"error":{"code":500,"message":"Agent Card could not be governed"}}"#.to_vec()
            })
        }
        Variant::PublicGet => {
            let body = serde_json::json!({ "error": MESSAGE });
            serde_json::to_vec(&body).unwrap_or_else(|_| {
                br#"{"error":"Agent Card could not be governed"}"#.to_vec()
            })
        }
    }
}

/// Write `body` to the response body handler, logging (never panicking) on
/// failure. Shared by the governed-body and fail-closed paths — both simply
/// replace the body. The stale upstream `content-length` is dropped earlier, in
/// the headers phase, so the gateway recomputes it from the new bytes (the
/// sanctioned PDK pattern — pdk-request-headers-bodies: "Remove `content-length`
/// before modifying body"). On a body-less flow `set_body` returns `BodyNotSent`;
/// the card fetch always has a body, but we log and give up rather than panic if
/// not.
fn set_body_or_log(handler: &dyn BodyHandler, body: &[u8], what: &str) {
    if let Err(e) = handler.set_body(body) {
        logger::error!("[skill-governor] failed to write {}: {}", what, e);
    }
}

/// Response filter: when the request filter flagged this as a card fetch,
/// reshape the card's `skills[]` and write it back. Fails closed — replacing the
/// card *body* with a surface-appropriate error envelope — if a confirmed card
/// cannot be shaped.
///
/// Uses the split headers→body flow (mirrors the sibling `mcp-apps` / `rest-to-a2a`
/// response filters): the header mutation (`content-length` removal) happens in
/// the headers phase, then the body is read and rewritten in the body phase. The
/// combined headers+body state is deliberately NOT used on the response leg — it
/// hangs on Flex 1.12.1, yielding an Envoy 504 on every response-transforming
/// request.
///
/// Consequence for fail-closed: the HTTP `:status` is committed in the headers
/// phase, before the body is read, so it is left as whatever the upstream sent
/// (normally 200). Only the body is replaced — see [`fail_closed_body`]. The
/// security property (never leak ungoverned skills) is preserved because the
/// card body is always overwritten on failure.
async fn response_filter(
    response: ResponseHeadersState,
    request_data: RequestData<Option<CardContext>>,
    rules: &GovernorRules,
) {
    let ctx = match request_data {
        RequestData::Continue(Some(ctx)) => ctx,
        _ => return, // not a card fetch → pass through
    };

    if !response.contains_body() {
        return; // no body to shape → pass through
    }

    // Header mutations must happen in the headers phase. Drop the stale
    // content-length now, whether or not we end up rewriting the body — if we
    // pass through untouched the body is byte-identical, and the gateway
    // recomputing an unchanged length is harmless.
    response.handler().remove_header(CONTENT_LENGTH_HEADER);

    let body_state = response.into_body_state().await;
    let handler = body_state.handler();
    let body = handler.body();

    let mut warned = |m: String| logger::warn!("[skill-governor] {}", m);
    match transform_card_body(&body, &ctx.variant, rules, ctx.surface, &ctx.identity, &mut warned) {
        Ok(None) => {} // pass through untouched
        Ok(Some(new_body)) => set_body_or_log(handler, &new_body, "governed body"),
        Err(e) => {
            logger::error!("[skill-governor] failing closed on card shaping: {}", e);
            // Body-only fail-closed: replace the card body with the error
            // envelope; leave :status as the upstream committed it. A set_body
            // failure here would leave the ungoverned upstream card (a leak),
            // but that is unreachable: BodyNotSent is excluded by the
            // contains_body() guard above, and the envelope is a few hundred
            // bytes so ExceededBodySize cannot fire.
            set_body_or_log(handler, &fail_closed_body(&ctx.variant), "fail-closed body");
        }
    }
}

#[entrypoint]
async fn configure(launcher: Launcher, Configuration(bytes): Configuration) -> Result<()> {
    let config: Config = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow!("Failed to parse configuration: {}", e))?;

    let scope_claim_key = config
        .scope_claim_key
        .clone()
        .unwrap_or_else(|| "scope".to_owned());

    // Compile the rule set once at configuration time; warnings surface at load.
    let rules = GovernorRules::compile(&config, &mut |m| {
        logger::warn!("[skill-governor] {}", m)
    });

    // `Authentication` is a filter-context injectable (it implements
    // `FromContext<FilterContext>`, not `ConfigureContext`), so it is injected
    // into the request closure rather than the entrypoint.
    let filter = on_request(|rs, auth| request_filter(rs, auth, &rules, &scope_claim_key))
        .on_response(|rs, rd| response_filter(rs, rd, &rules));
    launcher.launch(filter).await?;
    Ok(())
}

#[cfg(test)]
mod resp_tests {
    use super::*;
    use crate::detect::Variant;
    use crate::generated::config::*;
    use crate::governor::{GovernorRules, Identity, Surface};

    fn deny_all_rules() -> GovernorRules {
        let c = Config {
            scope_claim_key: None,
            default_allow: Some(false),
            visibility: Some(vec![]),
            skills: Some(vec![]),
        };
        GovernorRules::compile(&c, &mut |_| {})
    }

    #[test]
    fn public_card_body_skills_removed() {
        let body = br#"{"name":"A","skills":[{"id":"a","name":"A"}]}"#;
        let out = transform_card_body(
            body,
            &Variant::PublicGet,
            &deny_all_rules(),
            Surface::Public,
            &Identity::default(),
            &mut |_| {},
        )
        .unwrap()
        .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["skills"].as_array().unwrap().len(), 0);
        assert_eq!(v["name"], "A"); // untouched
    }

    #[test]
    fn jsonrpc_error_passes_through() {
        let body = br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"x"}}"#;
        let r = transform_card_body(
            body,
            &Variant::V1JsonRpc,
            &deny_all_rules(),
            Surface::Extended,
            &Identity::default(),
            &mut |_| {},
        );
        assert!(matches!(r, Ok(None))); // None => pass through untouched
    }

    #[test]
    fn non_card_passes_through() {
        let body = br#"{"jsonrpc":"2.0","id":1,"result":{"foo":"bar"}}"#;
        let r = transform_card_body(
            body,
            &Variant::V1JsonRpc,
            &deny_all_rules(),
            Surface::Extended,
            &Identity::default(),
            &mut |_| {},
        );
        assert!(matches!(r, Ok(None)));
    }

    #[test]
    fn jsonrpc_result_card_governed_and_rewrapped() {
        let body = br#"{"jsonrpc":"2.0","id":7,"result":{"name":"A","skills":[{"id":"a"}]}}"#;
        let out = transform_card_body(
            body,
            &Variant::V1JsonRpc,
            &deny_all_rules(),
            Surface::Extended,
            &Identity::default(),
            &mut |_| {},
        )
        .unwrap()
        .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["id"], 7); // envelope preserved
        assert_eq!(v["result"]["skills"].as_array().unwrap().len(), 0);
    }
}
