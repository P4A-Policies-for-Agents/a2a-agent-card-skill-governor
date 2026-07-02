// Copyright 2026 Salesforce, Inc. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Test setup helpers: build a Flex+httpmock TestComposite with a configurable
//! policy config, plus helpers to mock the upstream A2A backend returning an
//! AgentCard on the three card surfaces, and to build request bodies/cards.

use httpmock::MockServer;
use pdk_test::port::Port;
use pdk_test::services::flex::{ApiConfig, Flex, FlexConfig, PolicyConfig};
use pdk_test::services::httpmock::{HttpMock, HttpMockConfig};
use pdk_test::TestComposite;
use serde_json::{json, Value};

use super::{COMMON_CONFIG_DIR, POLICY_DIR, POLICY_NAME};

pub const FLEX_PORT: Port = 8081;

/// A2A card-surface paths (mirror the constants in `src/a2a.rs`).
pub const WELL_KNOWN_PATH: &str = "/.well-known/agent-card.json";
pub const EXT_CARD_HTTPJSON_PATH: &str = "/extendedAgentCard";
pub const EXT_CARD_V1_METHOD: &str = "GetExtendedAgentCard";
pub const EXT_CARD_LEGACY_METHOD: &str = "agent/getAuthenticatedExtendedCard";

/// Build a `PolicyConfig` from a JSON value carrying the gcl.yaml-shaped
/// configuration. The caller fully controls the schema (scopeClaimKey,
/// defaultAllow, visibility[], skills[]).
pub fn build_policy_config(config: Value) -> PolicyConfig {
    PolicyConfig::builder()
        .name(POLICY_NAME)
        .configuration(config)
        .build()
}

/// Spin up a Flex Gateway with the given policy configuration plus an httpmock
/// acting as the upstream A2A agent. Returns the composite (keep it alive), the
/// public Flex URL to target, and a connected `MockServer` for expectations.
pub async fn setup_test(
    policy_config: Value,
) -> anyhow::Result<(TestComposite, String, MockServer)> {
    let httpmock_config = HttpMockConfig::builder()
        .port(80)
        .version("latest")
        .hostname("backend")
        .build();

    let policy_config = build_policy_config(policy_config);

    // This is an OUTBOUND-injection policy (metadata/capabilities/injectionPoint:
    // outbound). It must be attached via `.outbound_policies(...)`, NOT
    // `.policies(...)` — an outbound policy cannot bind to the inbound slot of an
    // ApiInstance (the gateway rejects it: "target ... is invalid to extension
    // ... because it is of kind ApiInstance"). This is the key deviation from the
    // inbound pii-guard template, whose policy binds on `.policies(...)`.
    let api_config = ApiConfig::builder()
        .name("a2aApi")
        .upstream(&httpmock_config)
        .path("/")
        .port(FLEX_PORT)
        .outbound_policies([policy_config])
        .build();

    let flex_config = FlexConfig::builder()
        .version("1.12.1")
        .hostname("local-flex")
        .with_api(api_config)
        .config_mounts([(POLICY_DIR, "policy"), (COMMON_CONFIG_DIR, "common")])
        .build();

    let composite = TestComposite::builder()
        .with_service(flex_config)
        .with_service(httpmock_config)
        .build()
        .await?;

    let flex: Flex = composite.service()?;
    let flex_url = flex.external_url(FLEX_PORT).unwrap();

    let httpmock: HttpMock = composite.service()?;
    let mock_server = MockServer::connect_async(httpmock.socket()).await;

    Ok((composite, flex_url, mock_server))
}

// ---------------------------------------------------------------------------
// Card / body builders
// ---------------------------------------------------------------------------

/// Build a bare AgentCard with the given skill objects. Skills are passed as
/// full JSON `Value`s so tests can craft malformed entries (fail-closed cases).
pub fn agent_card(skills: Vec<Value>) -> Value {
    json!({
        "name": "Test Agent",
        "description": "An agent for integration testing",
        "url": "http://backend/",
        "version": "1.0.0",
        "capabilities": {},
        "defaultInputModes": ["text"],
        "defaultOutputModes": ["text"],
        "skills": skills,
    })
}

/// A single skill object with just id + name (enough to survive round-trip).
pub fn skill(id: &str, name: &str) -> Value {
    json!({ "id": id, "name": name, "description": format!("{name} description") })
}

/// The canonical 3-skill card used across several tests: `search`, `secret`,
/// `report`.
pub fn three_skill_card() -> Value {
    agent_card(vec![
        skill("search", "Search"),
        skill("secret", "Secret Op"),
        skill("report", "Report"),
    ])
}

/// Wrap a bare card in a JSON-RPC 2.0 success `result` envelope with `id`.
pub fn jsonrpc_result(id: Value, card: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": card })
}

// ---------------------------------------------------------------------------
// Upstream mocks — one per card surface
// ---------------------------------------------------------------------------

/// Mock the upstream to serve `card` (a bare AgentCard) on
/// `GET /.well-known/agent-card.json`.
pub async fn mock_well_known(mock: &MockServer, card: Value) {
    let body = card.to_string();
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::GET)
            .path_contains("/.well-known/agent-card.json");
        then.status(200)
            .header("content-type", "application/json")
            .body(body);
    })
    .await;
}

/// Mock the upstream to serve `card` (a bare AgentCard) on
/// `GET /extendedAgentCard`.
pub async fn mock_ext_httpjson(mock: &MockServer, card: Value) {
    let body = card.to_string();
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::GET)
            .path_contains("/extendedAgentCard");
        then.status(200)
            .header("content-type", "application/json")
            .body(body);
    })
    .await;
}

/// Mock the upstream to serve a JSON-RPC 2.0 `result`-wrapped card on any POST
/// (the extended JSON-RPC card fetch). `card` is the bare card placed in
/// `result`; `id` is echoed in the envelope.
pub async fn mock_ext_jsonrpc(mock: &MockServer, id: Value, card: Value) {
    let body = jsonrpc_result(id, card).to_string();
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST);
        then.status(200)
            .header("content-type", "application/json")
            .body(body);
    })
    .await;
}

/// Mock the upstream to serve an arbitrary JSON-RPC 2.0 body on any POST — used
/// for the non-card passthrough case (`message/send` result).
pub async fn mock_jsonrpc_body(mock: &MockServer, body: Value) {
    let body = body.to_string();
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST);
        then.status(200)
            .header("content-type", "application/json")
            .body(body);
    })
    .await;
}

// ---------------------------------------------------------------------------
// Client requests — one per card surface
// ---------------------------------------------------------------------------

/// GET the public well-known card through the Flex.
pub async fn get_well_known(flex_url: &str) -> anyhow::Result<reqwest::Response> {
    Ok(reqwest::get(format!("{flex_url}{WELL_KNOWN_PATH}")).await?)
}

/// GET the extended HTTP+JSON card through the Flex, signalling `A2A-Version: 1.0`.
pub async fn get_ext_httpjson(flex_url: &str) -> anyhow::Result<reqwest::Response> {
    Ok(reqwest::Client::new()
        .get(format!("{flex_url}{EXT_CARD_HTTPJSON_PATH}"))
        .header("A2A-Version", "1.0")
        .send()
        .await?)
}

/// POST a `GetExtendedAgentCard` JSON-RPC request with `A2A-Version: 1.0`.
pub async fn post_ext_jsonrpc(
    flex_url: &str,
    id: Value,
) -> anyhow::Result<reqwest::Response> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": EXT_CARD_V1_METHOD,
    });
    Ok(reqwest::Client::new()
        .post(format!("{flex_url}/"))
        .header("content-type", "application/json")
        .header("A2A-Version", "1.0")
        .body(body.to_string())
        .send()
        .await?)
}

/// POST an arbitrary JSON-RPC request (used for `message/send` passthrough).
pub async fn post_jsonrpc(
    flex_url: &str,
    body: Value,
) -> anyhow::Result<reqwest::Response> {
    Ok(reqwest::Client::new()
        .post(format!("{flex_url}/"))
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await?)
}
