# Gridlock

A single-player puzzle game about balancing a Power-to-X grid over one day.
You size production, storage, conversion and security against a forecast,
then watch the real day play out and react in time. Built on
[fss-mqtt-broker](https://github.com/mbilling/fss-mqtt-broker) (mqttd) as its event
bus and fss-mqtt as its analyst HUD.

This is milestone **M0**: a headless, deterministic Rust simulation with a CLI
and one level, `cloudy_tuesday`. No graphics yet.

## Play it

```sh
cargo build --release

# Check a scenario, and a plan's cost and attack exposure
./target/release/gridlock validate scenarios/cloudy_tuesday/cloudy_tuesday.toml \
    --plan tests/golden/plans/balanced.toml

# Play a plan hands-off: per-tick table, View's debrief, PASS/FAIL
./target/release/gridlock run scenarios/cloudy_tuesday/cloudy_tuesday.toml \
    --plan tests/golden/plans/balanced.toml

# Play interactively: Enter = next tick, b = boost, u <id> = start an upgrade, q = quit
./target/release/gridlock play scenarios/cloudy_tuesday/cloudy_tuesday.toml \
    --plan tests/golden/plans/unsecured.toml
```

`run` also takes `--interactions <log.toml>` (replay what a player did),
`--json <out.json>` (the whole run, a stable format) and `--tick-ms <ms>`
(animate). `gridlock debrief <out.json>` reprints a saved run's debrief.
`play --tick-ms 625` plays a day in 60 seconds.

Exit codes: `0` pass, `1` fail, `2` invalid scenario or plan, `3` bus error.

To make your own plan, copy `tests/golden/plans/balanced.toml` and change it.
The level's catalog, costs and budget are in
`scenarios/cloudy_tuesday/cloudy_tuesday.toml`.

## Rules

A day is 96 ticks of 15 minutes. Each tick, production goes to demand first.
Surplus fills `charge_priority` in order, and whatever is left is curtailed. A
deficit draws on `discharge_priority` in order, then the boost if it is armed;
whatever is left is unmet.

You **pass** with at least 1 star, efficiency at or above the target, and every
contract delivered.

- **Stars** start at 3. You lose one for each outage tick (unmet demand above
  2% of demand), one the first time CO₂ exceeds the cap, one for each missed
  contract, and one when a full broker outage lands. A broker outage that
  `cluster` shortens to a blip does not cost that star. At 0 stars the day
  ends immediately.
- **Efficiency** is `1 − (curtailed + exported + conversion losses − leftover) / harvested`,
  clamped to 0–1. Export earns import credits but counts as waste. Leftover is
  the day-end gain in stored energy, with H₂ counted at fuel-cell efficiency.
- **CO₂** comes from imports (0.15 kg/kWh) and boost (0.45 kg/kWh).
- **Forecast tiers** reveal the next N ticks of the real curves during play.
  Events and attacks are never revealed.
- **Security**: `mtls` blocks credential theft and rogue clients, `acl` blocks
  rogue clients, `cluster` turns a broker outage into a 2-tick failover, and
  `ids` warns 4 ticks before an attack. An upgrade can be bought in Plan, or
  started in Play from unspent budget. When started in Play, it takes effect
  after `setup_ticks`, and the operator is busy until then.

### Cloudy Tuesday, the reference plans

| Plan | Result | Why |
| --- | --- | --- |
| `balanced.toml` | PASS, 3 stars, 98% | Battery for the night and the 19:00 peak, electrolyser for the 40 kg H₂ contract, mTLS and cluster |
| `unsecured.toml` | FAIL | Same assets, no upgrades: the battery hijack at 15:00 costs 1,700 kWh and the 19:00 broker outage finishes it |
| `no_storage.toml` | FAIL at 00:30 | Nothing carries the night; play stops on the third outage tick |
| `all_import.toml` | FAIL | Lives on the interconnector; imports push CO₂ past the 400 kg cap |

## Topic contract

All payloads are UTF-8 JSON. Times are tick indices, power is in kW and energy in kWh. A
configurable root prefix (for example `ptx/`) is prepended to every topic.

| Topic | Payload | QoS | Retain | Direction |
| --- | --- | --- | --- | --- |
| `game/phase` | `{"phase":"play","level":"cloudy_tuesday"}` | 1 | yes | out |
| `game/plan` | the plan as JSON | 1 | yes | out |
| `game/action` | `{"action":"boost"}`, `{"action":"upgrade","arg":"mtls"}` | 1 | no | in |
| `game/boost` | `{"arm":true}` (shorthand for boost) | 1 | no | in |
| `sim/tick` | `{"t":37,"minute":555,"dt":15}` | 0 | yes | out |
| `forecast/demand`, `forecast/pv`, `forecast/wind` | `[kW per tick]`, sources at planned capacity | 1 | yes | out |
| `forecast/actual_window` | `{"from":38,"demand":[],"pv":[],"wind":[]}` | 0 | yes | out |
| `plant/<id>/state` | `{"kind":"battery","p_kw":-120,"soc":0.63,"tank_kg":null,"status":"ok"}` | 1 | yes | out |
| `grid/demand` | `{"kw":812}` | 0 | no | out |
| `grid/balance` | `{"supply_kw":…,"demand_kw":…,"unmet_kw":…,"curtailed_kw":…,"export_kw":…,"import_kw":…}` | 0 | yes | out |
| `grid/event` | `{"kind":"outage","asset":"wind-1","ticks":8}` | 1 | no | out |
| `score/live` | `{"stars":2,"efficiency":0.81,"co2_kg":140,"boost_charges":1}` | 0 | yes | out |
| `score/round` | `{"stars":2,"efficiency":0.81,"co2_kg":140,"pass":true,"contracts_met":1}` | 1 | yes | out |
| `security/status` | `{"mtls":true,"acl":false,"cluster":false,"ids":true,"busy_until":44}` | 1 | yes | out |
| `security/alert` | `{"vector":"rogue_client","eta_ticks":4}` (only with `ids`) | 1 | no | out |
| `security/attack` | `{"vector":"credential_theft","target":"bat-1","blocked_by":"mtls","ticks":8}` | 1 | no | out |
| `debrief/<n>` | `{"kind":"curtailment","t_from":44,"t_to":56,"kwh":1240,"asset":"bat-1","text":"…"}` | 1 | yes | out |

`p_kw` is positive when delivering to the grid and negative when drawing from it. Each
asset's will on `plant/<id>/state` is `{"status":"offline"}`. Retained topics
are cleared when a new level starts.

## Layout

| Path | What |
| --- | --- |
| `crates/gridlock-core` | Scenario loading, plan validation, tick engine, scoring, debrief. No IO and no RNG; builds for `wasm32-unknown-unknown`. |
| `crates/gridlock-bus` | `Bus` trait, `MemoryBus` (the reference topic contract), `Session` (engine ↔ bus). `MqttBus` is stubbed behind feature `mqtt` until M1. |
| `crates/gridlock-cli` | The `gridlock` binary, plus the golden-test harness. |
| `scenarios/cloudy_tuesday` | The level: TOML plus CSV curves. `tools/gen_curves.py` regenerates the curves. |
| `tests/golden` | Reference plans, an interaction log and expected `--json` output. |
| `tests/fixtures` | Broken scenarios and plans that `validate` must reject. |

## Develop

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --target wasm32-unknown-unknown -p gridlock-core
UPDATE_GOLDEN=1 cargo test -p gridlock-cli   # after an intended behaviour change
tools/summary.sh                              # verdicts of all reference plans
```

For M1 against a live mqttd, note that the broker is secure by default. A local
test broker needs `MQTTD_PLAINTEXT_BIND=127.0.0.1:1883 MQTTD_ALLOW_ANONYMOUS=1`.
The client reads `GRIDLOCK_MQTT_URL`, `GRIDLOCK_MQTT_USER` and
`GRIDLOCK_MQTT_PASS`.

The design spec, with the decisions taken on its open questions, is the
"Power-to-X Puzzle Game — Build Spec" doc.
