//! "Copy Desk": live web dashboard that shows the followed wallet, every step
//! of the bot pipeline (as six agents) and the paper account.

pub mod demo;
pub mod server;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use serde::Serialize;

use crate::engine::{Action, CopyEngine, Decision, Sizing};
use crate::hyperliquid::{
    HyperliquidClient, OpenOrder, PositionInfo, parse_clearinghouse_state, position_details,
};
use crate::journal::Journal;
use crate::source::MarketSource;
use crate::types::{AccountSnapshot, Fill, Mids};
use crate::ws::{TradeEvent, TradeKind, WalletEvent};
use server::Hub;

const MAX_ACTIVITY: usize = 300;
const MAX_HISTORY: usize = 900;

/// Everything the desk needs from one poll of the target wallet.
pub struct DeskSnapshot {
    pub target: AccountSnapshot,
    pub positions: Vec<PositionInfo>,
    pub orders: Vec<OpenOrder>,
    pub mids: Mids,
    /// Trades detected by the source itself (demo); live trades come via WebSocket.
    pub trades: Vec<TradeEvent>,
}

pub trait DeskSource {
    fn poll(&mut self, wallet: &str) -> Result<DeskSnapshot>;
}

impl DeskSource for HyperliquidClient {
    fn poll(&mut self, wallet: &str) -> Result<DeskSnapshot> {
        let state = self.clearinghouse_state(wallet)?;
        let positions = position_details(&state);
        let target = parse_clearinghouse_state(state)?;
        Ok(DeskSnapshot {
            target,
            positions,
            orders: self.open_orders(wallet)?,
            mids: self.mids()?,
            trades: Vec::new(),
        })
    }
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    Spotter,
    Tracker,
    Sizer,
    Risk,
    Executor,
    Ledger,
}

impl Agent {
    pub const ALL: [Agent; 6] = [
        Agent::Spotter,
        Agent::Tracker,
        Agent::Sizer,
        Agent::Risk,
        Agent::Executor,
        Agent::Ledger,
    ];

    fn role(self) -> &'static str {
        match self {
            Agent::Spotter => "Live-Feed: erkennt Trades & Orders",
            Agent::Tracker => "Synchronisiert Positionen der Signalquelle",
            Agent::Sizer => "Rechnet Größe auf unser Konto um",
            Agent::Risk => "Hebel, Limits, Mindestorder",
            Agent::Executor => "Führt Paper-Orders aus",
            Agent::Ledger => "PnL, Fees, Journal",
        }
    }
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Trade,
    Skip,
    Warn,
    Pnl,
}

#[derive(Serialize, Clone)]
pub struct Activity {
    pub seq: u64,
    /// Sync round the entry belongs to; all steps of one trade share it.
    pub tick: u64,
    pub ts: i64,
    pub agent: Agent,
    pub level: Level,
    pub text: String,
    pub value: Option<f64>,
}

#[derive(Serialize, Clone)]
pub struct AgentView {
    pub id: Agent,
    pub role: &'static str,
    pub count: u64,
    pub last: String,
    pub last_ts: i64,
}

#[derive(Serialize, Clone, Default)]
pub struct FeedView {
    pub connected: bool,
    pub last_event_ts: Option<i64>,
    pub reconnects: u32,
    pub error: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct ConfigView {
    pub sizing: String,
    pub max_leverage: f64,
    pub min_order_usd: f64,
    pub max_position_usd: Option<f64>,
    pub rebalance_threshold: f64,
    pub interval_secs: u64,
    pub skip_existing: bool,
}

#[derive(Serialize, Clone)]
pub struct TargetPos {
    pub coin: String,
    pub size: f64,
    pub entry_px: f64,
    pub mark: Option<f64>,
    pub value: f64,
    pub upnl: f64,
    pub leverage: Option<String>,
    pub liq_px: Option<f64>,
}

#[derive(Serialize, Clone)]
pub struct OrderView {
    pub coin: String,
    pub side: &'static str,
    pub size: f64,
    pub limit_px: f64,
    pub trigger_px: Option<f64>,
    pub order_type: String,
    pub reduce_only: bool,
    pub tpsl: bool,
    pub mark: Option<f64>,
}

#[derive(Serialize, Clone, Default)]
pub struct TargetView {
    pub account_value: f64,
    pub notional: f64,
    pub positions: Vec<TargetPos>,
    pub orders: Vec<OrderView>,
}

#[derive(Serialize, Clone)]
pub struct PaperPos {
    pub coin: String,
    pub size: f64,
    pub entry_px: f64,
    pub mark: Option<f64>,
    pub upnl: f64,
}

#[derive(Serialize, Clone, Default)]
pub struct PaperView {
    pub equity: f64,
    pub start: f64,
    pub cash: f64,
    pub realized: f64,
    pub unrealized: f64,
    pub fees: f64,
    pub trades: u64,
    pub wins: u64,
    pub losses: u64,
    pub notional: f64,
    pub scale: f64,
    pub positions: Vec<PaperPos>,
}

#[derive(Serialize, Clone)]
pub struct DecisionView {
    pub coin: String,
    pub target_size: f64,
    pub desired: f64,
    pub current: f64,
    pub delta: f64,
    pub mid: Option<f64>,
    pub action: &'static str,
    pub capped: bool,
}

#[derive(Serialize, Clone)]
pub struct DeskState {
    pub mode: &'static str,
    pub wallet: String,
    pub started: i64,
    pub updated: i64,
    pub tick: u64,
    pub feed: FeedView,
    pub config: ConfigView,
    pub target: TargetView,
    pub paper: PaperView,
    pub decisions: Vec<DecisionView>,
    pub agents: Vec<AgentView>,
    pub activity: VecDeque<Activity>,
    /// `[timestamp_ms, our equity, target account value]`
    pub history: VecDeque<[f64; 3]>,
    pub mids: BTreeMap<String, f64>,
}

pub struct DeskOptions {
    pub demo: bool,
    pub interval: Duration,
    pub state_path: Option<PathBuf>,
    pub journal: Option<Journal>,
}

pub struct Desk {
    pub state: DeskState,
    engine: CopyEngine,
    hub: Hub,
    opts: DeskOptions,
    seq: u64,
    last_target: Option<HashMap<String, f64>>,
    last_actions: HashMap<String, Action>,
    in_tick: bool,
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn action_name(a: Action) -> &'static str {
    match a {
        Action::Trade => "trade",
        Action::InSync => "sync",
        Action::SkipMinOrder => "min_order",
        Action::SkipThreshold => "threshold",
        Action::SkipExisting => "existing",
        Action::SkipNoPrice => "no_price",
    }
}

fn label(t: &TradeEvent) -> String {
    let dir = |size: f64| if size > 0.0 { "LONG" } else { "SHORT" };
    match t.kind() {
        TradeKind::Open => format!("eröffnet {}", dir(t.new_position())),
        TradeKind::Increase => format!("stockt {} auf", dir(t.new_position())),
        TradeKind::Reduce => format!("reduziert {}", dir(t.start_position)),
        TradeKind::Close => format!("schließt {}", dir(t.start_position)),
        TradeKind::Flip => format!("dreht auf {}", dir(t.new_position())),
    }
}

impl Desk {
    pub fn new(engine: CopyEngine, hub: Hub, opts: DeskOptions) -> Self {
        let c = &engine.config;
        let config = ConfigView {
            sizing: match c.sizing {
                Sizing::Proportional { multiplier } => format!("proportional ×{multiplier}"),
                Sizing::Fixed { ratio } => format!("fix {ratio}"),
            },
            max_leverage: c.max_leverage,
            min_order_usd: c.min_order_usd,
            max_position_usd: c.max_position_usd,
            rebalance_threshold: c.rebalance_threshold,
            interval_secs: opts.interval.as_secs(),
            skip_existing: c.skip_existing,
        };
        let state = DeskState {
            mode: if opts.demo { "DEMO" } else { "LIVE" },
            wallet: c.wallet.clone(),
            started: now_ms(),
            updated: now_ms(),
            tick: 0,
            feed: FeedView::default(),
            config,
            target: TargetView::default(),
            paper: PaperView::default(),
            decisions: Vec::new(),
            agents: Agent::ALL
                .iter()
                .map(|&id| AgentView {
                    id,
                    role: id.role(),
                    count: 0,
                    last: "bereit".into(),
                    last_ts: 0,
                })
                .collect(),
            activity: VecDeque::new(),
            history: VecDeque::new(),
            mids: BTreeMap::new(),
        };
        let mut desk = Self {
            state,
            engine,
            hub,
            opts,
            seq: 0,
            last_target: None,
            last_actions: HashMap::new(),
            in_tick: false,
        };
        desk.refresh_paper(&Mids::new(), &AccountSnapshot::default());
        desk.publish();
        desk
    }

    fn agent(&mut self, id: Agent) -> &mut AgentView {
        self.state
            .agents
            .iter_mut()
            .find(|a| a.id == id)
            .expect("all agents exist")
    }

    /// Update an agent's status line without adding to the activity log.
    fn beat(&mut self, id: Agent, text: impl Into<String>) {
        let a = self.agent(id);
        a.last = text.into();
        a.last_ts = now_ms();
    }

    fn log(&mut self, agent: Agent, level: Level, text: impl Into<String>, value: Option<f64>) {
        let text = text.into();
        self.seq += 1;
        let a = self.agent(agent);
        a.count += 1;
        a.last = text.clone();
        a.last_ts = now_ms();
        // Live-feed events arrive between rounds and belong to the next one.
        let tick = if self.in_tick {
            self.state.tick
        } else {
            self.state.tick + 1
        };
        self.state.activity.push_front(Activity {
            seq: self.seq,
            tick,
            ts: now_ms(),
            agent,
            level,
            text,
            value,
        });
        self.state.activity.truncate(MAX_ACTIVITY);
    }

    pub fn publish(&mut self) {
        self.state.updated = now_ms();
        if let Ok(json) = serde_json::to_string(&self.state) {
            self.hub.broadcast(json);
        }
    }

    pub fn handle_event(&mut self, event: &WalletEvent) {
        match event {
            WalletEvent::Connected => {
                self.state.feed.connected = true;
                self.state.feed.error = None;
                self.log(
                    Agent::Spotter,
                    Level::Info,
                    "Live-Feed verbunden (userFills + orderUpdates)",
                    None,
                );
            }
            WalletEvent::Disconnected(err) => {
                if self.state.feed.connected {
                    self.state.feed.reconnects += 1;
                }
                self.state.feed.connected = false;
                self.state.feed.error = Some(err.clone());
                self.log(
                    Agent::Spotter,
                    Level::Warn,
                    format!("Feed getrennt: {err}"),
                    None,
                );
            }
            WalletEvent::Trade(t) => {
                self.state.feed.last_event_ts = Some(now_ms());
                let text = format!(
                    "Quelle {} {} {:+} @ {} → Position {:+}",
                    label(t),
                    t.coin,
                    round(t.signed_size()),
                    px(t.avg_px),
                    round(t.new_position())
                );
                let pnl = (t.closed_pnl != 0.0).then_some(t.closed_pnl);
                self.log(Agent::Spotter, Level::Trade, text, pnl);
            }
            WalletEvent::Order(o) => {
                self.state.feed.last_event_ts = Some(now_ms());
                let side = if o.side == crate::types::Side::Buy {
                    "BUY"
                } else {
                    "SELL"
                };
                let text = format!(
                    "Order {}: {side} {} {} @ {}",
                    o.status,
                    round(o.size),
                    o.coin,
                    px(o.limit_px)
                );
                self.log(Agent::Spotter, Level::Info, text, None);
            }
        }
    }

    pub fn tick(&mut self, source: &mut dyn DeskSource) {
        self.state.tick += 1;
        self.in_tick = true;
        self.tick_inner(source);
        self.in_tick = false;
    }

    fn tick_inner(&mut self, source: &mut dyn DeskSource) {
        let snap = match source.poll(&self.state.wallet) {
            Ok(s) => s,
            Err(e) => {
                self.log(
                    Agent::Tracker,
                    Level::Warn,
                    format!("Abruf fehlgeschlagen: {e:#}"),
                    None,
                );
                self.publish();
                return;
            }
        };
        for t in &snap.trades {
            self.handle_event(&WalletEvent::Trade(t.clone()));
        }
        self.track(&snap);

        let decisions = self.engine.plan_decisions(&snap.target, &snap.mids);
        self.review(&decisions, &snap);
        let orders = CopyEngine::orders(&decisions);
        let fills = self.engine.execute(&orders);
        for fill in &fills {
            self.book(fill, &snap.mids);
        }

        self.state.decisions = decisions
            .iter()
            .map(|d| DecisionView {
                coin: d.coin.clone(),
                target_size: d.target_size,
                desired: d.desired,
                current: self.engine.account.size(&d.coin),
                delta: d.delta,
                mid: d.mid,
                action: action_name(d.action),
                capped: d.capped,
            })
            .collect();
        self.refresh_target(&snap);
        self.refresh_paper(&snap.mids, &snap.target);
        let equity = self.state.paper.equity;
        self.beat(
            Agent::Ledger,
            format!(
                "Equity {:.2} · uPnL {:+.2}",
                equity, self.state.paper.unrealized
            ),
        );
        self.state
            .history
            .push_back([now_ms() as f64, equity, snap.target.account_value]);
        while self.state.history.len() > MAX_HISTORY {
            self.state.history.pop_front();
        }
        if let Some(path) = &self.opts.state_path
            && let Err(e) = self.engine.account.save(path)
        {
            self.log(
                Agent::Ledger,
                Level::Warn,
                format!("State nicht gespeichert: {e}"),
                None,
            );
        }
        self.publish();
    }

    fn track(&mut self, snap: &DeskSnapshot) {
        let now = &snap.target.positions;
        match self.last_target.take() {
            None => {
                let text = format!(
                    "Startzustand: {} Positionen, Konto {:.0} USDC",
                    now.len(),
                    snap.target.account_value
                );
                self.log(Agent::Tracker, Level::Info, text, None);
            }
            Some(before) => {
                let mut coins: Vec<&String> = before.keys().chain(now.keys()).collect();
                coins.sort();
                coins.dedup();
                for coin in coins {
                    let (a, b) = (
                        before.get(coin).copied().unwrap_or(0.0),
                        now.get(coin).copied().unwrap_or(0.0),
                    );
                    if (a - b).abs() > 1e-12 {
                        let text = format!("{coin}: Position {:+} → {:+}", round(a), round(b));
                        self.log(Agent::Tracker, Level::Info, text, None);
                    }
                }
            }
        }
        self.last_target = Some(now.clone());
        self.beat(
            Agent::Tracker,
            format!(
                "Sync #{}: {} Positionen · {} Orders",
                self.state.tick,
                now.len(),
                snap.orders.len()
            ),
        );
    }

    fn review(&mut self, decisions: &[Decision], snap: &DeskSnapshot) {
        let scale = self.engine.scale(&snap.target, &snap.mids);
        for d in decisions {
            let changed = self.last_actions.get(&d.coin) != Some(&d.action);
            match d.action {
                Action::Trade => {
                    let text = if d.desired == 0.0 {
                        format!(
                            "{}: Quelle ist raus → wir schließen {:+}",
                            d.coin,
                            round(d.current)
                        )
                    } else {
                        format!(
                            "{}: {:+} × {:.6} = Soll {:+} (Ist {:+})",
                            d.coin,
                            round(d.target_size),
                            scale,
                            round(d.desired),
                            round(d.current)
                        )
                    };
                    self.log(Agent::Sizer, Level::Info, text, None);
                    if d.capped {
                        let text = format!(
                            "{}: Limit greift → Größe auf {:+} gedeckelt",
                            d.coin,
                            round(d.desired)
                        );
                        self.log(Agent::Risk, Level::Warn, text, None);
                    } else {
                        self.log(
                            Agent::Risk,
                            Level::Info,
                            format!("{}: freigegeben", d.coin),
                            None,
                        );
                    }
                }
                Action::SkipMinOrder if changed => {
                    let usd = d.delta.abs() * d.mid.unwrap_or_default();
                    let text = format!(
                        "{}: Anpassung {:.2} $ < {} $ Mindestorder – wartet",
                        d.coin, usd, self.engine.config.min_order_usd
                    );
                    self.log(Agent::Risk, Level::Skip, text, None);
                }
                Action::SkipThreshold if changed => {
                    let text = format!("{}: Abweichung unter Rebalance-Schwelle – bleibt", d.coin);
                    self.log(Agent::Risk, Level::Skip, text, None);
                }
                Action::SkipExisting if changed => {
                    let text = format!("{}: bestand schon beim Start – wird ignoriert", d.coin);
                    self.log(Agent::Risk, Level::Skip, text, None);
                }
                Action::SkipNoPrice if changed => {
                    self.log(
                        Agent::Risk,
                        Level::Warn,
                        format!("{}: kein Preis – übersprungen", d.coin),
                        None,
                    );
                }
                _ => {}
            }
        }
        self.last_actions = decisions
            .iter()
            .map(|d| (d.coin.clone(), d.action))
            .collect();
        self.beat(
            Agent::Sizer,
            format!("Faktor {scale:.6} (unser Equity / Quelle)"),
        );
        if !decisions.iter().any(|d| d.action == Action::Trade) {
            self.beat(Agent::Executor, "keine Order nötig");
        }
    }

    fn book(&mut self, fill: &Fill, mids: &Mids) {
        let text = format!(
            "{} {} {} @ {} · Fee {:.4}",
            fill.side,
            round(fill.size),
            fill.coin,
            px(fill.price),
            fill.fee
        );
        self.log(Agent::Executor, Level::Trade, text, Some(-fill.fee));
        if fill.realized_pnl != 0.0 {
            if fill.realized_pnl > 0.0 {
                self.state.paper.wins += 1;
            } else {
                self.state.paper.losses += 1;
            }
            let text = format!("{}: realisiert {:+.2} USDC", fill.coin, fill.realized_pnl);
            self.log(Agent::Ledger, Level::Pnl, text, Some(fill.realized_pnl));
        }
        if let Some(journal) = &self.opts.journal {
            let _ = journal.record(fill, self.engine.account.equity(mids));
        }
    }

    fn refresh_target(&mut self, snap: &DeskSnapshot) {
        let mark = |coin: &str| snap.mids.get(coin).copied();
        self.state.target = TargetView {
            account_value: snap.target.account_value,
            notional: snap.positions.iter().map(|p| p.position_value).sum(),
            positions: snap
                .positions
                .iter()
                .map(|p| TargetPos {
                    coin: p.coin.clone(),
                    size: p.size,
                    entry_px: p.entry_px,
                    mark: mark(&p.coin),
                    value: p.position_value,
                    upnl: p.unrealized_pnl,
                    leverage: p.leverage.clone(),
                    liq_px: p.liquidation_px,
                })
                .collect(),
            orders: snap
                .orders
                .iter()
                .map(|o| OrderView {
                    coin: o.coin.clone(),
                    side: if o.side == "B" { "BUY" } else { "SELL" },
                    size: o.sz.parse().unwrap_or(0.0),
                    limit_px: o.limit_px.parse().unwrap_or(0.0),
                    trigger_px: o.is_trigger.then(|| o.trigger_px.parse().ok()).flatten(),
                    order_type: o.order_type.clone(),
                    reduce_only: o.reduce_only,
                    tpsl: o.is_position_tpsl,
                    mark: mark(&o.coin),
                })
                .collect(),
        };
        let mut coins: Vec<&String> = snap.target.positions.keys().collect();
        coins.extend(self.engine.account.positions.keys());
        coins.extend(snap.orders.iter().map(|o| &o.coin));
        self.state.mids = coins
            .into_iter()
            .filter_map(|c| snap.mids.get(c).map(|p| (c.clone(), *p)))
            .collect();
    }

    fn refresh_paper(&mut self, mids: &Mids, target: &AccountSnapshot) {
        let acc = &self.engine.account;
        let mut positions: Vec<PaperPos> = acc
            .positions
            .iter()
            .map(|(coin, p)| {
                let mark = mids.get(coin).copied();
                PaperPos {
                    coin: coin.clone(),
                    size: p.size,
                    entry_px: p.entry_px,
                    mark,
                    upnl: mark.map_or(0.0, |m| p.size * (m - p.entry_px)),
                }
            })
            .collect();
        positions.sort_by(|a, b| a.coin.cmp(&b.coin));
        self.state.paper = PaperView {
            equity: acc.equity(mids),
            start: acc.starting_balance,
            cash: acc.cash,
            realized: acc.realized_pnl,
            unrealized: acc.unrealized_pnl(mids),
            fees: acc.fees_paid,
            trades: acc.trade_count,
            wins: self.state.paper.wins,
            losses: self.state.paper.losses,
            notional: acc.notional(mids),
            scale: self.engine.scale(target, mids),
            positions,
        };
    }

    /// Main loop: poll every `interval`, but re-sync immediately when the live
    /// feed reports a trade.
    pub fn run(
        mut self,
        mut source: Box<dyn DeskSource>,
        feed: Option<Receiver<WalletEvent>>,
    ) -> ! {
        if self.opts.demo {
            self.state.feed.connected = true;
            self.log(
                Agent::Spotter,
                Level::Info,
                "Demo-Feed aktiv (synthetische Signalquelle)",
                None,
            );
        }
        loop {
            self.tick(source.as_mut());
            let deadline = Instant::now() + self.opts.interval;
            let Some(rx) = &feed else {
                std::thread::sleep(self.opts.interval);
                continue;
            };
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                match rx.recv_timeout(left) {
                    Ok(event) => {
                        let is_trade = matches!(event, WalletEvent::Trade(_));
                        self.handle_event(&event);
                        self.publish();
                        if is_trade {
                            // Collect the rest of a burst, then re-sync right away.
                            while let Ok(more) = rx.recv_timeout(Duration::from_millis(300)) {
                                self.handle_event(&more);
                            }
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => {
                        std::thread::sleep(left);
                        break;
                    }
                }
            }
        }
    }
}

/// Quantity for log texts: compact (5.57M, 12.3K) with a few significant
/// digits; honours `{:+}`.
struct Qty(f64);

impl std::fmt::Display for Qty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (x, a) = (self.0, self.0.abs());
        let sign = if x < 0.0 {
            "-"
        } else if f.sign_plus() && x > 0.0 {
            "+"
        } else {
            ""
        };
        let trim = |s: String| {
            if s.contains('.') {
                s.trim_end_matches('0').trim_end_matches('.').to_string()
            } else {
                s
            }
        };
        let body = if a >= 1e6 {
            format!("{:.2}M", a / 1e6)
        } else if a >= 1e4 {
            format!("{:.1}K", a / 1e3)
        } else if a >= 100.0 {
            trim(format!("{a:.1}"))
        } else if a >= 1.0 {
            trim(format!("{a:.3}"))
        } else if a == 0.0 {
            "0".into()
        } else {
            // Three significant digits for small sizes (0.0613, 0.000412).
            let decimals = (2 - a.log10().floor() as i32).clamp(0, 10) as usize;
            trim(format!("{a:.decimals$}"))
        };
        write!(f, "{sign}{body}")
    }
}

fn round(x: f64) -> Qty {
    Qty(x)
}

/// Price with precision depending on magnitude.
fn px(p: f64) -> String {
    let decimals = match p.abs() {
        a if a >= 1_000.0 => 1,
        a if a >= 10.0 => 3,
        a if a >= 1.0 => 4,
        _ => 6,
    };
    format!("{p:.decimals$}")
}

/// Allow `Desk` to be driven by a plain `MarketSource` (e.g. the scripted mock) in tests.
pub struct PlainSource<S: MarketSource>(pub S);

impl<S: MarketSource> DeskSource for PlainSource<S> {
    fn poll(&mut self, wallet: &str) -> Result<DeskSnapshot> {
        let target = self.0.target_snapshot(wallet)?;
        let mids = self.0.mids()?;
        let positions = target
            .positions
            .iter()
            .map(|(coin, &size)| PositionInfo {
                coin: coin.clone(),
                size,
                entry_px: mids.get(coin).copied().unwrap_or_default(),
                position_value: (size * mids.get(coin).copied().unwrap_or_default()).abs(),
                unrealized_pnl: 0.0,
                liquidation_px: None,
                leverage: None,
            })
            .collect();
        Ok(DeskSnapshot {
            target,
            positions,
            orders: Vec::new(),
            mids,
            trades: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::CopyConfig;
    use crate::mock::{ScriptedSource, demo_scenario};
    use crate::paper::PaperAccount;

    #[test]
    fn quantities_read_compact() {
        let f = |x: f64| format!("{:+}", Qty(x));
        assert_eq!(f(5_569_012.0), "+5.57M");
        assert_eq!(f(-29_567.17), "-29.6K");
        assert_eq!(f(263.06), "+263.1");
        assert_eq!(f(1.4199), "+1.42");
        assert_eq!(f(0.061284), "+0.0613");
        assert_eq!(f(0.0), "0");
        assert_eq!(format!("{}", Qty(2.5)), "2.5");
    }

    #[test]
    fn desk_logs_every_pipeline_stage() {
        let engine = CopyEngine::new(
            CopyConfig::new("0xabc"),
            PaperAccount::new(1_000.0, 0.00045, 5.0),
        );
        let opts = DeskOptions {
            demo: true,
            interval: Duration::from_secs(1),
            state_path: None,
            journal: None,
        };
        let mut desk = Desk::new(engine, Hub::default(), opts);
        let mut source = PlainSource(ScriptedSource::new(demo_scenario()));
        for _ in 0..demo_scenario().len() {
            desk.tick(&mut source);
        }
        let s = &desk.state;
        for agent in [
            Agent::Tracker,
            Agent::Sizer,
            Agent::Risk,
            Agent::Executor,
            Agent::Ledger,
        ] {
            assert!(
                s.activity.iter().any(|a| a.agent == agent),
                "{agent:?} never logged"
            );
        }
        assert_eq!(s.paper.trades, 7);
        assert!(s.paper.wins >= 3);
        assert!(s.paper.positions.is_empty());
        assert_eq!(s.history.len(), 7);
        // Newest first, strictly increasing sequence numbers.
        assert!(
            s.activity
                .iter()
                .zip(s.activity.iter().skip(1))
                .all(|(a, b)| a.seq > b.seq)
        );
        serde_json::to_string(s).unwrap();
    }
}
