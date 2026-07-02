// Copyright 2026 Salesforce, Inc. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A2A Agent Card Skill Governor policy for MuleSoft Omni Gateway.
//!
//! Reshapes the `skills[]` array of an A2A Agent Card in-flight, per caller
//! identity (extended card) or globally (public card).

mod a2a;
mod generated;
mod governor;

use anyhow::{anyhow, Result};
use pdk::hl::*;

use crate::generated::config::Config;

async fn request_filter(_request_state: RequestState) -> Flow<()> {
    Flow::Continue(())
}

async fn response_filter(_response_state: ResponseState, _request_data: RequestData<()>) {}

#[entrypoint]
async fn configure(launcher: Launcher, Configuration(bytes): Configuration) -> Result<()> {
    let _config: Config = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow!("Failed to parse configuration: {}", e))?;
    let filter = on_request(|rs| request_filter(rs)).on_response(|rs, rd| response_filter(rs, rd));
    launcher.launch(filter).await?;
    Ok(())
}
