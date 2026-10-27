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

impl HyperliquidClient {
    pub fn clearinghouse_state(&self, wallet: &str) -> Result<ClearinghouseState> {
        self.info(json!({ "type": "clearinghouseState", "user": wallet }))
    }

    /// Resting orders of the wallet, including TP/SL trigger orders.
    pub fn open_orders(&self, wallet: &str) -> Result<Vec<OpenOrder>> {
        self.info(json!({ "type": "frontendOpenOrders", "user": wallet }))
    }
}

impl MarketSource for HyperliquidClient {
    fn target_snapshot(&mut self, wallet: &str) -> Result<AccountSnapshot> {
        parse_clearinghouse_state(self.clearinghouse_state(wallet)?)
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
    pub entry_px: Option<String>,
    pub position_value: Option<String>,
    pub unrealized_pnl: Option<String>,
    pub liquidation_px: Option<String>,
    pub leverage: Option<Leverage>,
}

#[derive(Debug, Deserialize)]
pub struct Leverage {
    #[serde(rename = "type")]
    pub kind: String,
    pub value: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenOrder {
    pub coin: String,
    /// "B" = bid/buy, "A" = ask/sell.
    pub side: String,
    pub limit_px: String,
    pub sz: String,
    pub oid: u64,
    pub timestamp: u64,
    #[serde(default)]
    pub order_type: String,
    #[serde(default)]
    pub reduce_only: bool,
    #[serde(default)]
    pub is_trigger: bool,
    #[serde(default)]
    pub trigger_px: String,
    #[serde(default)]
    pub is_position_tpsl: bool,
}

/// Open position with the details shown by `watch` / `status`.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionInfo {
    pub coin: String,
    pub size: f64,
    pub entry_px: f64,
    pub position_value: f64,
    pub unrealized_pnl: f64,
    pub liquidation_px: Option<f64>,
    pub leverage: Option<String>,
}

fn num(v: &Option<String>) -> f64 {
    v.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0.0)
}

pub fn position_details(state: &ClearinghouseState) -> Vec<PositionInfo> {
    let mut out: Vec<PositionInfo> = state
        .asset_positions
        .iter()
        .map(|ap| &ap.position)
        .filter_map(|p| {
            let size: f64 = p.szi.parse().ok()?;
            (size != 0.0).then(|| PositionInfo {
                coin: p.coin.clone(),
                size,
                entry_px: num(&p.entry_px),
                position_value: num(&p.position_value),
                unrealized_pnl: num(&p.unrealized_pnl),
                liquidation_px: p.liquidation_px.as_deref().and_then(|s| s.parse().ok()),
                leverage: p
                    .leverage
                    .as_ref()
                    .map(|l| format!("{}x {}", l.value, l.kind)),
            })
        })
        .collect();
    out.sort_by(|a, b| b.position_value.total_cmp(&a.position_value));
    out
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
    fn parses_position_details_and_open_orders() {
        let body = r#"{
            "marginSummary": {"accountValue": "5000"},
            "assetPositions": [
                {"type": "oneWay", "position": {"coin": "ETH", "szi": "-2", "entryPx": "3000", "positionValue": "5800",
                 "unrealizedPnl": "200", "liquidationPx": null, "leverage": {"type": "isolated", "value": 3}}},
                {"type": "oneWay", "position": {"coin": "BTC", "szi": "0.1", "entryPx": "60000", "positionValue": "6200",
                 "unrealizedPnl": "200", "liquidationPx": "41000.5", "leverage": {"type": "cross", "value": 20}}}
            ]
        }"#;
        let state: ClearinghouseState = serde_json::from_str(body).unwrap();
        let pos = position_details(&state);
        assert_eq!(pos[0].coin, "BTC");
        assert_eq!(pos[0].liquidation_px, Some(41000.5));
        assert_eq!(pos[0].leverage.as_deref(), Some("20x cross"));
        assert_eq!(pos[1].size, -2.0);
        assert_eq!(pos[1].liquidation_px, None);

        let orders: Vec<OpenOrder> = serde_json::from_str(r#"[
            {"coin": "BTC", "side": "A", "limitPx": "70000", "sz": "0.1", "oid": 42, "timestamp": 1700000000000,
             "origSz": "0.1", "orderType": "Take Profit Market", "reduceOnly": true, "isTrigger": true,
             "triggerPx": "70000", "triggerCondition": "Price above 70000", "isPositionTpsl": true, "children": [], "tif": null, "cloid": null}
        ]"#).unwrap();
        assert_eq!(orders[0].order_type, "Take Profit Market");
        assert!(orders[0].reduce_only && orders[0].is_trigger);
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
