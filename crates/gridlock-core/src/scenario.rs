//! Scenario loading and validation. Curves arrive as strings keyed by the path
//! written in the scenario file; the core never opens a file.

use crate::error::LoadError;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

// ---------------------------------------------------------------- raw TOML

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundSpec {
    pub id: String,
    pub name: String,
    pub tick_minutes: u32,
    pub budget: f64,
    pub target_efficiency: f64,
    pub co2_cap_kg: f64,
    pub stars: u32,
    #[serde(default = "default_tolerance")]
    pub outage_tolerance: f64,
}

fn default_tolerance() -> f64 {
    0.02
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoostSpec {
    pub kw: f64,
    pub charges: u32,
    pub co2_per_kwh: f64,
    /// Export kWh per extra charge; 0 disables.
    #[serde(default)]
    pub credit_kwh: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurvePaths {
    pub demand: String,
    pub pv: String,
    pub wind: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawActual {
    demand: String,
    pv: String,
    wind: String,
    #[serde(default)]
    events: Vec<RawEvent>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEvent {
    at: u32,
    kind: String,
    ticks: u32,
    asset_catalog: Option<String>,
    vector: Option<String>,
    target_catalog: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForecastTier {
    pub tier: u32,
    pub lookahead_ticks: u32,
    pub cost: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCatalog {
    id: String,
    kind: String,
    curve: Option<String>,
    cost_per_kw: Option<f64>,
    max_capacity_kw: Option<f64>,
    eta_in: Option<f64>,
    eta_out: Option<f64>,
    soc_init: Option<f64>,
    cost_per_kwh: Option<f64>,
    input: Option<String>,
    output: Option<String>,
    eta: Option<f64>,
    kwh_per_kg: Option<f64>,
    tank_cost_per_kg: Option<f64>,
    co2_per_kwh_import: Option<f64>,
    export_credit_ratio: Option<f64>,
    max_kw: Option<f64>,
    credits_init: Option<f64>,
    cost: Option<f64>,
    setup_ticks: Option<u32>,
    defeats: Option<Vec<String>>,
    failover_ticks: Option<u32>,
    warn_ticks: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    pub product: String,
    pub kg: f64,
    pub by_tick: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecuritySpec {
    #[serde(default = "default_spread")]
    pub spread_ticks: u32,
    #[serde(default = "default_isolate")]
    pub isolate_busy_ticks: u32,
    #[serde(default = "default_conduit")]
    pub conduit_cost: f64,
}

fn default_spread() -> u32 {
    4
}
fn default_isolate() -> u32 {
    2
}
fn default_conduit() -> f64 {
    5.0
}

impl Default for SecuritySpec {
    fn default() -> Self {
        Self {
            spread_ticks: 4,
            isolate_busy_ticks: 2,
            conduit_cost: 5.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScenario {
    round: RoundSpec,
    boost: BoostSpec,
    forecast: CurvePaths,
    actual: RawActual,
    #[serde(default)]
    forecast_tiers: Vec<ForecastTier>,
    catalog: Vec<RawCatalog>,
    #[serde(default)]
    contracts: Vec<Contract>,
    #[serde(default)]
    security: Option<SecuritySpec>,
}

// ---------------------------------------------------------------- typed

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vector {
    CredentialTheft,
    RogueClient,
    BrokerOutage,
}

impl Vector {
    pub const ALL: [Vector; 3] = [
        Vector::CredentialTheft,
        Vector::RogueClient,
        Vector::BrokerOutage,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Vector::CredentialTheft => "credential_theft",
            Vector::RogueClient => "rogue_client",
            Vector::BrokerOutage => "broker_outage",
        }
    }

    /// Whether a landed attack of this vector costs a star by itself
    /// (decided 2026-11-05: only the broker outage does).
    pub fn costs_star_directly(self) -> bool {
        matches!(self, Vector::BrokerOutage)
    }
}

impl fmt::Display for Vector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Vector {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Vector::ALL.into_iter().find(|v| v.as_str() == s).ok_or(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CurveId {
    Pv,
    Wind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CatalogKind {
    Source {
        curve: CurveId,
        cost_per_kw: f64,
        max_capacity_kw: f64,
    },
    Storage {
        eta_in: f64,
        eta_out: f64,
        soc_init: f64,
        cost_per_kwh: f64,
        cost_per_kw: f64,
    },
    Electrolyser {
        eta: f64,
        kwh_per_kg: f64,
        cost_per_kw: f64,
        tank_cost_per_kg: f64,
    },
    FuelCell {
        eta: f64,
        kwh_per_kg: f64,
        cost_per_kw: f64,
    },
    Interconnector {
        eta: f64,
        co2_per_kwh_import: f64,
        export_credit_ratio: f64,
        cost_per_kw: f64,
        max_kw: f64,
        credits_init: f64,
    },
    Security(SecurityUpgrade),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SecurityUpgrade {
    pub cost: f64,
    pub setup_ticks: u32,
    pub defeats: Vec<Vector>,
    pub failover_ticks: Option<u32>,
    pub warn_ticks: Option<u32>,
}

impl SecurityUpgrade {
    /// Blocks or shortens attacks of this vector.
    pub fn covers(&self, v: Vector) -> bool {
        self.defeats.contains(&v) || (v == Vector::BrokerOutage && self.failover_ticks.is_some())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CatalogEntry {
    pub id: String,
    pub kind: CatalogKind,
}

impl CatalogEntry {
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            CatalogKind::Source { .. } => "source",
            CatalogKind::Storage { .. } => "storage",
            CatalogKind::Electrolyser { .. } | CatalogKind::FuelCell { .. } => "converter",
            CatalogKind::Interconnector { .. } => "interconnector",
            CatalogKind::Security(_) => "security",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventKind {
    Outage {
        asset_catalog: String,
    },
    Attack {
        vector: Vector,
        target_catalog: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub at: u32,
    pub ticks: u32,
    pub kind: EventKind,
}

impl Event {
    pub fn covers(&self, t: u32) -> bool {
        t >= self.at && t < self.at + self.ticks
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Curves {
    pub demand: Vec<f64>,
    pub pv: Vec<f64>,
    pub wind: Vec<f64>,
}

impl Curves {
    pub fn source(&self, c: CurveId) -> &[f64] {
        match c {
            CurveId::Pv => &self.pv,
            CurveId::Wind => &self.wind,
        }
    }
}

/// A fully validated scenario.
#[derive(Debug, Clone, PartialEq)]
pub struct Scenario {
    pub round: RoundSpec,
    pub boost: BoostSpec,
    pub forecast: Curves,
    pub actual: Curves,
    pub events: Vec<Event>,
    pub tiers: Vec<ForecastTier>,
    pub catalog: Vec<CatalogEntry>,
    pub contracts: Vec<Contract>,
    pub security: SecuritySpec,
}

impl Scenario {
    pub fn ticks(&self) -> u32 {
        1440 / self.round.tick_minutes
    }

    pub fn dt_h(&self) -> f64 {
        f64::from(self.round.tick_minutes) / 60.0
    }

    pub fn catalog_entry(&self, id: &str) -> Option<&CatalogEntry> {
        self.catalog.iter().find(|c| c.id == id)
    }

    pub fn upgrade(&self, id: &str) -> Option<&SecurityUpgrade> {
        match self.catalog_entry(id).map(|c| &c.kind) {
            Some(CatalogKind::Security(s)) => Some(s),
            _ => None,
        }
    }

    pub fn security_catalog(&self) -> impl Iterator<Item = (&str, &SecurityUpgrade)> {
        self.catalog.iter().filter_map(|c| match &c.kind {
            CatalogKind::Security(s) => Some((c.id.as_str(), s)),
            _ => None,
        })
    }

    pub fn tier(&self, n: u32) -> Option<&ForecastTier> {
        self.tiers.iter().find(|t| t.tier == n)
    }

    pub fn has_attacks(&self) -> bool {
        self.events
            .iter()
            .any(|e| matches!(e.kind, EventKind::Attack { .. }))
    }
}

fn default_tiers() -> Vec<ForecastTier> {
    [(0, 0, 0.0), (1, 4, 10.0), (2, 12, 25.0), (3, 24, 50.0)]
        .into_iter()
        .map(|(tier, lookahead_ticks, cost)| ForecastTier {
            tier,
            lookahead_ticks,
            cost,
        })
        .collect()
}

// ---------------------------------------------------------------- loading

/// A parsed but not yet loaded scenario: knows which curve files it needs.
#[derive(Debug, Clone)]
pub struct ScenarioFile {
    name: String,
    raw: RawScenario,
}

impl ScenarioFile {
    /// Parse scenario TOML. `name` is used in error messages (usually the path).
    pub fn parse(name: &str, toml_text: &str) -> Result<Self, LoadError> {
        let raw: RawScenario = toml::from_str(toml_text).map_err(|e| {
            LoadError::new(
                name,
                "",
                e.message().to_string() + &span_hint(toml_text, e.span()),
            )
        })?;
        Ok(Self {
            name: name.to_string(),
            raw,
        })
    }

    /// The curve paths, as written in the scenario, keyed by scenario key.
    pub fn curve_paths(&self) -> Vec<(String, String)> {
        let f = &self.raw.forecast;
        let a = &self.raw.actual;
        vec![
            ("forecast.demand".into(), f.demand.clone()),
            ("forecast.pv".into(), f.pv.clone()),
            ("forecast.wind".into(), f.wind.clone()),
            ("actual.demand".into(), a.demand.clone()),
            ("actual.pv".into(), a.pv.clone()),
            ("actual.wind".into(), a.wind.clone()),
        ]
    }

    /// Validate and build the scenario. `curves` maps each curve path (as written
    /// in the scenario) to the CSV contents.
    pub fn load(self, curves: &BTreeMap<String, String>) -> Result<Scenario, LoadError> {
        let name = self.name.as_str();
        let raw = self.raw;
        let err = |key: &str, msg: String| LoadError::new(name, key, msg);
        let r = &raw.round;

        if r.tick_minutes == 0 || 1440 % r.tick_minutes != 0 {
            return Err(err(
                "round.tick_minutes",
                format!("{} does not divide 1440", r.tick_minutes),
            ));
        }
        if !(0.0..=1.0).contains(&r.target_efficiency) {
            return Err(err(
                "round.target_efficiency",
                format!("{} is outside 0 to 1", r.target_efficiency),
            ));
        }
        if !(0.0..=1.0).contains(&r.outage_tolerance) {
            return Err(err(
                "round.outage_tolerance",
                format!("{} is outside 0 to 1", r.outage_tolerance),
            ));
        }
        if r.budget < 0.0 || r.co2_cap_kg < 0.0 {
            return Err(err("round", "budget and co2_cap_kg must be >= 0".into()));
        }
        if r.stars == 0 {
            return Err(err("round.stars", "must be at least 1".into()));
        }
        let b = &raw.boost;
        if b.kw < 0.0 || b.co2_per_kwh < 0.0 || b.credit_kwh < 0.0 {
            return Err(err(
                "boost",
                "kw, co2_per_kwh and credit_kwh must be >= 0".into(),
            ));
        }
        let ticks = (1440 / r.tick_minutes) as usize;

        let curve = |key: &str,
                     path: &str,
                     header: &str,
                     unit_range: bool|
         -> Result<Vec<f64>, LoadError> {
            let text = curves.get(path).ok_or_else(|| {
                LoadError::new(
                    path,
                    "",
                    format!("missing curve file (named by {name} {key})"),
                )
            })?;
            parse_curve(path, text, header, ticks, unit_range)
        };
        let forecast = Curves {
            demand: curve("forecast.demand", &raw.forecast.demand, "kw", false)?,
            pv: curve("forecast.pv", &raw.forecast.pv, "pu", true)?,
            wind: curve("forecast.wind", &raw.forecast.wind, "pu", true)?,
        };
        let actual = Curves {
            demand: curve("actual.demand", &raw.actual.demand, "kw", false)?,
            pv: curve("actual.pv", &raw.actual.pv, "pu", true)?,
            wind: curve("actual.wind", &raw.actual.wind, "pu", true)?,
        };

        // catalog
        let mut ids = BTreeSet::new();
        let mut catalog = Vec::new();
        for (i, c) in raw.catalog.iter().enumerate() {
            let key = format!("catalog[{i}]");
            if !ids.insert(c.id.clone()) {
                return Err(err(
                    &format!("{key}.id"),
                    format!("duplicate catalog id '{}'", c.id),
                ));
            }
            catalog.push(catalog_entry(name, &key, c)?);
        }

        // events
        let mut events = Vec::new();
        for (i, e) in raw.actual.events.iter().enumerate() {
            let key = format!("actual.events[{i}]");
            if e.ticks == 0 {
                return Err(err(&format!("{key}.ticks"), "must be at least 1".into()));
            }
            if (e.at + e.ticks) as usize > ticks {
                return Err(err(
                    &key,
                    format!(
                        "at + ticks = {} runs past the end of the day ({ticks} ticks)",
                        e.at + e.ticks
                    ),
                ));
            }
            let kind = match e.kind.as_str() {
                "outage" => {
                    let ac = e.asset_catalog.clone().ok_or_else(|| {
                        err(
                            &format!("{key}.asset_catalog"),
                            "an outage needs asset_catalog".into(),
                        )
                    })?;
                    if !ids.contains(&ac) {
                        return Err(err(
                            &format!("{key}.asset_catalog"),
                            format!("unknown catalog id '{ac}'"),
                        ));
                    }
                    EventKind::Outage { asset_catalog: ac }
                }
                "attack" => {
                    let vs = e.vector.as_deref().ok_or_else(|| {
                        err(&format!("{key}.vector"), "an attack needs a vector".into())
                    })?;
                    let vector = Vector::from_str(vs).map_err(|_| {
                        err(&format!("{key}.vector"), format!("unknown attack vector '{vs}' (credential_theft, rogue_client, broker_outage)"))
                    })?;
                    if vector == Vector::CredentialTheft {
                        match &e.target_catalog {
                            None => {
                                return Err(err(
                                    &format!("{key}.target_catalog"),
                                    "credential_theft needs target_catalog".into(),
                                ))
                            }
                            Some(tc) if !ids.contains(tc) => {
                                return Err(err(
                                    &format!("{key}.target_catalog"),
                                    format!("unknown catalog id '{tc}'"),
                                ))
                            }
                            _ => {}
                        }
                    }
                    EventKind::Attack {
                        vector,
                        target_catalog: e.target_catalog.clone(),
                    }
                }
                other => {
                    return Err(err(
                        &format!("{key}.kind"),
                        format!("unknown event kind '{other}' (outage, attack)"),
                    ))
                }
            };
            events.push(Event {
                at: e.at,
                ticks: e.ticks,
                kind,
            });
        }

        // tiers
        let tiers = if raw.forecast_tiers.is_empty() {
            default_tiers()
        } else {
            raw.forecast_tiers.clone()
        };
        let mut seen = BTreeSet::new();
        for (i, t) in tiers.iter().enumerate() {
            if !seen.insert(t.tier) {
                return Err(err(
                    &format!("forecast_tiers[{i}].tier"),
                    format!("duplicate tier {}", t.tier),
                ));
            }
        }

        // contracts
        for (i, c) in raw.contracts.iter().enumerate() {
            let key = format!("contracts[{i}]");
            if c.product != "h2" {
                return Err(err(
                    &format!("{key}.product"),
                    format!("unknown product '{}' (h2)", c.product),
                ));
            }
            if c.by_tick as usize >= ticks {
                return Err(err(
                    &format!("{key}.by_tick"),
                    format!("{} is outside the day (0..{ticks})", c.by_tick),
                ));
            }
            if c.kg <= 0.0 {
                return Err(err(&format!("{key}.kg"), "must be > 0".into()));
            }
        }

        Ok(Scenario {
            round: raw.round,
            boost: raw.boost,
            forecast,
            actual,
            events,
            tiers,
            catalog,
            contracts: raw.contracts,
            security: raw.security.unwrap_or_default(),
        })
    }
}

fn span_hint(text: &str, span: Option<std::ops::Range<usize>>) -> String {
    match span {
        Some(s) => {
            let line = text[..s.start.min(text.len())].matches('\n').count() + 1;
            format!(" (line {line})")
        }
        None => String::new(),
    }
}

fn catalog_entry(file: &str, key: &str, c: &RawCatalog) -> Result<CatalogEntry, LoadError> {
    let need = |v: Option<f64>, field: &str| -> Result<f64, LoadError> {
        let x = v.ok_or_else(|| {
            LoadError::new(
                file,
                format!("{key}.{field}"),
                format!("required for kind '{}'", c.kind),
            )
        })?;
        if x < 0.0 || !x.is_finite() {
            return Err(LoadError::new(
                file,
                format!("{key}.{field}"),
                format!("{x} must be a finite number >= 0"),
            ));
        }
        Ok(x)
    };
    let eta = |v: Option<f64>, field: &str| -> Result<f64, LoadError> {
        let x = need(v, field)?;
        if x <= 0.0 || x > 1.0 {
            return Err(LoadError::new(
                file,
                format!("{key}.{field}"),
                format!("{x} is outside (0, 1]"),
            ));
        }
        Ok(x)
    };
    let kind = match c.kind.as_str() {
        "source" => {
            let curve = match c.curve.as_deref() {
                Some("pv") => CurveId::Pv,
                Some("wind") => CurveId::Wind,
                Some(o) => {
                    return Err(LoadError::new(
                        file,
                        format!("{key}.curve"),
                        format!("unknown curve '{o}' (pv, wind)"),
                    ))
                }
                None => {
                    return Err(LoadError::new(
                        file,
                        format!("{key}.curve"),
                        "required for kind 'source'",
                    ))
                }
            };
            CatalogKind::Source {
                curve,
                cost_per_kw: need(c.cost_per_kw, "cost_per_kw")?,
                max_capacity_kw: need(c.max_capacity_kw, "max_capacity_kw")?,
            }
        }
        "storage" => {
            let soc_init = need(c.soc_init, "soc_init")?;
            if soc_init > 1.0 {
                return Err(LoadError::new(
                    file,
                    format!("{key}.soc_init"),
                    "must be within 0 to 1",
                ));
            }
            CatalogKind::Storage {
                eta_in: eta(c.eta_in, "eta_in")?,
                eta_out: eta(c.eta_out, "eta_out")?,
                soc_init,
                cost_per_kwh: need(c.cost_per_kwh, "cost_per_kwh")?,
                cost_per_kw: need(c.cost_per_kw, "cost_per_kw")?,
            }
        }
        "converter" => match (c.input.as_deref(), c.output.as_deref()) {
            (Some("electricity"), Some("h2")) => CatalogKind::Electrolyser {
                eta: eta(c.eta, "eta")?,
                kwh_per_kg: need(c.kwh_per_kg, "kwh_per_kg")?,
                cost_per_kw: need(c.cost_per_kw, "cost_per_kw")?,
                tank_cost_per_kg: need(c.tank_cost_per_kg, "tank_cost_per_kg")?,
            },
            (Some("h2"), Some("electricity")) => CatalogKind::FuelCell {
                eta: eta(c.eta, "eta")?,
                kwh_per_kg: need(c.kwh_per_kg, "kwh_per_kg")?,
                cost_per_kw: need(c.cost_per_kw, "cost_per_kw")?,
            },
            _ => return Err(LoadError::new(
                file,
                format!("{key}.input"),
                "a converter is electricity -> h2 (electrolyser) or h2 -> electricity (fuel cell)",
            )),
        },
        "interconnector" => CatalogKind::Interconnector {
            eta: eta(c.eta, "eta")?,
            co2_per_kwh_import: need(c.co2_per_kwh_import, "co2_per_kwh_import")?,
            export_credit_ratio: need(c.export_credit_ratio, "export_credit_ratio")?,
            cost_per_kw: need(c.cost_per_kw, "cost_per_kw")?,
            max_kw: c.max_kw.unwrap_or(f64::INFINITY),
            credits_init: c.credits_init.unwrap_or(0.0),
        },
        "security" => {
            let mut defeats = Vec::new();
            for (j, d) in c.defeats.clone().unwrap_or_default().iter().enumerate() {
                let v = Vector::from_str(d).map_err(|_| {
                    LoadError::new(
                        file,
                        format!("{key}.defeats[{j}]"),
                        format!("unknown attack vector '{d}'"),
                    )
                })?;
                defeats.push(v);
            }
            CatalogKind::Security(SecurityUpgrade {
                cost: need(c.cost, "cost")?,
                setup_ticks: c.setup_ticks.unwrap_or(0),
                defeats,
                failover_ticks: c.failover_ticks,
                warn_ticks: c.warn_ticks,
            })
        }
        other => {
            return Err(LoadError::new(
                file,
                format!("{key}.kind"),
                format!(
                    "unknown kind '{other}' (source, storage, converter, interconnector, security)"
                ),
            ))
        }
    };
    Ok(CatalogEntry {
        id: c.id.clone(),
        kind,
    })
}

/// Parse a one-column curve CSV: a header row, then exactly `ticks` rows.
pub fn parse_curve(
    path: &str,
    text: &str,
    header: &str,
    ticks: usize,
    unit_range: bool,
) -> Result<Vec<f64>, LoadError> {
    let mut lines = text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty());
    match lines.next() {
        Some((_, h)) if h.trim() == header => {}
        Some((_, h)) => {
            return Err(LoadError::new(
                path,
                "row 1",
                format!("header is '{}', expected '{header}'", h.trim()),
            ))
        }
        None => return Err(LoadError::new(path, "", "empty curve file")),
    }
    let mut out = Vec::with_capacity(ticks);
    for (i, line) in lines {
        let row = i + 1;
        let v: f64 = line.trim().parse().map_err(|_| {
            LoadError::new(
                path,
                format!("row {row}"),
                format!("'{}' is not a number", line.trim()),
            )
        })?;
        if !v.is_finite() || v < 0.0 || (unit_range && v > 1.0) {
            let range = if unit_range { "0 to 1" } else { ">= 0" };
            return Err(LoadError::new(
                path,
                format!("row {row}"),
                format!("value {v} is outside {range}"),
            ));
        }
        out.push(v);
    }
    if out.len() != ticks {
        return Err(LoadError::new(
            path,
            "",
            format!("{} data rows, expected {ticks} (one per tick)", out.len()),
        ));
    }
    Ok(out)
}
