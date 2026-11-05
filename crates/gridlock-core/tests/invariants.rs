//! Conservation and determinism over the real scenario.

mod common;
use common::*;
use gridlock_core::engine::Status;
use gridlock_core::plan::{AssetDecl, Dispatch, PlanHeader, PlanSecurity};
use gridlock_core::{Action, Engine, Phase, Plan};

/// Tiny deterministic generator so the test needs no RNG crate.
struct XorShift(u64);
impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (self.next() % 10_000) as f64 / 10_000.0 * (hi - lo)
    }
    fn chance(&mut self, p: f64) -> bool {
        self.range(0.0, 1.0) < p
    }
}

fn decl(id: &str, catalog: &str) -> AssetDecl {
    AssetDecl {
        id: id.into(),
        catalog: catalog.into(),
        capacity_kw: None,
        capacity_kwh: None,
        max_kw: None,
        reserve: None,
        tank_kg: None,
        export_kw: None,
        import_kw: None,
    }
}

fn random_plan(r: &mut XorShift) -> Plan {
    let mut assets = vec![AssetDecl {
        capacity_kw: Some(r.range(100.0, 2500.0)),
        ..decl("pv-1", "pv")
    }];
    let (mut charge, mut discharge) = (Vec::new(), Vec::new());
    if r.chance(0.7) {
        assets.push(AssetDecl {
            capacity_kw: Some(r.range(0.0, 1500.0)),
            ..decl("wind-1", "wind")
        });
    }
    for (id, cat) in [("bat-1", "battery"), ("ph-1", "pumped_hydro")] {
        if r.chance(0.6) {
            assets.push(AssetDecl {
                capacity_kwh: Some(r.range(0.0, 4000.0)),
                max_kw: Some(r.range(0.0, 800.0)),
                reserve: Some(r.range(0.0, 0.5)),
                ..decl(id, cat)
            });
            charge.push(id.to_string());
            discharge.push(id.to_string());
        }
    }
    if r.chance(0.6) {
        assets.push(AssetDecl {
            max_kw: Some(r.range(0.0, 400.0)),
            tank_kg: Some(r.range(0.0, 80.0)),
            ..decl("ely-1", "electrolyser")
        });
        charge.push("ely-1".into());
        if r.chance(0.5) {
            assets.push(AssetDecl {
                max_kw: Some(r.range(0.0, 300.0)),
                ..decl("fc-1", "fuel_cell")
            });
            discharge.push("fc-1".into());
        }
    }
    if r.chance(0.6) {
        assets.push(AssetDecl {
            export_kw: Some(r.range(0.0, 900.0)),
            import_kw: Some(r.range(0.0, 900.0)),
            ..decl("ic-1", "interconnector")
        });
        charge.push("ic-1".into());
        discharge.push("ic-1".into());
    }
    if r.chance(0.5) {
        charge.reverse();
    }
    if r.chance(0.5) {
        discharge.reverse();
    }
    let upgrades = ["mtls", "acl", "cluster", "ids"]
        .into_iter()
        .filter(|_| r.chance(0.3))
        .map(String::from)
        .collect();
    Plan {
        plan: PlanHeader {
            scenario: "cloudy_tuesday".into(),
            forecast_tier: (r.next() % 4) as u32,
        },
        assets,
        dispatch: Dispatch {
            charge_priority: charge,
            discharge_priority: discharge,
        },
        security: Some(PlanSecurity { upgrades }),
        zones: Vec::new(),
    }
}

#[test]
fn energy_is_conserved_every_tick() {
    let mut sc = cloudy();
    sc.round.budget = 1e9; // explore the space, not the budget
    sc.round.stars = 1000; // play whole days so every event is reached
    let dt = sc.dt_h();
    let mut r = XorShift(0x9e37_79b9_7f4a_7c15);
    let (mut rogue_ticks, mut idle_ticks, mut plans) = (0, 0, 0);
    for _ in 0..300 {
        let p = random_plan(&mut r);
        let mut e = Engine::new(sc.clone());
        e.submit_plan(&p, "random").unwrap();
        plans += 1;
        while e.phase() == Phase::Play {
            e.step().unwrap();
            if r.chance(0.05) {
                let _ = e.act(&Action::Boost);
            }
            if r.chance(0.02) {
                let _ = e.act(&Action::Upgrade(
                    ["mtls", "acl", "cluster", "ids"][(r.next() % 4) as usize].into(),
                ));
            }
        }
        for rec in e.records() {
            let supply =
                rec.harvested_kw + rec.discharge_kw + rec.fc_kw + rec.import_kw + rec.boost_kw;
            let use_ =
                rec.served_kw + rec.charge_kw + rec.ely_kw + rec.export_kw + rec.curtailed_kw;
            assert!(
                ((supply - use_) * dt).abs() < 1e-6,
                "t={} imbalance {} kWh in {:?}",
                rec.t,
                (supply - use_) * dt,
                p
            );
            assert!(
                rec.unmet_kw >= 0.0
                    && rec.curtailed_kw >= 0.0
                    && rec.served_kw <= rec.demand_kw + 1e-9
            );
            for a in &rec.assets {
                assert!(
                    a.soc.is_none_or(|s| (-1e-9..=1.0 + 1e-9).contains(&s)),
                    "soc {:?}",
                    a.soc
                );
                assert!(a.tank_kg.is_none_or(|k| k >= -1e-9));
                rogue_ticks += usize::from(a.status == Status::Rogue);
                idle_ticks += usize::from(a.status == Status::Idle);
            }
            assert!(rec.credits_kwh >= -1e-9);
        }
    }
    assert_eq!(plans, 300);
    assert!(rogue_ticks > 0, "covers ticks with a rogue asset");
    assert!(idle_ticks > 0, "covers ticks under a broker outage");
}

fn run_json(name: &str) -> String {
    let mut e = Engine::new(cloudy());
    e.submit_plan(&plan_file(name), name).unwrap();
    while e.phase() == Phase::Play {
        let t = e.step().unwrap();
        if t == 40 {
            let _ = e.act(&Action::Boost);
        }
    }
    serde_json::to_string(&e.run_output().unwrap()).unwrap()
}

#[test]
fn identical_inputs_give_identical_output() {
    for p in ["balanced.toml", "unsecured.toml", "all_import.toml"] {
        assert_eq!(run_json(p), run_json(p), "{p}");
    }
}
