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

use crate::detect::{classify_surface, Variant};
use crate::generated::config::Config;
use crate::governor::{GovernorRules, Identity, Surface};

const POST_METHOD: &str = "POST";
const CONTENT_TYPE_HEADER: &str = "content-type";
const A2A_VERSION_HEADER: &str = "A2A-Version";
const A2A_VERSION_QUERY_KEY: &str = "a2a-version";
const A2A_VERSION_V1: &str = "1.0";
const PATH_PSEUDO_HEADER: &str = ":path";

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

/// Response filter stub — the card-reshaping body lands in Task 6. For now it
/// accepts the threaded [`CardContext`] and the compiled rules but passes the
/// response through unchanged.
async fn response_filter(
    _response_state: ResponseState,
    _request_data: RequestData<Option<CardContext>>,
    _rules: &GovernorRules,
) {
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
