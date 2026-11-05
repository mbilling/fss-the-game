//! Golden runs of the reference plans, plus the CLI's exit-code contract.
//! Regenerate the expected JSON with `UPDATE_GOLDEN=1 cargo test -p gridlock-cli`.

use gridlock_bus::{Bus, MemoryBus, Session};
use gridlock_cli::{load_interactions, load_plan, load_scenario, run, to_json};
use gridlock_core::engine::RunOutput;
use gridlock_core::plan::InteractionLog;
use gridlock_core::scenario::Vector;
use gridlock_core::Engine;
use std::path::PathBuf;
use std::process::Command;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scenario_path() -> PathBuf {
    repo().join("scenarios/cloudy_tuesday/cloudy_tuesday.toml")
}

fn golden(plan: &str, log: Option<&str>, expected: &str) -> RunOutput {
    let sc = load_scenario(&scenario_path()).unwrap();
    let pp = repo().join("tests/golden/plans").join(plan);
    let p = load_plan(&pp).unwrap();
    let log = match log {
        Some(l) => load_interactions(&repo().join("tests/golden/interactions").join(l)).unwrap(),
        None => InteractionLog::default(),
    };
    let s = run(sc, &p, &format!("plans/{plan}"), &log).unwrap();
    let out = s.engine().run_output().unwrap();
    let json = to_json(&out);
    let exp = repo().join("tests/golden/expected").join(expected);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&exp, &json).unwrap();
    } else {
        let want = std::fs::read_to_string(&exp)
            .unwrap_or_else(|_| panic!("missing {}; run with UPDATE_GOLDEN=1", exp.display()));
        assert!(
            want == json,
            "{} differs from the golden file; rerun with UPDATE_GOLDEN=1 if the change is intended",
            expected
        );
    }
    out
}

#[test]
fn balanced_passes_with_three_stars() {
    let out = golden("balanced.toml", None, "balanced.json");
    assert!(out.score.pass);
    assert_eq!(out.score.stars, 3);
    assert!(
        out.score.efficiency >= 0.80,
        "efficiency {}",
        out.score.efficiency
    );
    assert_eq!(out.score.contracts_met, 1);
    assert!(out
        .attacks
        .iter()
        .all(|a| a.blocked_by.is_some() || a.mitigated_by.is_some()));
}

#[test]
fn balanced_replay_with_a_boost() {
    let out = golden(
        "balanced.toml",
        Some("balanced_run1.toml"),
        "balanced_run1.json",
    );
    assert_eq!(out.interactions.len(), 1);
    assert!(out.interactions[0].accepted);
    assert_eq!(
        out.ticks[72].boost_kw, 0.0,
        "boost fired at 72 but had no deficit to cover"
    );
    assert!(out.score.pass);
}

#[test]
fn no_storage_fails_on_the_third_outage_tick() {
    let out = golden("no_storage.toml", None, "no_storage.json");
    let outages: Vec<u32> = out.ticks.iter().filter(|r| r.outage).map(|r| r.t).collect();
    assert_eq!(outages.len(), 3);
    assert_eq!(
        out.ticks.last().unwrap().t,
        outages[2],
        "play stops on the third outage tick"
    );
    assert!(out.score.ended_early && !out.score.pass);
}

#[test]
fn all_import_loses_a_star_to_the_co2_cap() {
    let out = golden("all_import.toml", None, "all_import.json");
    assert!(out
        .score
        .star_losses
        .iter()
        .any(|l| l.cause.contains("CO2")));
    assert!(out.score.co2_kg > out.score.co2_cap_kg);
    assert!(!out.score.pass);
}

#[test]
fn unsecured_fails_to_the_battery_hijack() {
    let out = golden("unsecured.toml", None, "unsecured.json");
    let hijack = out
        .attacks
        .iter()
        .find(|a| a.vector == Vector::CredentialTheft)
        .unwrap();
    assert_eq!(
        (hijack.at, hijack.landed, hijack.target.as_deref()),
        (60, true, Some("bat-1"))
    );
    assert!(!out.score.pass);
    assert!(
        out.score.reasons.contains(&"efficiency".to_string()),
        "the dumped battery sinks efficiency"
    );
}

#[test]
fn bus_carries_the_topic_tree_and_clears_it_between_levels() {
    let sc = load_scenario(&scenario_path()).unwrap();
    let p = load_plan(&repo().join("tests/golden/plans/balanced.toml")).unwrap();
    let mut s = Session::new(Engine::new(sc.clone()), MemoryBus::new(), "ptx/");
    s.submit_plan(&p, "balanced.toml").unwrap();
    for _ in 0..10 {
        s.step().unwrap();
    }
    let b = s.bus();
    for t in [
        "ptx/game/phase",
        "ptx/sim/tick",
        "ptx/forecast/demand",
        "ptx/plant/bat-1/state",
        "ptx/grid/balance",
        "ptx/score/live",
        "ptx/security/status",
    ] {
        assert!(b.retained().contains_key(t), "{t} missing");
    }
    assert_eq!(b.retained_json("ptx/sim/tick").unwrap()["t"], 9);
    assert_eq!(
        b.retained_json("ptx/forecast/actual_window").unwrap()["from"],
        10
    );
    assert_eq!(b.wills().len(), 4, "one will per asset");

    s.new_level(Engine::new(sc));
    let left: Vec<&String> = s.bus().retained().keys().collect();
    assert_eq!(
        left,
        vec!["ptx/game/phase"],
        "only the new level's phase survives"
    );

    // Commands sent as an external client arrive through the bus.
    let mut b = MemoryBus::new();
    b.subscribe("x/#");
    b.publish(
        "x/game/action",
        br#"{"action":"boost"}"#,
        gridlock_bus::Qos::AtLeastOnce,
        false,
    );
    assert_eq!(b.poll().len(), 1);
}

// ---------------------------------------------------------------- binary

fn gridlock(args: &[&str]) -> i32 {
    Command::new(env!("CARGO_BIN_EXE_gridlock"))
        .current_dir(repo())
        .args(args)
        .output()
        .unwrap()
        .status
        .code()
        .unwrap()
}

#[test]
fn run_exit_codes() {
    let sc = "scenarios/cloudy_tuesday/cloudy_tuesday.toml";
    for (plan, code) in [
        ("balanced", 0),
        ("no_storage", 1),
        ("all_import", 1),
        ("unsecured", 1),
    ] {
        assert_eq!(
            gridlock(&[
                "run",
                sc,
                "--plan",
                &format!("tests/golden/plans/{plan}.toml")
            ]),
            code,
            "{plan}"
        );
    }
    assert_eq!(
        gridlock(&[
            "run",
            sc,
            "--plan",
            "tests/golden/plans/balanced.toml",
            "--bus",
            "mqtt"
        ]),
        3
    );
}

#[test]
fn validate_rejects_every_fixture() {
    for f in ["bad_rows", "bad_range", "bad_vector"] {
        assert_eq!(
            gridlock(&["validate", &format!("tests/fixtures/{f}/scenario.toml")]),
            2,
            "{f}"
        );
    }
    let sc = "scenarios/cloudy_tuesday/cloudy_tuesday.toml";
    for p in ["unknown_catalog", "over_budget"] {
        assert_eq!(
            gridlock(&[
                "validate",
                sc,
                "--plan",
                &format!("tests/fixtures/{p}.toml")
            ]),
            2,
            "{p}"
        );
    }
    assert_eq!(
        gridlock(&["validate", sc, "--plan", "tests/golden/plans/balanced.toml"]),
        0
    );
}

#[test]
fn debrief_reads_a_saved_run() {
    let dir = std::env::temp_dir().join(format!("gridlock-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let json = dir.join("run.json");
    let js = json.to_str().unwrap();
    let sc = "scenarios/cloudy_tuesday/cloudy_tuesday.toml";
    assert_eq!(
        gridlock(&[
            "run",
            sc,
            "--plan",
            "tests/golden/plans/balanced.toml",
            "--json",
            js
        ]),
        0
    );
    assert_eq!(gridlock(&["debrief", js]), 0);
    let _ = std::fs::remove_dir_all(dir);
}
