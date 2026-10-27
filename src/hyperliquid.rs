use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;

use crate::source::MarketSource;
use crate::types::{AccountSnapshot, Mids};

pub const MAINNET_URL: &str = "https://api.hyperliquid.xyz";
pub const TESTNET_URL: &str = "https://api.hyperliquid-testnet.xyz";

/// Read-only client for the public Hyperliquid `/info` endpoint.
pub struct HyperliquidClient {
    http: reqwest::blocking::Client,
    base_url: String,
}

impl HyperliquidClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
        })
    }

    fn info<T: for<'de> Deserialize<'de>>(&self, body: serde_json::Value) -> Result<T> {
        let url = format!("{}/info", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .with_context(|| format!("request to {url} failed"))?
            .error_for_status()?;
        Ok(resp.json()?)
    }
}

impl MarketSource for HyperliquidClient {
    fn target_snapshot(&mut self, wallet: &str) -> Result<AccountSnapshot> {
        let state: ClearinghouseState =
            self.info(json!({ "type": "clearinghouseState", "user": wallet }))?;
        parse_clearinghouse_state(state)
    }

    fn mids(&mut self) -> Result<Mids> {
        let raw: HashMap<String, String> = self.info(json!({ "type": "allMids" }))?;
        parse_mids(raw)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearinghouseState {
    pub margin_summary: MarginSummary,
    #[serde(default)]
    pub asset_positions: Vec<AssetPosition>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarginSummary {
    pub account_value: String,
}

#[derive(Debug, Deserialize)]
pub struct AssetPosition {
    pub position: RawPosition,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawPosition {
    pub coin: String,
    pub szi: String,
}

pub fn parse_clearinghouse_state(state: ClearinghouseState) -> Result<AccountSnapshot> {
    let account_value = state
        .margin_summary
        .account_value
        .parse()
        .context("invalid accountValue")?;
    let mut positions = HashMap::new();
    for ap in state.asset_positions {
        let size: f64 = ap
            .position
            .szi
            .parse()
            .with_context(|| format!("invalid szi for {}", ap.position.coin))?;
        if size != 0.0 {
            positions.insert(ap.position.coin, size);
        }
    }
    Ok(AccountSnapshot {
        account_value,
        positions,
    })
}

pub fn parse_mids(raw: HashMap<String, String>) -> Result<Mids> {
    raw.into_iter()
        // Spot pairs show up as "@123" / "PURR/USDC"; only perps are copied.
        .filter(|(coin, _)| !coin.starts_with('@') && !coin.contains('/'))
        .map(|(coin, px)| {
            let px = px
                .parse()
                .with_context(|| format!("invalid mid for {coin}"))?;
            Ok((coin, px))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clearinghouse_state() {
        let body = r#"{
            "marginSummary": {"accountValue": "12345.6", "totalNtlPos": "1", "totalRawUsd": "1", "totalMarginUsed": "1"},
            "crossMarginSummary": {"accountValue": "12345.6"},
            "withdrawable": "100.0",
            "assetPositions": [
                {"type": "oneWay", "position": {"coin": "BTC", "szi": "0.5", "entryPx": "60000", "leverage": {"type": "cross", "value": 10}}},
                {"type": "oneWay", "position": {"coin": "ETH", "szi": "-2.25", "entryPx": "3000"}},
                {"type": "oneWay", "position": {"coin": "SOL", "szi": "0.0"}}
            ],
            "time": 1700000000000
        }"#;
        let state: ClearinghouseState = serde_json::from_str(body).unwrap();
        let snap = parse_clearinghouse_state(state).unwrap();
        assert_eq!(snap.account_value, 12345.6);
        assert_eq!(snap.positions.len(), 2);
        assert_eq!(snap.positions["BTC"], 0.5);
        assert_eq!(snap.positions["ETH"], -2.25);
    }

    #[test]
    fn parses_mids_and_skips_spot() {
        let raw: HashMap<String, String> = serde_json::from_str(
            r#"{"BTC": "65000.5", "ETH": "3100", "@107": "1.2", "PURR/USDC": "0.1"}"#,
        )
        .unwrap();
        let mids = parse_mids(raw).unwrap();
        assert_eq!(mids.len(), 2);
        assert_eq!(mids["BTC"], 65000.5);
    }
}
