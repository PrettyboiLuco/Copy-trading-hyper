# copy-trading-hyper

Copy-Trading-Bot für **Hyperliquid** in Rust – als **Test-System (Paper Trading)**.
Der Bot folgt standardmäßig der Wallet

```
0x0c1f6941FDcDe9488DfD36cA16FeD6B3bc49672B
```

und spiegelt deren Perp-Positionen auf ein simuliertes Konto. Es werden **keine echten Orders**
gesendet und **kein Private Key** benötigt – es wird nur die öffentliche `/info`-API gelesen.

## Funktionsweise

Alle `--interval` Sekunden:

1. `clearinghouseState` der Ziel-Wallet abrufen (Account Value + Positionen je Coin)
2. `allMids` abrufen (aktuelle Mid-Preise)
3. Soll-Position je Coin berechnen:
   - `proportional` (Standard): `Zielgröße × (mein Equity / Ziel-Equity) × multiplier`
   - `fixed`: `Zielgröße × ratio`
4. Limits anwenden: `--max-leverage` (Gesamt-Notional), `--max-position-usd` (pro Coin)
5. Differenz zur eigenen Paper-Position als Market-Order zum Mid ± Slippage ausführen
   (inkl. Taker-Fee). Orders unter `--min-order-usd` und Mini-Rebalances unter
   `--rebalance-threshold` werden übersprungen; schließt das Ziel einen Coin, wird immer komplett geschlossen.

Der Kontostand wird in `paper_state.json` gespeichert (Neustart setzt fort), jede simulierte
Ausführung landet in `trades.csv`.

## Nutzung

```bash
# Tests
cargo test

# Offline-Demo-Szenario (kein Netzwerk nötig)
cargo run -- simulate

# Aktuelle Positionen der Wallet anzeigen
cargo run -- status

# Live folgen & Paper-traden (1000 USDC Startkapital, max. 5x Hebel)
cargo run --release -- run --balance 1000 --interval 5

# Nur neue Trades kopieren, bestehende Positionen beim Start ignorieren
cargo run --release -- run --skip-existing

# Fester Faktor statt proportional: 1 % der Zielgröße
cargo run --release -- run --sizing fixed --ratio 0.01
```

Andere Wallet: `--wallet 0x...` oder Umgebungsvariable `TARGET_WALLET`.
Alle Optionen: `cargo run -- run --help`. Neu starten mit frischem Konto: `paper_state.json` löschen.

## Aufbau

| Datei | Inhalt |
|---|---|
| `src/hyperliquid.rs` | Read-only Client für die Hyperliquid Info-API |
| `src/engine.rs` | Copy-Logik: Sizing, Limits, Order-Planung |
| `src/paper.rs` | Simuliertes Konto: Fills, Fees, Slippage, PnL |
| `src/mock.rs` | Skript-basierte Offline-Datenquelle + Demo-Szenario |
| `src/journal.rs` | CSV-Trade-Log |
| `tests/scenario.rs` | End-to-End-Test über das Demo-Szenario |

## Grenzen der Simulation

- Ausführung zum Mid-Preis ± fixer Slippage, keine Orderbuch-Tiefe
- Funding-Zahlungen und Liquidationen werden nicht simuliert
- Nur Perps (Haupt-DEX), kein Spot
- Polling-basiert: Trades zwischen zwei Abfragen werden verzögert kopiert
