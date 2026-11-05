#![allow(dead_code)]

use gridlock_core::{Engine, Plan, Scenario, ScenarioFile};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Load a scenario from disk (tests may do IO; the core may not).
pub fn load(path: &str) -> Scenario {
    let p = repo().join(path);
    let file = ScenarioFile::parse(path, &std::fs::read_to_string(&p).unwrap()).unwrap();
    let dir = p.parent().unwrap();
    let curves: BTreeMap<String, String> = file
        .curve_paths()
        .into_iter()
        .map(|(_, rel)| {
            (
                rel.clone(),
                std::fs::read_to_string(dir.join(&rel)).unwrap(),
            )
        })
        .collect();
    file.load(&curves).unwrap()
}

pub fn cloudy() -> Scenario {
    load("scenarios/cloudy_tuesday/cloudy_tuesday.toml")
}

pub fn plan_file(name: &str) -> Plan {
    let p = repo().join("tests/golden/plans").join(name);
    Plan::parse(name, &std::fs::read_to_string(p).unwrap()).unwrap()
}

pub const BASE: &str = r#"
[round]
id = "synth"
name = "Synthetic"
tick_minutes = 60
budget = 1000
target_efficiency = 0.0
co2_cap_kg = 100
stars = 3
outage_tolerance = 0.02

[boost]
kw = 100
charges = 1
co2_per_kwh = 0.5
credit_kwh = 0

[forecast]
demand = "d.csv"
pv = "pv.csv"
wind = "w.csv"

[actual]
demand = "d.csv"
pv = "pv.csv"
wind = "w.csv"

[[catalog]]
id = "pv"
kind = "source"
curve = "pv"
cost_per_kw = 0.0
max_capacity_kw = 10000

[[catalog]]
id = "wind"
kind = "source"
curve = "wind"
cost_per_kw = 0.0
max_capacity_kw = 10000

[[catalog]]
id = "battery"
kind = "storage"
eta_in = 1.0
eta_out = 1.0
soc_init = 0.5
cost_per_kwh = 0.0
cost_per_kw = 0.0

[[catalog]]
id = "electrolyser"
kind = "converter"
input = "electricity"
output = "h2"
eta = 1.0
kwh_per_kg = 10.0
cost_per_kw = 0.0
tank_cost_per_kg = 0.0

[[catalog]]
id = "interconnector"
kind = "interconnector"
eta = 1.0
co2_per_kwh_import = 1.0
export_credit_ratio = 1.0
cost_per_kw = 0.0

[[catalog]]
id = "mtls"
kind = "security"
cost = 10
setup_ticks = 3
defeats = ["credential_theft", "rogue_client"]

[[catalog]]
id = "acl"
kind = "security"
cost = 10
setup_ticks = 2
defeats = ["rogue_client"]

[[catalog]]
id = "cluster"
kind = "security"
cost = 10
setup_ticks = 4
failover_ticks = 1

[[catalog]]
id = "ids"
kind = "security"
cost = 10
setup_ticks = 1
warn_ticks = 2
"#;

pub fn csv(header: &str, v: &[f64]) -> String {
    let mut s = format!("{header}\n");
    for x in v {
        s += &format!("{x}\n");
    }
    s
}

/// A 24-tick (hourly) scenario: BASE plus `extra` TOML, with the given curves.
pub fn synth(
    extra: &str,
    demand: &[f64],
    pv: &[f64],
    wind: &[f64],
) -> Result<Scenario, gridlock_core::LoadError> {
    synth_r(&[], extra, demand, pv, wind)
}

/// Like [`synth`], with string replacements applied to BASE first.
pub fn synth_r(
    reps: &[(&str, &str)],
    extra: &str,
    demand: &[f64],
    pv: &[f64],
    wind: &[f64],
) -> Result<Scenario, gridlock_core::LoadError> {
    let mut base = BASE.to_string();
    for (a, b) in reps {
        assert!(base.contains(a), "replacement target {a:?} not in BASE");
        base = base.replace(a, b);
    }
    let toml = format!("{base}\n{extra}");
    let file = ScenarioFile::parse("synth.toml", &toml)?;
    let mut curves = BTreeMap::new();
    curves.insert("d.csv".to_string(), csv("kw", demand));
    curves.insert("pv.csv".to_string(), csv("pu", pv));
    curves.insert("w.csv".to_string(), csv("pu", wind));
    file.load(&curves)
}

pub fn flat(v: f64) -> Vec<f64> {
    vec![v; 24]
}

pub fn plan(toml: &str) -> Plan {
    let full = format!("[plan]\nscenario = \"synth\"\nforecast_tier = 0\n{toml}");
    Plan::parse("plan.toml", &full).unwrap()
}

pub fn start(sc: Scenario, p: &Plan) -> Engine {
    let mut e = Engine::new(sc);
    e.submit_plan(p, "plan.toml").unwrap();
    e
}

pub fn play_out(e: &mut Engine) {
    while e.phase() == gridlock_core::Phase::Play {
        e.step().unwrap();
    }
}

pub const PV_ONLY: &str = r#"
[[assets]]
id = "pv-1"
catalog = "pv"
capacity_kw = 100

[dispatch]
"#;
