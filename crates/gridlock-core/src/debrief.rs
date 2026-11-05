//! Rule-based debrief findings, narrated by View.

use crate::clock;
use crate::engine::{Engine, TickRecord};
use crate::plan::{AssetSpec, ValidPlan};
use crate::scenario::Vector;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub kind: String,
    pub t_from: u32,
    pub t_to: u32,
    pub kwh: f64,
    pub asset: Option<String>,
    pub text: String,
}

pub fn findings(e: &Engine) -> Vec<Finding> {
    let Some(plan) = e.plan() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    curtailment(e, plan, &mut out);
    outages(e, plan, &mut out);
    co2(e, &mut out);
    security(e, &mut out);
    losses(e, &mut out);
    out
}

fn kwh(kw: f64, e: &Engine) -> f64 {
    kw * e.scenario().dt_h()
}

/// The contiguous run of ticks with the most curtailed kWh.
fn curtailment(e: &Engine, plan: &ValidPlan, out: &mut Vec<Finding>) {
    let r = e.records();
    let mut best: Option<(usize, usize, f64)> = None;
    let mut i = 0;
    while i < r.len() {
        if r[i].curtailed_kw <= 1e-9 {
            i += 1;
            continue;
        }
        let start = i;
        let mut sum = 0.0;
        while i < r.len() && r[i].curtailed_kw > 1e-9 {
            sum += kwh(r[i].curtailed_kw, e);
            i += 1;
        }
        if best.is_none_or(|(_, _, b)| sum > b) {
            best = Some((start, i - 1, sum));
        }
    }
    let Some((a, b, sum)) = best else { return };
    let full = plan.assets.iter().enumerate().find(|(k, s)| {
        matches!(s.spec, AssetSpec::Storage { .. })
            && r[a..=b]
                .iter()
                .any(|rec| rec.assets[*k].soc.is_some_and(|x| x > 0.999))
    });
    let tm = e.scenario().round.tick_minutes;
    let why = match full {
        Some((_, s)) => format!(
            " {} was full; more storage or a bigger electrolyser would have soaked it up.",
            s.id
        ),
        None if plan
            .assets
            .iter()
            .any(|s| matches!(s.spec, AssetSpec::Storage { .. })) =>
        {
            " Storage was charging at its max_kw limit or held back by a rogue command.".to_string()
        }
        None => " There was no storage to put it in.".to_string(),
    };
    out.push(Finding {
        kind: "curtailment".into(),
        t_from: r[a].t,
        t_to: r[b].t,
        kwh: sum,
        asset: full.map(|(_, s)| s.id.clone()),
        text: format!(
            "Between {} and {} you threw away {:.0} kWh.{}",
            clock(r[a].t, tm),
            clock(r[b].t + 1, tm),
            sum,
            why
        ),
    });
}

/// One finding per outage tick, with the smallest single change that covers it.
fn outages(e: &Engine, plan: &ValidPlan, out: &mut Vec<Finding>) {
    let sc = e.scenario();
    let tm = sc.round.tick_minutes;
    let dt = sc.dt_h();
    for rec in e.records().iter().filter(|r| r.outage) {
        let u = rec.unmet_kw;
        let (asset, fix) = smallest_fix(rec, plan, sc.boost.kw, dt, u);
        out.push(Finding {
            kind: "outage".into(),
            t_from: rec.t,
            t_to: rec.t,
            kwh: u * dt,
            asset,
            text: format!(
                "At {} you were {:.0} kW short. {}",
                clock(rec.t, tm),
                u,
                fix
            ),
        });
    }
}

fn smallest_fix(
    rec: &TickRecord,
    plan: &ValidPlan,
    boost_kw: f64,
    dt: f64,
    u: f64,
) -> (Option<String>, String) {
    for (k, a) in plan.assets.iter().enumerate() {
        if let AssetSpec::Storage {
            capacity_kwh,
            max_kw,
            reserve,
            eta_out,
            ..
        } = a.spec
        {
            let st = &rec.assets[k];
            let soc_kwh = st.soc.unwrap_or(0.0) * capacity_kwh;
            if st.status != crate::engine::Status::Ok {
                continue;
            }
            if (st.p_kw - max_kw).abs() < 1e-6
                && soc_kwh - reserve * capacity_kwh > u * dt / eta_out
            {
                return (
                    Some(a.id.clone()),
                    format!(
                        "Raising {} max_kw from {:.0} to {:.0} would have covered it.",
                        a.id,
                        max_kw,
                        max_kw + u
                    ),
                );
            }
            if reserve > 0.0
                && soc_kwh * eta_out >= u * dt
                && soc_kwh <= reserve * capacity_kwh + 1e-6
            {
                let need = (u * dt / eta_out) / capacity_kwh;
                return (
                    Some(a.id.clone()),
                    format!("{} was holding its {:.0}% reserve; a reserve {:.0} points lower would have covered this tick.", a.id, reserve * 100.0, (need * 100.0).ceil()),
                );
            }
        }
    }
    if rec.boost_kw == 0.0 && u <= boost_kw {
        return (
            None,
            "One boost armed the tick before would have covered it.".into(),
        );
    }
    let kwh = u * dt;
    match plan
        .assets
        .iter()
        .find(|a| matches!(a.spec, AssetSpec::Storage { .. }))
    {
        Some(a) => (
            Some(a.id.clone()),
            format!(
                "{} ran dry; about {:.0} kWh more capacity charged earlier would have covered it.",
                a.id,
                kwh / 0.9
            ),
        ),
        None => (
            None,
            format!(
                "A battery of at least {:.0} kW and {:.0} kWh would have covered it.",
                u,
                kwh / 0.9
            ),
        ),
    }
}

fn co2(e: &Engine, out: &mut Vec<Finding>) {
    let sc = e.scenario();
    if e.co2_kg() <= 0.0 {
        return;
    }
    let cap = sc.round.co2_cap_kg.max(1e-9);
    let imp_co2: f64 = e.co2_kg() - e.boost_kwh() * sc.boost.co2_per_kwh;
    let boost_co2 = e.boost_kwh() * sc.boost.co2_per_kwh;
    out.push(Finding {
        kind: "co2".into(),
        t_from: 0,
        t_to: e.records().last().map_or(0, |r| r.t),
        kwh: e.import_kwh() + e.boost_kwh(),
        asset: None,
        text: format!(
            "CO2 came to {:.0} of {:.0} kg: imports {:.0} kWh ({:.0} kg, {:.0}% of the cap), boost {:.0} kWh ({:.0} kg, {:.0}% of the cap).",
            e.co2_kg(),
            sc.round.co2_cap_kg,
            e.import_kwh(),
            imp_co2,
            imp_co2 / cap * 100.0,
            e.boost_kwh(),
            boost_co2,
            boost_co2 / cap * 100.0
        ),
    });
}

fn security(e: &Engine, out: &mut Vec<Finding>) {
    let tm = e.scenario().round.tick_minutes;
    let dt = e.scenario().dt_h();
    for a in e.attacks() {
        let to = a.at + a.ticks.max(1) - 1;
        let window: Vec<&TickRecord> = e
            .records()
            .iter()
            .filter(|r| r.t >= a.at && r.t <= to)
            .collect();
        let outage_ticks = window.iter().filter(|r| r.outage).count();
        let direct =
            usize::from(a.landed && a.mitigated_by.is_none() && a.vector.costs_star_directly());
        let lost_kwh: f64 = window
            .iter()
            .map(|r| {
                (r.unmet_kw
                    + if a.landed && a.vector == Vector::CredentialTheft {
                        r.curtailed_kw
                    } else {
                        0.0
                    })
                    * dt
            })
            .sum();
        let target = a
            .target
            .as_deref()
            .map(|t| format!(" on {t}"))
            .unwrap_or_default();
        let acted = a.alert_t.map(|al| {
            e.interactions()
                .iter()
                .any(|i| i.accepted && i.t >= al && i.t < a.at && i.action == "upgrade")
        });
        let alert = match (a.alert_t, acted) {
            (Some(al), Some(true)) => format!(
                " IDS warned at {} and you started an upgrade in time.",
                clock(al, tm)
            ),
            (Some(al), _) => format!(
                " IDS warned at {} but nothing was started before it hit.",
                clock(al, tm)
            ),
            _ => String::new(),
        };
        let text = if let Some(b) = &a.blocked_by {
            format!(
                "{} {} at {} was blocked by {}.{}",
                a.vector,
                target.trim_start(),
                clock(a.at, tm),
                b,
                alert
            )
        } else if !a.landed {
            format!(
                "{} at {} found no target in your plant.{}",
                a.vector,
                clock(a.at, tm),
                alert
            )
        } else {
            let mit = a
                .mitigated_by
                .as_deref()
                .map(|m| format!(" {m} failed over, so it only lasted {} ticks.", a.ticks))
                .unwrap_or_default();
            format!(
                "{}{} at {} landed.{} It cost {} star(s) and {:.0} kWh.{}",
                a.vector,
                target,
                clock(a.at, tm),
                mit,
                outage_ticks + direct,
                lost_kwh,
                alert
            )
        };
        out.push(Finding {
            kind: "attack".into(),
            t_from: a.at,
            t_to: to,
            kwh: lost_kwh,
            asset: a.target.clone(),
            text,
        });
    }
}

fn losses(e: &Engine, out: &mut Vec<Finding>) {
    let last = e.records().last().map_or(0, |r| r.t);
    for (id, l) in e.device_losses() {
        if l > 0.5 {
            out.push(Finding {
                kind: "losses".into(),
                t_from: 0,
                t_to: last,
                kwh: l,
                asset: Some(id.clone()),
                text: format!(
                    "{id} lost {l:.0} kWh to conversion; that counts against efficiency."
                ),
            });
        }
    }
}
