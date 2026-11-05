//! The tick engine: Plan -> Play -> Debrief as an explicit state machine.

use crate::debrief::{self, Finding};
use crate::error::LoadError;
use crate::message::{topics, Message, Qos};
use crate::plan::{AssetSpec, InteractionEntry, Plan, ValidPlan};
use crate::scenario::{CatalogKind, CurveId, EventKind, Scenario, Vector};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

const EPS: f64 = 1e-9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Plan,
    Play,
    Debrief,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Plan => "plan",
            Phase::Play => "play",
            Phase::Debrief => "debrief",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Offline,
    Idle,
    Rogue,
}

/// A Play interaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Boost,
    Upgrade(String),
    Isolate(String),
    Reconnect(String),
    Unknown(String),
}

impl Action {
    pub fn from_parts(action: &str, arg: Option<&str>) -> Self {
        match (action, arg) {
            ("boost", _) => Action::Boost,
            ("upgrade", Some(a)) => Action::Upgrade(a.into()),
            ("isolate", Some(a)) => Action::Isolate(a.into()),
            ("reconnect", Some(a)) => Action::Reconnect(a.into()),
            (other, _) => Action::Unknown(other.into()),
        }
    }

    pub fn from_entry(e: &InteractionEntry) -> Self {
        Self::from_parts(&e.action, e.arg.as_deref())
    }

    pub fn name(&self) -> &str {
        match self {
            Action::Boost => "boost",
            Action::Upgrade(_) => "upgrade",
            Action::Isolate(_) => "isolate",
            Action::Reconnect(_) => "reconnect",
            Action::Unknown(s) => s,
        }
    }

    pub fn arg(&self) -> Option<&str> {
        match self {
            Action::Upgrade(a) | Action::Isolate(a) | Action::Reconnect(a) => Some(a),
            _ => None,
        }
    }

    /// The `game/action` payload for this action.
    pub fn to_json(&self) -> Value {
        match self.arg() {
            Some(a) => json!({ "action": self.name(), "arg": a }),
            None => json!({ "action": self.name() }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    Busy,
    NoCharge,
    NoBudget,
    TooLate,
    Unknown,
    /// The broker is down; nothing can be coordinated.
    BrokerDown,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Refusal::Busy => "busy",
            Refusal::NoCharge => "no_charge",
            Refusal::NoBudget => "no_budget",
            Refusal::TooLate => "too_late",
            Refusal::Unknown => "unknown",
            Refusal::BrokerDown => "broker_down",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    WrongPhase { expected: Phase, actual: Phase },
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::WrongPhase { expected, actual } => {
                write!(
                    f,
                    "engine is in phase {}, expected {}",
                    actual.as_str(),
                    expected.as_str()
                )
            }
        }
    }
}

impl std::error::Error for EngineError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetTick {
    pub id: String,
    pub p_kw: f64,
    pub soc: Option<f64>,
    pub tank_kg: Option<f64>,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TickRecord {
    pub t: u32,
    pub harvested_kw: f64,
    pub demand_kw: f64,
    pub served_kw: f64,
    pub unmet_kw: f64,
    pub curtailed_kw: f64,
    pub charge_kw: f64,
    pub discharge_kw: f64,
    pub ely_kw: f64,
    pub fc_kw: f64,
    pub export_kw: f64,
    pub import_kw: f64,
    pub boost_kw: f64,
    pub co2_kg: f64,
    pub credits_kwh: f64,
    pub stars: u32,
    pub outage: bool,
    pub assets: Vec<AssetTick>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractionRecord {
    pub t: u32,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg: Option<String>,
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Refusal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttackOutcome {
    pub at: u32,
    pub vector: Vector,
    pub target: Option<String>,
    pub blocked_by: Option<String>,
    pub mitigated_by: Option<String>,
    pub ticks: u32,
    pub landed: bool,
    pub alert_t: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StarLoss {
    pub t: u32,
    pub cause: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundScore {
    pub stars: u32,
    pub efficiency: f64,
    pub co2_kg: f64,
    pub co2_cap_kg: f64,
    pub pass: bool,
    pub reasons: Vec<String>,
    pub contracts_met: u32,
    pub contracts_total: u32,
    pub target_efficiency: f64,
    pub harvested_kwh: f64,
    pub curtailed_kwh: f64,
    pub export_kwh: f64,
    pub losses_kwh: f64,
    pub leftover_kwh: f64,
    pub ticks_played: u32,
    pub ended_early: bool,
    pub budget: f64,
    pub budget_used: f64,
    pub star_losses: Vec<StarLoss>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub id: String,
    pub kind: String,
    pub slot: usize,
}

/// Everything a run produced. Serialised by `--json`; golden tests compare it byte for byte.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunOutput {
    pub scenario: String,
    pub plan: Plan,
    pub plan_cost: f64,
    pub interactions: Vec<InteractionRecord>,
    pub ticks: Vec<TickRecord>,
    pub attacks: Vec<AttackOutcome>,
    pub score: RoundScore,
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone)]
struct AssetState {
    soc_kwh: f64,
    tank_kg: f64,
    losses_kwh: f64,
}

#[derive(Debug, Clone)]
struct OwnedUpgrade {
    id: String,
    active_from: u32,
}

#[derive(Debug, Clone)]
struct ActiveAttack {
    vector: Vector,
    target: Option<usize>,
    from: u32,
    until: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    Offline,
    Idle,
    Rogue,
}

pub struct Engine {
    sc: Arc<Scenario>,
    phase: Phase,
    plan: Option<Arc<ValidPlan>>,
    t: u32,
    st: Vec<AssetState>,
    stars: u32,
    star_losses: Vec<StarLoss>,
    co2_kg: f64,
    co2_star_lost: bool,
    credits_kwh: f64,
    boost_charges: u32,
    boost_armed: bool,
    export_accum_kwh: f64,
    budget_left: f64,
    upgrades: Vec<OwnedUpgrade>,
    busy_until: u32,
    active_attacks: Vec<ActiveAttack>,
    attacks: Vec<AttackOutcome>,
    alerts: BTreeMap<usize, u32>,
    contracts_met: u32,
    contracts_missed: u32,
    records: Vec<TickRecord>,
    interactions: Vec<InteractionRecord>,
    outbox: Vec<Message>,
    harvested_kwh: f64,
    curtailed_kwh: f64,
    export_kwh: f64,
    import_kwh: f64,
    boost_kwh: f64,
    stored_init_kwh: f64,
    findings: Vec<Finding>,
    score: Option<RoundScore>,
}

impl Engine {
    /// A new level in the Plan phase.
    pub fn new(sc: Scenario) -> Self {
        let mut e = Self {
            boost_charges: sc.boost.charges,
            budget_left: sc.round.budget,
            stars: sc.round.stars,
            sc: Arc::new(sc),
            phase: Phase::Plan,
            plan: None,
            t: 0,
            st: Vec::new(),
            star_losses: Vec::new(),
            co2_kg: 0.0,
            co2_star_lost: false,
            credits_kwh: 0.0,
            boost_armed: false,
            export_accum_kwh: 0.0,
            upgrades: Vec::new(),
            busy_until: 0,
            active_attacks: Vec::new(),
            attacks: Vec::new(),
            alerts: BTreeMap::new(),
            contracts_met: 0,
            contracts_missed: 0,
            records: Vec::new(),
            interactions: Vec::new(),
            outbox: Vec::new(),
            harvested_kwh: 0.0,
            curtailed_kwh: 0.0,
            export_kwh: 0.0,
            import_kwh: 0.0,
            boost_kwh: 0.0,
            stored_init_kwh: 0.0,
            findings: Vec::new(),
            score: None,
        };
        e.publish_phase();
        e
    }

    // ------------------------------------------------------------ accessors

    pub fn scenario(&self) -> &Scenario {
        &self.sc
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    /// The next tick to play (= ticks played so far).
    pub fn t(&self) -> u32 {
        self.t
    }
    pub fn ticks(&self) -> u32 {
        self.sc.ticks()
    }
    pub fn plan(&self) -> Option<&ValidPlan> {
        self.plan.as_deref()
    }
    pub fn records(&self) -> &[TickRecord] {
        &self.records
    }
    pub fn interactions(&self) -> &[InteractionRecord] {
        &self.interactions
    }
    pub fn attacks(&self) -> &[AttackOutcome] {
        &self.attacks
    }
    pub fn stars(&self) -> u32 {
        self.stars
    }
    pub fn boost_charges(&self) -> u32 {
        self.boost_charges
    }
    pub fn boost_armed(&self) -> bool {
        self.boost_armed
    }
    pub fn busy_until(&self) -> u32 {
        self.busy_until
    }
    pub fn budget_left(&self) -> f64 {
        self.budget_left
    }
    pub fn co2_kg(&self) -> f64 {
        self.co2_kg
    }
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }
    pub fn score(&self) -> Option<&RoundScore> {
        self.score.as_ref()
    }

    /// Messages produced since the last drain, in publish order.
    pub fn drain_outbox(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.outbox)
    }

    /// Is upgrade `id` owned and active at tick `t`?
    pub fn upgrade_active(&self, id: &str, t: u32) -> bool {
        self.upgrades
            .iter()
            .any(|u| u.id == id && u.active_from <= t)
    }

    /// Ticks `t+1 ..= t+lookahead` of actual demand and production (kW at planned capacity).
    pub fn actual_window(&self) -> Value {
        self.window_from(self.t, self.t.saturating_sub(1))
    }

    /// Window starting at tick `from`, as known after tick `now` was played.
    fn window_from(&self, from: u32, now: u32) -> Value {
        let Some(plan) = self.plan.as_deref() else {
            return json!({});
        };
        let to = (from + plan.tier.lookahead_ticks).min(self.ticks());
        let mut demand = Vec::new();
        let mut pv = Vec::new();
        let mut wind = Vec::new();
        for k in from..to {
            demand.push(self.sc.actual.demand[k as usize]);
            let (mut p, mut w) = (0.0, 0.0);
            for a in &plan.assets {
                if let AssetSpec::Source { curve, capacity_kw } = &a.spec {
                    // Only outages already under way are visible.
                    let known_down = self.sc.events.iter().any(|e| {
                        matches!(&e.kind, EventKind::Outage { asset_catalog } if *asset_catalog == a.catalog) && e.at <= now && e.covers(k)
                    });
                    let kw = if known_down {
                        0.0
                    } else {
                        self.sc.actual.source(*curve)[k as usize] * capacity_kw
                    };
                    match curve {
                        CurveId::Pv => p += kw,
                        CurveId::Wind => w += kw,
                    }
                }
            }
            pv.push(p);
            wind.push(w);
        }
        json!({ "from": from, "demand": demand, "pv": pv, "wind": wind })
    }

    // ------------------------------------------------------------ plan

    /// Validate and accept a plan; enters Play. Returns the asset manifest.
    pub fn submit_plan(
        &mut self,
        plan: &Plan,
        name: &str,
    ) -> Result<Vec<ManifestEntry>, LoadError> {
        if self.phase != Phase::Plan {
            return Err(LoadError::new(
                name,
                "",
                format!("cannot submit a plan in phase {}", self.phase.as_str()),
            ));
        }
        let vp = plan.validate(&self.sc, name)?;
        self.budget_left = self.sc.round.budget - vp.cost;
        self.st = vp
            .assets
            .iter()
            .map(|a| AssetState {
                soc_kwh: match a.spec {
                    AssetSpec::Storage {
                        capacity_kwh,
                        soc_init,
                        ..
                    } => capacity_kwh * soc_init,
                    _ => 0.0,
                },
                tank_kg: 0.0,
                losses_kwh: 0.0,
            })
            .collect();
        self.stored_init_kwh = self.st.iter().map(|s| s.soc_kwh).sum();
        self.credits_kwh = vp
            .assets
            .iter()
            .filter_map(|a| match a.spec {
                AssetSpec::Interconnector { credits_init, .. } => Some(credits_init),
                _ => None,
            })
            .fold(0.0, f64::max);
        self.upgrades = vp
            .upgrades
            .iter()
            .map(|u| OwnedUpgrade {
                id: u.clone(),
                active_from: 0,
            })
            .collect();
        let manifest = vp
            .assets
            .iter()
            .enumerate()
            .map(|(slot, a)| ManifestEntry {
                id: a.id.clone(),
                kind: a.spec.kind().into(),
                slot,
            })
            .collect();

        let plan_json = serde_json::to_value(plan).unwrap_or(Value::Null);
        self.push(topics::GAME_PLAN, &plan_json, Qos::AtLeastOnce, true);
        let demand = json!(self.sc.forecast.demand);
        self.push(topics::FORECAST_DEMAND, &demand, Qos::AtLeastOnce, true);
        let (mut pv, mut wind) = (
            vec![0.0; self.ticks() as usize],
            vec![0.0; self.ticks() as usize],
        );
        for a in &vp.assets {
            if let AssetSpec::Source { curve, capacity_kw } = &a.spec {
                let (dst, src) = match curve {
                    CurveId::Pv => (&mut pv, &self.sc.forecast.pv),
                    CurveId::Wind => (&mut wind, &self.sc.forecast.wind),
                };
                for (d, s) in dst.iter_mut().zip(src) {
                    *d += s * capacity_kw;
                }
            }
        }
        self.push(topics::FORECAST_PV, &json!(pv), Qos::AtLeastOnce, true);
        self.push(topics::FORECAST_WIND, &json!(wind), Qos::AtLeastOnce, true);
        self.plan = Some(Arc::new(vp));
        self.phase = Phase::Play;
        self.publish_security_status();
        self.publish_phase();
        Ok(manifest)
    }

    // ------------------------------------------------------------ actions

    /// Apply a Play interaction issued after the last played tick. Every call is
    /// logged, accepted or refused.
    pub fn act(&mut self, action: &Action) -> Result<(), Refusal> {
        let t = self.t.saturating_sub(1);
        let res = self.try_act(action, t);
        self.interactions.push(InteractionRecord {
            t,
            action: action.name().to_string(),
            arg: action.arg().map(str::to_string),
            accepted: res.is_ok(),
            reason: res.err(),
        });
        if res.is_ok() {
            self.publish_security_status();
        }
        res
    }

    fn broker_down_at(&self, t: u32) -> bool {
        self.active_attacks
            .iter()
            .any(|a| a.vector == Vector::BrokerOutage && a.from <= t && t < a.until)
    }

    fn try_act(&mut self, action: &Action, t: u32) -> Result<(), Refusal> {
        if self.phase != Phase::Play || self.t == 0 {
            return Err(Refusal::Unknown);
        }
        if self.broker_down_at(t) {
            return Err(Refusal::BrokerDown);
        }
        if t < self.busy_until {
            return Err(Refusal::Busy);
        }
        match action {
            Action::Boost => {
                if self.boost_armed {
                    return Err(Refusal::Unknown);
                }
                if self.boost_charges == 0 {
                    return Err(Refusal::NoCharge);
                }
                self.boost_armed = true;
                Ok(())
            }
            Action::Upgrade(id) => {
                let up = self.sc.upgrade(id).cloned().ok_or(Refusal::Unknown)?;
                if self.upgrades.iter().any(|u| &u.id == id) {
                    return Err(Refusal::Unknown);
                }
                let in_progress = self
                    .active_attacks
                    .iter()
                    .any(|a| a.from <= t && t < a.until && up.covers(a.vector));
                if in_progress {
                    return Err(Refusal::TooLate);
                }
                if up.cost > self.budget_left + EPS {
                    return Err(Refusal::NoBudget);
                }
                self.budget_left -= up.cost;
                self.busy_until = t + up.setup_ticks;
                self.upgrades.push(OwnedUpgrade {
                    id: id.clone(),
                    active_from: t + up.setup_ticks,
                });
                Ok(())
            }
            // Zones are M1.
            Action::Isolate(_) | Action::Reconnect(_) | Action::Unknown(_) => Err(Refusal::Unknown),
        }
    }

    // ------------------------------------------------------------ tick

    /// Play one tick. Returns the tick index just played.
    pub fn step(&mut self) -> Result<u32, EngineError> {
        if self.phase != Phase::Play {
            return Err(EngineError::WrongPhase {
                expected: Phase::Play,
                actual: self.phase,
            });
        }
        let plan = Arc::clone(self.plan.as_ref().expect("plan is set in Play"));
        let sc = Arc::clone(&self.sc);
        let t = self.t;
        let dt = sc.dt_h();
        let last = t + 1 == sc.ticks();
        let n = plan.assets.len();

        // 1. events: outages, alerts, attacks
        let mut outage_down = vec![false; n];
        for ev in &sc.events {
            if let EventKind::Outage { asset_catalog } = &ev.kind {
                if ev.covers(t) {
                    for (i, a) in plan.assets.iter().enumerate() {
                        if &a.catalog == asset_catalog {
                            outage_down[i] = true;
                            if ev.at == t {
                                self.push(
                                    topics::GRID_EVENT,
                                    &json!({"kind": "outage", "asset": a.id, "ticks": ev.ticks}),
                                    Qos::AtLeastOnce,
                                    false,
                                );
                            }
                        }
                    }
                }
            }
        }
        let ids_warn = sc
            .security_catalog()
            .filter(|(id, s)| s.warn_ticks.is_some() && self.upgrade_active(id, t))
            .filter_map(|(_, s)| s.warn_ticks)
            .max();
        if let Some(warn) = ids_warn {
            for (ei, ev) in sc.events.iter().enumerate() {
                if let EventKind::Attack { vector, .. } = &ev.kind {
                    if ev.at >= warn && ev.at - warn == t {
                        self.alerts.insert(ei, t);
                        self.push(
                            topics::SECURITY_ALERT,
                            &json!({"vector": vector, "eta_ticks": warn}),
                            Qos::AtLeastOnce,
                            false,
                        );
                    }
                }
            }
        }
        self.active_attacks.retain(|a| a.until > t);
        for ei in 0..sc.events.len() {
            if sc.events[ei].at == t && matches!(sc.events[ei].kind, EventKind::Attack { .. }) {
                self.resolve_attack(ei, &plan);
            }
        }
        let broker_out = self.broker_down_at(t);
        let rogue_client = self
            .active_attacks
            .iter()
            .any(|a| a.vector == Vector::RogueClient && a.from <= t);
        let mut rogue = vec![false; n];
        for a in &self.active_attacks {
            if let (Vector::CredentialTheft, Some(i)) = (a.vector, a.target) {
                rogue[i] = true;
            }
        }
        let modes: Vec<Mode> = plan
            .assets
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let source = matches!(a.spec, AssetSpec::Source { .. });
                if outage_down[i] {
                    Mode::Offline
                } else if broker_out {
                    if source {
                        Mode::Normal
                    } else {
                        Mode::Idle
                    }
                } else if rogue[i] {
                    if source {
                        Mode::Offline
                    } else {
                        Mode::Rogue
                    }
                } else if rogue_client && !source {
                    Mode::Idle
                } else {
                    Mode::Normal
                }
            })
            .collect();

        // 2. production, rogue behaviour
        let mut p = vec![0.0; n];
        let mut harvested = 0.0;
        for (i, a) in plan.assets.iter().enumerate() {
            if let (AssetSpec::Source { curve, capacity_kw }, Mode::Normal) = (&a.spec, modes[i]) {
                let kw = sc.actual.source(*curve)[t as usize] * capacity_kw;
                p[i] = kw;
                harvested += kw;
            }
        }
        let demand = sc.actual.demand[t as usize];
        let (mut charge_kw, mut discharge_kw, mut ely_kw, mut fc_kw) = (0.0, 0.0, 0.0, 0.0);
        let (mut export_kw, mut import_kw, mut boost_kw, mut curtailed) = (0.0, 0.0, 0.0, 0.0);
        let mut co2_tick = 0.0;
        let mut rogue_import = 0.0;
        for (i, a) in plan.assets.iter().enumerate() {
            if modes[i] != Mode::Rogue {
                continue;
            }
            match &a.spec {
                AssetSpec::Storage {
                    max_kw, eta_out, ..
                } => {
                    let kw = max_kw.min(self.st[i].soc_kwh * eta_out / dt).max(0.0);
                    self.st[i].soc_kwh -= kw * dt / eta_out;
                    self.st[i].losses_kwh += kw * dt * (1.0 / eta_out - 1.0);
                    p[i] += kw;
                    discharge_kw += kw;
                    curtailed += kw;
                }
                AssetSpec::Interconnector {
                    import_kw: im,
                    co2_per_kwh_import,
                    ..
                } => {
                    p[i] += im;
                    import_kw += im;
                    rogue_import += im;
                    co2_tick += im * dt * co2_per_kwh_import;
                }
                _ => {}
            }
        }

        // 3./4. greedy dispatch
        let net = harvested + rogue_import - demand;
        let mut unmet = 0.0;
        if net >= 0.0 {
            let mut rem = net;
            for &i in &plan.charge {
                if modes[i] != Mode::Normal || rem <= EPS {
                    continue;
                }
                let take = match &plan.assets[i].spec {
                    AssetSpec::Storage {
                        capacity_kwh,
                        max_kw,
                        eta_in,
                        ..
                    } => {
                        let head = ((capacity_kwh - self.st[i].soc_kwh) / eta_in / dt).max(0.0);
                        let take = rem.min(*max_kw).min(head);
                        self.st[i].soc_kwh += take * dt * eta_in;
                        self.st[i].losses_kwh += take * dt * (1.0 - eta_in);
                        charge_kw += take;
                        take
                    }
                    AssetSpec::Electrolyser {
                        max_kw,
                        tank_kg,
                        eta,
                        kwh_per_kg,
                    } => {
                        let head =
                            ((tank_kg - self.st[i].tank_kg) * kwh_per_kg / eta / dt).max(0.0);
                        let take = rem.min(*max_kw).min(head);
                        self.st[i].tank_kg += take * dt * eta / kwh_per_kg;
                        self.st[i].losses_kwh += take * dt * (1.0 - eta);
                        ely_kw += take;
                        take
                    }
                    AssetSpec::Interconnector {
                        export_kw: cap,
                        export_credit_ratio,
                        ..
                    } => {
                        let take = rem.min(*cap);
                        self.credits_kwh += take * dt * export_credit_ratio;
                        export_kw += take;
                        self.export_accum_kwh += take * dt;
                        if sc.boost.credit_kwh > 0.0 {
                            while self.export_accum_kwh + EPS >= sc.boost.credit_kwh {
                                self.export_accum_kwh -= sc.boost.credit_kwh;
                                self.boost_charges += 1;
                            }
                        }
                        take
                    }
                    _ => 0.0,
                };
                p[i] -= take;
                rem -= take;
            }
            curtailed += rem.max(0.0);
        } else {
            let mut rem = -net;
            for &i in &plan.discharge {
                if modes[i] != Mode::Normal || rem <= EPS {
                    continue;
                }
                let give = match &plan.assets[i].spec {
                    AssetSpec::Storage {
                        capacity_kwh,
                        max_kw,
                        reserve,
                        eta_out,
                        ..
                    } => {
                        let floor = if last { 0.0 } else { reserve * capacity_kwh };
                        let avail = ((self.st[i].soc_kwh - floor).max(0.0)) * eta_out / dt;
                        let give = rem.min(*max_kw).min(avail);
                        self.st[i].soc_kwh -= give * dt / eta_out;
                        self.st[i].losses_kwh += give * dt * (1.0 / eta_out - 1.0);
                        discharge_kw += give;
                        give
                    }
                    AssetSpec::FuelCell {
                        max_kw,
                        eta,
                        kwh_per_kg,
                    } => {
                        let total_kg: f64 = self.st.iter().map(|s| s.tank_kg).sum();
                        let give = rem.min(*max_kw).min(total_kg * kwh_per_kg * eta / dt);
                        let mut kg = give * dt / (kwh_per_kg * eta);
                        self.st[i].losses_kwh += kg * kwh_per_kg * (1.0 - eta);
                        for s in &mut self.st {
                            let d = kg.min(s.tank_kg);
                            s.tank_kg -= d;
                            kg -= d;
                        }
                        fc_kw += give;
                        give
                    }
                    AssetSpec::Interconnector {
                        import_kw: cap,
                        eta,
                        co2_per_kwh_import,
                        ..
                    } => {
                        let give = rem.min(*cap).min(self.credits_kwh * eta / dt).max(0.0);
                        self.credits_kwh = (self.credits_kwh - give * dt / eta).max(0.0);
                        co2_tick += give * dt * co2_per_kwh_import;
                        import_kw += give;
                        give
                    }
                    _ => 0.0,
                };
                p[i] += give;
                rem -= give;
            }
            unmet = rem.max(0.0);
        }
        if self.boost_armed && !broker_out {
            self.boost_armed = false;
            self.boost_charges = self.boost_charges.saturating_sub(1);
            boost_kw = unmet.min(sc.boost.kw);
            unmet -= boost_kw;
            co2_tick += boost_kw * dt * sc.boost.co2_per_kwh;
        }
        if unmet < EPS {
            unmet = 0.0;
        }

        // 5. contracts
        for c in &sc.contracts {
            if c.by_tick != t {
                continue;
            }
            let total: f64 = self.st.iter().map(|s| s.tank_kg).sum();
            if total + EPS >= c.kg {
                let mut kg = c.kg;
                for s in &mut self.st {
                    let d = kg.min(s.tank_kg);
                    s.tank_kg -= d;
                    kg -= d;
                }
                self.contracts_met += 1;
            } else {
                self.contracts_missed += 1;
                self.lose_star(
                    t,
                    format!(
                        "missed contract: {} kg {} by {}",
                        c.kg,
                        c.product,
                        crate::clock(t, sc.round.tick_minutes)
                    ),
                );
            }
        }

        // 6. score the tick
        let outage = demand > 0.0 && unmet > sc.round.outage_tolerance * demand;
        if outage {
            self.lose_star(t, format!("outage: {unmet:.0} kW unmet"));
        }
        if let Some(a) = self.attacks.iter().find(|a| {
            a.at == t && a.landed && a.mitigated_by.is_none() && a.vector.costs_star_directly()
        }) {
            let cause = format!("{} landed", a.vector);
            self.lose_star(t, cause);
        }
        self.co2_kg += co2_tick;
        if !self.co2_star_lost && self.co2_kg > sc.round.co2_cap_kg + EPS {
            self.co2_star_lost = true;
            self.lose_star(t, format!("CO2 over the {} kg cap", sc.round.co2_cap_kg));
        }
        self.harvested_kwh += harvested * dt;
        self.curtailed_kwh += curtailed * dt;
        self.export_kwh += export_kw * dt;
        self.import_kwh += import_kw * dt;
        self.boost_kwh += boost_kw * dt;

        let assets = plan
            .assets
            .iter()
            .enumerate()
            .map(|(i, a)| AssetTick {
                id: a.id.clone(),
                p_kw: p[i],
                soc: match a.spec {
                    AssetSpec::Storage { capacity_kwh, .. } if capacity_kwh > 0.0 => {
                        Some(self.st[i].soc_kwh / capacity_kwh)
                    }
                    AssetSpec::Storage { .. } => Some(0.0),
                    _ => None,
                },
                tank_kg: matches!(a.spec, AssetSpec::Electrolyser { .. })
                    .then_some(self.st[i].tank_kg),
                status: match modes[i] {
                    Mode::Normal => Status::Ok,
                    Mode::Offline => Status::Offline,
                    Mode::Idle => Status::Idle,
                    Mode::Rogue => Status::Rogue,
                },
            })
            .collect();
        let rec = TickRecord {
            t,
            harvested_kw: harvested,
            demand_kw: demand,
            served_kw: demand - unmet,
            unmet_kw: unmet,
            curtailed_kw: curtailed,
            charge_kw,
            discharge_kw,
            ely_kw,
            fc_kw,
            export_kw,
            import_kw,
            boost_kw,
            co2_kg: self.co2_kg,
            credits_kwh: self.credits_kwh,
            stars: self.stars,
            outage,
            assets,
        };
        self.publish_tick(&rec, &plan, broker_out);
        self.records.push(rec);
        self.t += 1;
        if self.stars == 0 || self.t >= sc.ticks() {
            self.finish();
        }
        Ok(t)
    }

    fn lose_star(&mut self, t: u32, cause: String) {
        if self.stars > 0 {
            self.stars -= 1;
            self.star_losses.push(StarLoss { t, cause });
        }
    }

    fn resolve_attack(&mut self, ei: usize, plan: &ValidPlan) {
        let ev = self.sc.events[ei].clone();
        let EventKind::Attack {
            vector,
            target_catalog,
        } = &ev.kind
        else {
            return;
        };
        let t = ev.at;
        let target = match vector {
            Vector::CredentialTheft => target_catalog
                .as_ref()
                .and_then(|c| plan.assets.iter().position(|a| &a.catalog == c)),
            _ => None,
        };
        let blocked_by = self
            .sc
            .security_catalog()
            .find(|(id, s)| s.defeats.contains(vector) && self.upgrade_active(id, t))
            .map(|(id, _)| id.to_string());
        let mut ticks = ev.ticks;
        let mut mitigated_by = None;
        if blocked_by.is_none() && *vector == Vector::BrokerOutage {
            if let Some((id, f)) = self
                .sc
                .security_catalog()
                .find(|(id, s)| s.failover_ticks.is_some() && self.upgrade_active(id, t))
                .map(|(id, s)| (id, s.failover_ticks))
            {
                ticks = f.unwrap_or(ticks).min(ev.ticks);
                mitigated_by = Some(id.to_string());
            }
        }
        let landed = blocked_by.is_none()
            && ticks > 0
            && (*vector != Vector::CredentialTheft || target.is_some());
        let target_id = target.map(|i| plan.assets[i].id.clone());
        let mut payload = json!({"vector": vector, "target": target_id, "blocked_by": blocked_by, "ticks": ticks});
        if let Some(m) = &mitigated_by {
            payload["mitigated_by"] = json!(m);
        }
        self.push(topics::SECURITY_ATTACK, &payload, Qos::AtLeastOnce, false);
        if landed {
            self.active_attacks.push(ActiveAttack {
                vector: *vector,
                target,
                from: t,
                until: t + ticks,
            });
        }
        self.attacks.push(AttackOutcome {
            at: t,
            vector: *vector,
            target: target_id,
            blocked_by,
            mitigated_by,
            ticks,
            landed,
            alert_t: self.alerts.get(&ei).copied(),
        });
    }

    // ------------------------------------------------------------ scoring

    /// Day-end gain in stored energy (kWh), credited to efficiency.
    fn leftover_kwh(&self) -> f64 {
        // Only a full day earns the leftover credit; an early stop is a fail anyway.
        if self.t < self.sc.ticks() {
            return 0.0;
        }
        let soc: f64 = self.st.iter().map(|s| s.soc_kwh).sum();
        let tank: f64 = self.st.iter().map(|s| s.tank_kg).sum();
        let (fc_eta, kwh_per_kg) = self
            .sc
            .catalog
            .iter()
            .find_map(|c| match c.kind {
                CatalogKind::FuelCell {
                    eta, kwh_per_kg, ..
                } => Some((eta, kwh_per_kg)),
                _ => None,
            })
            .unwrap_or((0.5, 33.33));
        (soc - self.stored_init_kwh + tank * kwh_per_kg * fc_eta).max(0.0)
    }

    fn losses_kwh(&self) -> f64 {
        self.st.iter().map(|s| s.losses_kwh).sum()
    }

    /// `1 - (curtailed + export + losses - leftover) / harvested`, clamped to 0..1.
    pub fn efficiency(&self) -> f64 {
        if self.harvested_kwh <= EPS {
            return 1.0;
        }
        let waste = self.curtailed_kwh + self.export_kwh + self.losses_kwh() - self.leftover_kwh();
        (1.0 - waste / self.harvested_kwh).clamp(0.0, 1.0)
    }

    pub fn device_losses(&self) -> Vec<(String, f64)> {
        match self.plan.as_deref() {
            Some(p) => p
                .assets
                .iter()
                .zip(&self.st)
                .map(|(a, s)| (a.id.clone(), s.losses_kwh))
                .collect(),
            None => Vec::new(),
        }
    }

    pub fn import_kwh(&self) -> f64 {
        self.import_kwh
    }
    pub fn boost_kwh(&self) -> f64 {
        self.boost_kwh
    }

    pub fn live_score(&self) -> Value {
        json!({"stars": self.stars, "efficiency": self.efficiency(), "co2_kg": self.co2_kg, "boost_charges": self.boost_charges})
    }

    fn finish(&mut self) {
        let sc = &self.sc;
        let total = sc.contracts.len() as u32;
        let eff = self.efficiency();
        let mut reasons = Vec::new();
        if self.stars == 0 {
            reasons.push("stars".to_string());
        }
        if eff + EPS < sc.round.target_efficiency {
            reasons.push("efficiency".to_string());
        }
        if self.contracts_met < total {
            reasons.push("contract".to_string());
        }
        let score = RoundScore {
            stars: self.stars,
            efficiency: eff,
            co2_kg: self.co2_kg,
            co2_cap_kg: sc.round.co2_cap_kg,
            pass: reasons.is_empty(),
            reasons,
            contracts_met: self.contracts_met,
            contracts_total: total,
            target_efficiency: sc.round.target_efficiency,
            harvested_kwh: self.harvested_kwh,
            curtailed_kwh: self.curtailed_kwh,
            export_kwh: self.export_kwh,
            losses_kwh: self.losses_kwh(),
            leftover_kwh: self.leftover_kwh(),
            ticks_played: self.t,
            ended_early: self.t < sc.ticks(),
            budget: sc.round.budget,
            budget_used: sc.round.budget - self.budget_left,
            star_losses: self.star_losses.clone(),
        };
        self.findings = debrief::findings(self);
        self.phase = Phase::Debrief;
        let round = json!({
            "stars": score.stars, "efficiency": score.efficiency, "co2_kg": score.co2_kg,
            "pass": score.pass, "contracts_met": score.contracts_met,
        });
        self.score = Some(score);
        self.push(topics::SCORE_ROUND, &round, Qos::AtLeastOnce, true);
        for (n, f) in self.findings.clone().iter().enumerate() {
            let v = serde_json::to_value(f).unwrap_or(Value::Null);
            self.push(&topics::debrief(n), &v, Qos::AtLeastOnce, true);
        }
        self.publish_phase();
    }

    /// The whole run, for `--json` and golden tests. Only meaningful in Debrief.
    pub fn run_output(&self) -> Option<RunOutput> {
        let plan = self.plan.as_deref()?;
        Some(RunOutput {
            scenario: self.sc.round.id.clone(),
            plan: plan.plan.clone(),
            plan_cost: plan.cost,
            interactions: self.interactions.clone(),
            ticks: self.records.clone(),
            attacks: self.attacks.clone(),
            score: self.score.clone()?,
            findings: self.findings.clone(),
        })
    }

    // ------------------------------------------------------------ publishing

    fn push(&mut self, topic: &str, v: &Value, qos: Qos, retain: bool) {
        self.outbox.push(Message::json(topic, v, qos, retain));
    }

    fn publish_phase(&mut self) {
        let v = json!({"phase": self.phase.as_str(), "level": self.sc.round.id});
        self.push(topics::GAME_PHASE, &v, Qos::AtLeastOnce, true);
    }

    fn publish_security_status(&mut self) {
        self.publish_security_status_at(self.t);
    }

    fn publish_tick(&mut self, r: &TickRecord, plan: &ValidPlan, broker_out: bool) {
        let tm = self.sc.round.tick_minutes;
        self.push(
            topics::SIM_TICK,
            &json!({"t": r.t, "minute": r.t * tm, "dt": tm}),
            Qos::AtMostOnce,
            true,
        );
        if !broker_out {
            let w = self.window_from(r.t + 1, r.t);
            self.push(topics::FORECAST_WINDOW, &w, Qos::AtMostOnce, true);
        }
        for (a, at) in plan.assets.iter().zip(&r.assets) {
            let v = json!({"kind": a.catalog, "p_kw": at.p_kw, "soc": at.soc, "tank_kg": at.tank_kg, "status": at.status});
            self.push(&topics::plant_state(&a.id), &v, Qos::AtLeastOnce, true);
        }
        self.push(
            topics::GRID_DEMAND,
            &json!({"kw": r.demand_kw}),
            Qos::AtMostOnce,
            false,
        );
        let supply = r.harvested_kw + r.discharge_kw + r.fc_kw + r.import_kw + r.boost_kw;
        let bal = json!({
            "supply_kw": supply, "demand_kw": r.demand_kw, "unmet_kw": r.unmet_kw,
            "curtailed_kw": r.curtailed_kw, "export_kw": r.export_kw, "import_kw": r.import_kw,
        });
        self.push(topics::GRID_BALANCE, &bal, Qos::AtMostOnce, true);
        let live = self.live_score();
        self.push(topics::SCORE_LIVE, &live, Qos::AtMostOnce, true);
        self.publish_security_status_at(r.t + 1);
    }

    fn publish_security_status_at(&mut self, next: u32) {
        let mut m = serde_json::Map::new();
        let ids: Vec<String> = self
            .sc
            .security_catalog()
            .map(|(id, _)| id.to_string())
            .collect();
        for id in ids {
            m.insert(id.clone(), json!(self.upgrade_active(&id, next)));
        }
        m.insert("busy_until".into(), json!(self.busy_until));
        self.push(
            topics::SECURITY_STATUS,
            &Value::Object(m),
            Qos::AtLeastOnce,
            true,
        );
    }
}
