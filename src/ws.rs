//! Real-time feed of a wallet's trades and order changes via the Hyperliquid
//! WebSocket API (`userFills` + `orderUpdates` subscriptions).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use serde::Deserialize;
use serde_json::json;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Connector, Message, WebSocket};

use crate::types::Side;

const PING_EVERY: Duration = Duration::from_secs(30);
/// The server sends nothing on a quiet wallet, but always answers our pings.
const STALE_AFTER: Duration = Duration::from_secs(90);
const READ_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq)]
pub enum WalletEvent {
    Connected,
    Disconnected(String),
    Trade(TradeEvent),
    Order(OrderEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeKind {
    Open,
    Increase,
    Reduce,
    Close,
    Flip,
}

/// One order of the target, aggregated over all its partial fills in a message.
#[derive(Debug, Clone, PartialEq)]
pub struct TradeEvent {
    pub coin: String,
    pub side: Side,
    pub size: f64,
    pub avg_px: f64,
    pub start_position: f64,
    pub closed_pnl: f64,
    pub fee: f64,
    pub time_ms: u64,
    pub oid: u64,
    /// Hyperliquid's own label, e.g. "Open Long", "Close Short", "Long > Short".
    pub dir: String,
}

impl TradeEvent {
    pub fn signed_size(&self) -> f64 {
        match self.side {
            Side::Buy => self.size,
            Side::Sell => -self.size,
        }
    }

    pub fn new_position(&self) -> f64 {
        let p = self.start_position + self.signed_size();
        if p.abs() < 1e-9 { 0.0 } else { p }
    }

    pub fn kind(&self) -> TradeKind {
        let (start, end) = (self.start_position, self.new_position());
        if start == 0.0 {
            TradeKind::Open
        } else if end == 0.0 {
            TradeKind::Close
        } else if start.signum() != end.signum() {
            TradeKind::Flip
        } else if end.abs() > start.abs() {
            TradeKind::Increase
        } else {
            TradeKind::Reduce
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderEvent {
    pub coin: String,
    pub side: Side,
    pub limit_px: f64,
    pub size: f64,
    pub oid: u64,
    /// "open", "canceled", "triggered", "rejected", ...
    pub status: String,
}

#[derive(Deserialize)]
struct Envelope {
    channel: String,
    #[serde(default)]
    data: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserFills {
    #[serde(default)]
    is_snapshot: bool,
    fills: Vec<WsFill>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WsFill {
    coin: String,
    px: String,
    sz: String,
    side: String,
    time: u64,
    start_position: String,
    dir: String,
    closed_pnl: String,
    oid: u64,
    #[serde(default)]
    fee: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WsOrder {
    order: WsBasicOrder,
    status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WsBasicOrder {
    coin: String,
    side: String,
    limit_px: String,
    sz: String,
    oid: u64,
}

fn side(s: &str) -> Side {
    if s == "B" { Side::Buy } else { Side::Sell }
}

fn f(s: &str) -> f64 {
    s.parse().unwrap_or(0.0)
}

fn is_spot(coin: &str) -> bool {
    coin.starts_with('@') || coin.contains('/')
}

/// Turn one WebSocket text message into wallet events. Snapshots (the history
/// sent right after subscribing) and spot trades are ignored.
pub fn parse_message(text: &str) -> Result<Vec<WalletEvent>> {
    let env: Envelope = serde_json::from_str(text)?;
    match env.channel.as_str() {
        "userFills" => {
            let fills: UserFills = serde_json::from_value(env.data)?;
            if fills.is_snapshot {
                return Ok(Vec::new());
            }
            // Aggregate partial fills of the same order, keeping arrival order.
            let mut trades: Vec<TradeEvent> = Vec::new();
            let mut index: HashMap<u64, usize> = HashMap::new();
            for fill in fills.fills.into_iter().filter(|x| !is_spot(&x.coin)) {
                let (px, sz) = (f(&fill.px), f(&fill.sz));
                if let Some(&i) = index.get(&fill.oid) {
                    let t = &mut trades[i];
                    t.avg_px = (t.avg_px * t.size + px * sz) / (t.size + sz);
                    t.size += sz;
                    t.closed_pnl += f(&fill.closed_pnl);
                    t.fee += f(&fill.fee);
                    // Fills arrive in chronological order, so the first one
                    // already carries the order's start position.
                } else {
                    index.insert(fill.oid, trades.len());
                    trades.push(TradeEvent {
                        coin: fill.coin,
                        side: side(&fill.side),
                        size: sz,
                        avg_px: px,
                        start_position: f(&fill.start_position),
                        closed_pnl: f(&fill.closed_pnl),
                        fee: f(&fill.fee),
                        time_ms: fill.time,
                        oid: fill.oid,
                        dir: fill.dir,
                    });
                }
            }
            Ok(trades.into_iter().map(WalletEvent::Trade).collect())
        }
        "orderUpdates" => {
            let orders: Vec<WsOrder> = serde_json::from_value(env.data)?;
            Ok(orders
                .into_iter()
                // Executions are already reported as trades via userFills.
                .filter(|o| o.status != "filled" && !is_spot(&o.order.coin))
                .map(|o| {
                    WalletEvent::Order(OrderEvent {
                        coin: o.order.coin,
                        side: side(&o.order.side),
                        limit_px: f(&o.order.limit_px),
                        size: f(&o.order.sz),
                        oid: o.order.oid,
                        status: o.status,
                    })
                })
                .collect())
        }
        "error" => bail!("server error: {}", env.data),
        _ => Ok(Vec::new()), // subscriptionResponse, pong, ...
    }
}

/// WebSocket URL belonging to a REST base URL (`https://host` -> `wss://host/ws`).
pub fn ws_url(api_url: &str) -> String {
    format!(
        "{}/ws",
        api_url
            .trim_end_matches('/')
            .replacen("https://", "wss://", 1)
    )
}

/// Start a background thread that keeps a subscription for `wallet` alive
/// (with automatic reconnect) and forwards its events.
pub fn spawn(url: String, wallet: String) -> Receiver<WalletEvent> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut backoff = Duration::from_secs(1);
        loop {
            let started = Instant::now();
            let err = match run_session(&url, &wallet, &tx) {
                Ok(()) => return, // receiver dropped
                Err(e) => e,
            };
            if tx
                .send(WalletEvent::Disconnected(format!("{err:#}")))
                .is_err()
            {
                return;
            }
            if started.elapsed() > Duration::from_secs(60) {
                backoff = Duration::from_secs(1);
            }
            thread::sleep(backoff);
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    });
    rx
}

fn run_session(url: &str, wallet: &str, tx: &Sender<WalletEvent>) -> Result<()> {
    let mut ws = connect(url)?;
    for kind in ["userFills", "orderUpdates"] {
        let msg =
            json!({ "method": "subscribe", "subscription": { "type": kind, "user": wallet } });
        ws.send(Message::text(msg.to_string()))?;
    }
    if tx.send(WalletEvent::Connected).is_err() {
        return Ok(());
    }

    let mut last_ping = Instant::now();
    let mut last_msg = Instant::now();
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                last_msg = Instant::now();
                for event in parse_message(&text)? {
                    if tx.send(event).is_err() {
                        return Ok(());
                    }
                }
            }
            Ok(Message::Close(frame)) => bail!("closed by server: {frame:?}"),
            Ok(_) => last_msg = Instant::now(),
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Err(e.into()),
        }
        if last_msg.elapsed() > STALE_AFTER {
            bail!("no data for {}s", STALE_AFTER.as_secs());
        }
        if last_ping.elapsed() > PING_EVERY {
            ws.send(Message::text(r#"{"method":"ping"}"#))?;
            last_ping = Instant::now();
        }
    }
}

fn connect(url: &str) -> Result<WebSocket<MaybeTlsStream<TcpStream>>> {
    let rest = url.strip_prefix("wss://").context("expected wss:// url")?;
    let authority = rest.split('/').next().unwrap_or(rest);
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().context("bad port")?),
        None => (authority, 443u16),
    };

    let stream = match proxy_for(host) {
        Some(proxy) => connect_via_proxy(&proxy, host, port)?,
        None => {
            TcpStream::connect((host, port)).with_context(|| format!("connect {host}:{port}"))?
        }
    };
    stream.set_read_timeout(Some(READ_TIMEOUT))?;

    let (ws, _) = tungstenite::client_tls_with_config(url, stream, None, Some(tls_connector()?))
        .map_err(|e| anyhow!("websocket handshake with {host} failed: {e}"))?;
    Ok(ws)
}

/// Mozilla roots plus any extra CA bundle from `SSL_CERT_FILE`
/// (needed behind TLS-inspecting proxies).
fn tls_connector() -> Result<Connector> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Ok(path) = std::env::var("SSL_CERT_FILE") {
        for cert in CertificateDer::pem_file_iter(&path)
            .with_context(|| format!("cannot read SSL_CERT_FILE {path}"))?
            .flatten()
        {
            let _ = roots.add(cert);
        }
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(config)))
}

/// `host:port` of the HTTPS proxy to use for `host`, honouring `NO_PROXY`.
fn proxy_for(host: &str) -> Option<String> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let proxy = env("HTTPS_PROXY").or_else(|| env("https_proxy"))?;
    let no_proxy = env("NO_PROXY")
        .or_else(|| env("no_proxy"))
        .unwrap_or_default();
    let bypass = no_proxy.split(',').map(str::trim).any(|entry| {
        let entry = entry.trim_start_matches("*.").trim_start_matches('.');
        entry == "*" || host == entry || host.ends_with(&format!(".{entry}"))
    });
    if bypass {
        return None;
    }
    let addr = proxy.split("://").last().unwrap_or(&proxy);
    let addr = addr.rsplit('@').next().unwrap_or(addr);
    Some(addr.trim_end_matches('/').to_string())
}

fn connect_via_proxy(proxy: &str, host: &str, port: u16) -> Result<TcpStream> {
    let mut stream = TcpStream::connect(proxy).with_context(|| format!("connect proxy {proxy}"))?;
    write!(
        stream,
        "CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n"
    )?;
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    let mut reader = BufReader::new(&stream);
    let mut status = String::new();
    reader.read_line(&mut status)?;
    if !status.split_whitespace().nth(1).is_some_and(|c| c == "200") {
        bail!("proxy refused CONNECT {host}:{port}: {}", status.trim());
    }
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trades(text: &str) -> Vec<TradeEvent> {
        parse_message(text)
            .unwrap()
            .into_iter()
            .filter_map(|e| match e {
                WalletEvent::Trade(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    fn fill(
        coin: &str,
        side: &str,
        px: &str,
        sz: &str,
        start: &str,
        dir: &str,
        oid: u64,
    ) -> String {
        format!(
            r#"{{"coin":"{coin}","px":"{px}","sz":"{sz}","side":"{side}","time":1700000000000,"startPosition":"{start}",
               "dir":"{dir}","closedPnl":"0","hash":"0x0","oid":{oid},"crossed":true,"fee":"0.5","tid":1,"feeToken":"USDC"}}"#
        )
    }

    #[test]
    fn ignores_snapshot() {
        let msg = format!(
            r#"{{"channel":"userFills","data":{{"isSnapshot":true,"user":"0x1","fills":[{}]}}}}"#,
            fill("BTC", "B", "60000", "1", "0", "Open Long", 1)
        );
        assert!(parse_message(&msg).unwrap().is_empty());
    }

    #[test]
    fn aggregates_partial_fills_of_one_order() {
        let msg = format!(
            r#"{{"channel":"userFills","data":{{"user":"0x1","fills":[{},{},{}]}}}}"#,
            fill("BTC", "B", "60000", "1", "0", "Open Long", 7),
            fill("BTC", "B", "60300", "2", "1", "Open Long", 7),
            fill("@107", "B", "1", "5", "0", "Buy", 8),
        );
        let t = trades(&msg);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].size, 3.0);
        assert_eq!(t[0].avg_px, 60200.0);
        assert_eq!(t[0].start_position, 0.0);
        assert!((t[0].fee - 1.0).abs() < 1e-12);
        assert_eq!(t[0].kind(), TradeKind::Open);
        assert_eq!(t[0].new_position(), 3.0);
    }

    #[test]
    fn classifies_trades() {
        let kind = |side, sz, start, dir| {
            let msg = format!(
                r#"{{"channel":"userFills","data":{{"user":"0x1","fills":[{}]}}}}"#,
                fill("ETH", side, "3000", sz, start, dir, 1)
            );
            trades(&msg)[0].kind()
        };
        assert_eq!(kind("A", "2", "0", "Open Short"), TradeKind::Open);
        assert_eq!(kind("A", "2", "-1", "Open Short"), TradeKind::Increase);
        assert_eq!(kind("B", "1", "-3", "Close Short"), TradeKind::Reduce);
        assert_eq!(kind("B", "3", "-3", "Close Short"), TradeKind::Close);
        assert_eq!(kind("B", "5", "-3", "Short > Long"), TradeKind::Flip);
    }

    #[test]
    fn parses_order_updates() {
        let msg = r#"{"channel":"orderUpdates","data":[
            {"order":{"coin":"SOL","side":"B","limitPx":"150.5","sz":"10","oid":99,"timestamp":1,"origSz":"10"},"status":"open","statusTimestamp":1},
            {"order":{"coin":"SOL","side":"B","limitPx":"150.5","sz":"0","oid":98,"timestamp":1,"origSz":"10"},"status":"filled","statusTimestamp":1}
        ]}"#;
        let events = parse_message(msg).unwrap();
        assert_eq!(events.len(), 1);
        let WalletEvent::Order(o) = &events[0] else {
            panic!()
        };
        assert_eq!(
            (o.coin.as_str(), o.side, o.limit_px, o.status.as_str()),
            ("SOL", Side::Buy, 150.5, "open")
        );
    }

    #[test]
    fn ignores_control_messages() {
        assert!(parse_message(r#"{"channel":"pong"}"#).unwrap().is_empty());
        assert!(
            parse_message(r#"{"channel":"subscriptionResponse","data":{"method":"subscribe"}}"#)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn builds_ws_url() {
        assert_eq!(
            ws_url("https://api.hyperliquid.xyz"),
            "wss://api.hyperliquid.xyz/ws"
        );
        assert_eq!(
            ws_url("https://api.hyperliquid-testnet.xyz/"),
            "wss://api.hyperliquid-testnet.xyz/ws"
        );
    }
}
