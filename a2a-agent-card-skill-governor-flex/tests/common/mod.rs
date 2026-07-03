// Copyright 2026 Salesforce, Inc. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Common test utilities shared across the integration test binary.

#![allow(dead_code)]

pub mod setup;

// Directory where the policy implementation wasm + GCL are placed by `make build`.
pub const POLICY_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/target/wasm32-wasip1/release");

// Directory containing logging.yaml and (locally generated) registration.yaml.
pub const COMMON_CONFIG_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/config");

// Policy implementation reference name. Version 0.2.0 → `...-v0-2`.
// Override after a version bump (read it from `target/policy-ref-name.txt`
// after `make build`).
pub const POLICY_NAME: &str = "a-two-a-agent-card-skill-governor-flex-v0-2";
