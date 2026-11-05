//! Shared pieces of the `gridlock` CLI: loading files, running a session and
//! printing reports. Kept in a library so golden tests drive exactly what the
//! binary runs.

use gridlock_bus::{MemoryBus, Session};
use gridlock_core::engine::{RoundScore, RunOutput, TickRecord};
use gridlock_core::plan::{exposure, InteractionLog, ValidPlan};
use gridlock_core::{clock, Action, Engine, LoadError, Plan, Scenario, ScenarioFile};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

pub const EXIT_PASS: i32 = 0;
pub const EXIT_FAIL: i32 = 1;
pub const EXIT_INVALID: i32 = 2;
pub const EXIT_BUS: i32 = 3;

fn read(path: &Path) -> Result<String, LoadError> {
    std::fs::read_to_string(path)
        .map_err(|e| LoadError::new(path.display().to_string(), "", e.to_string()))
}

/// Load a scenario TOML and the curve files it names (relative to its directory).
pub fn load_scenario(path: &Path) -> Result<Scenario, LoadError> {
    let name = path.display().to_string();
    let file = ScenarioFile::parse(&name, &read(path)?)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut curves = BTreeMap::new();
    for (key, rel) in file.curve_paths() {
        if curves.contains_key(&rel) {
            continue;
        }
        let full = dir.join(&rel);
        let text = std::fs::read_to_string(&full).map_err(|e| {
            LoadError::new(
                full.display().to_string(),
                "",
                format!("{e} (named by {name} {key})"),
            )
        })?;
        curves.insert(rel, text);
    }
    file.load(&curves)
}

pub fn load_plan(path: &Path) -> Result<Plan, LoadError> {
    Plan::parse(&path.display().to_string(), &read(path)?)
}

pub fn load_interactions(path: &Path) -> Result<InteractionLog, LoadError> {
    InteractionLog::parse(&path.display().to_string(), &read(path)?)
}

/// Play a whole session hands-off on a [`MemoryBus`], replaying the interaction
/// log through the bus at the right ticks.
pub fn run(
    sc: Scenario,
    plan: &Plan,
    plan_name: &str,
    log: &InteractionLog,
) -> Result<Session<MemoryBus>, LoadError> {
    let mut s = Session::new(Engine::new(sc), MemoryBus::new(), "");
    s.submit_plan(plan, plan_name)?;
    let mut by_t: BTreeMap<u32, Vec<Action>> = BTreeMap::new();
    for e in &log.interactions {
        by_t.entry(e.t).or_default().push(Action::from_entry(e));
    }
    while s.engine().phase() == gridlock_core::Phase::Play {
        let t = s.step().expect("engine is in Play");
        if s.engine().phase() != gridlock_core::Phase::Play {
            break;
        }
        if let Some(actions) = by_t.get(&t) {
            for a in actions {
                s.send(a);
            }
        }
        s.pump();
    }
    Ok(s)
}

pub fn to_json(out: &RunOutput) -> String {
    let mut s = serde_json::to_string_pretty(out).expect("run output serialises");
    s.push('\n');
    s
}

// ---------------------------------------------------------------- reports

pub fn plan_summary(sc: &Scenario, vp: &ValidPlan) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "Plan for {} ({})", sc.round.name, sc.round.id);
    for c in &vp.cost_lines {
        let _ = writeln!(s, "  {:<22} {:>7.1}", c.item, c.cost);
    }
    let _ = writeln!(
        s,
        "  {:<22} {:>7.1} of {} (reserve {:.1})",
        "total",
        vp.cost,
        sc.round.budget,
        sc.round.budget - vp.cost
    );
    let _ = writeln!(s, "Exposure:");
    for (v, text) in exposure(sc, &vp.upgrades) {
        let _ = writeln!(s, "  {:<17} {}", v.as_str(), text);
    }
    for w in &vp.warnings {
        let _ = writeln!(s, "warning: {w}");
    }
    s
}

pub fn header(sc: &Scenario, vp: &ValidPlan) -> String {
    format!(
        "{} | target eff {:.0}% | CO2 cap {} kg | budget {:.1}/{} | forecast tier {} ({} ticks ahead)\n",
        sc.round.name,
        sc.round.target_efficiency * 100.0,
        sc.round.co2_cap_kg,
        vp.cost,
        sc.round.budget,
        vp.tier.tier,
        vp.tier.lookahead_ticks
    )
}

pub fn table_head() -> String {
    format!(
        "{:>3} {:>5} {:>7} {:>7} {:>7} {:>7} {:>6} {:>6} {:>6} {:>5} {:>7} {:>6} {:>6} {:>5}\n",
        "t",
        "clock",
        "harv",
        "demand",
        "served",
        "stor",
        "h2kg",
        "exp",
        "imp",
        "boost",
        "curt",
        "unmet",
        "co2",
        "stars"
    )
}

pub fn table_row(r: &TickRecord, tick_minutes: u32, marks: &str) -> String {
    let h2: f64 = r.assets.iter().filter_map(|a| a.tank_kg).sum();
    format!(
        "{:>3} {:>5} {:>7.0} {:>7.0} {:>7.0} {:>7.0} {:>6.1} {:>6.0} {:>6.0} {:>5.0} {:>7.0} {:>6.0} {:>6.0} {:>5}{}\n",
        r.t,
        clock(r.t, tick_minutes),
        r.harvested_kw,
        r.demand_kw,
        r.served_kw,
        r.discharge_kw - r.charge_kw,
        h2,
        r.export_kw,
        r.import_kw,
        r.boost_kw,
        r.curtailed_kw,
        r.unmet_kw,
        r.co2_kg,
        "*".repeat(r.stars as usize),
        marks
    )
}

/// Markers for a tick: outage, offline/rogue assets.
pub fn marks(r: &TickRecord) -> String {
    let mut m = String::new();
    if r.outage {
        m.push_str("  OUTAGE");
    }
    for a in &r.assets {
        match a.status {
            gridlock_core::engine::Status::Ok => {}
            st => {
                let _ = write!(
                    m,
                    "  {}:{}",
                    a.id,
                    serde_json::to_value(st)
                        .ok()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_default()
                );
            }
        }
    }
    m
}

pub fn final_line(s: &RoundScore) -> String {
    let body = format!(
        "{}* eff {:.0}% (target {:.0}%) co2 {:.0}/{:.0} kg contracts {}/{}",
        s.stars,
        s.efficiency * 100.0,
        s.target_efficiency * 100.0,
        s.co2_kg,
        s.co2_cap_kg,
        s.contracts_met,
        s.contracts_total
    );
    if s.pass {
        format!("PASS {body}")
    } else {
        format!("FAIL {body} ({})", s.reasons.join(", "))
    }
}

pub fn report(out: &RunOutput, tick_minutes: u32) -> String {
    let mut s = table_head();
    for r in &out.ticks {
        s += &table_row(r, tick_minutes, &marks(r));
    }
    s += &debrief_text(out);
    s
}

pub fn debrief_text(out: &RunOutput) -> String {
    let mut s = String::new();
    let refused: Vec<_> = out.interactions.iter().filter(|i| !i.accepted).collect();
    if !refused.is_empty() {
        s += "\nRefused interactions:\n";
        for i in refused {
            let reason = i.reason.map(|r| r.to_string()).unwrap_or_default();
            let _ = writeln!(
                s,
                "  t={} {} {} -> {}",
                i.t,
                i.action,
                i.arg.clone().unwrap_or_default(),
                reason
            );
        }
    }
    if !out.score.star_losses.is_empty() {
        s += "\nStars lost:\n";
        for l in &out.score.star_losses {
            let _ = writeln!(s, "  t={} {}", l.t, l.cause);
        }
    }
    s += "\nView's debrief:\n";
    for (n, f) in out.findings.iter().enumerate() {
        let _ = writeln!(s, "  {}. [{}] {}", n + 1, f.kind, f.text);
    }
    let sc = &out.score;
    let _ = writeln!(
        s,
        "\nEnergy: harvested {:.0} kWh, curtailed {:.0}, exported {:.0}, losses {:.0}, leftover credit {:.0}",
        sc.harvested_kwh, sc.curtailed_kwh, sc.export_kwh, sc.losses_kwh, sc.leftover_kwh
    );
    s += &final_line(sc);
    s.push('\n');
    s
}
