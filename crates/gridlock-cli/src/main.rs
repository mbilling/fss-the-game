use gridlock_bus::{MemoryBus, Session};
use gridlock_cli::*;
use gridlock_core::engine::RunOutput;
use gridlock_core::plan::InteractionLog;
use gridlock_core::{clock, Action, Engine, Phase};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

const USAGE: &str = "\
gridlock: a Power-to-X grid puzzle

usage:
  gridlock validate <scenario.toml> [--plan <plan.toml>]
  gridlock run      <scenario.toml> --plan <plan.toml> [--interactions <log.toml>]
                    [--bus memory|mqtt] [--tick-ms 0] [--json <out.json>]
  gridlock play     <scenario.toml> --plan <plan.toml> [--bus memory|mqtt] [--tick-ms <ms>]
  gridlock debrief  <run.json>

exit codes: 0 pass, 1 fail, 2 invalid scenario or plan, 3 bus error";

#[derive(Default)]
struct Args {
    pos: Vec<String>,
    plan: Option<PathBuf>,
    interactions: Option<PathBuf>,
    bus: Option<String>,
    tick_ms: Option<u64>,
    json: Option<PathBuf>,
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = raw.iter();
    while let Some(x) = it.next() {
        let mut val = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match x.as_str() {
            "--plan" => a.plan = Some(val("--plan")?.into()),
            "--interactions" => a.interactions = Some(val("--interactions")?.into()),
            "--bus" => a.bus = Some(val("--bus")?),
            "--tick-ms" => {
                a.tick_ms = Some(
                    val("--tick-ms")?
                        .parse()
                        .map_err(|_| "--tick-ms needs a number".to_string())?,
                )
            }
            "--json" => a.json = Some(val("--json")?.into()),
            "-h" | "--help" => return Err(String::new()),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s => a.pos.push(s.to_string()),
        }
    }
    Ok(a)
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let code = match raw.split_first() {
        Some((cmd, rest)) => match parse_args(rest) {
            Ok(args) => dispatch(cmd, &args),
            Err(e) => {
                if !e.is_empty() {
                    eprintln!("error: {e}\n");
                }
                eprintln!("{USAGE}");
                EXIT_INVALID
            }
        },
        None => {
            eprintln!("{USAGE}");
            EXIT_INVALID
        }
    };
    std::process::exit(code);
}

fn dispatch(cmd: &str, a: &Args) -> i32 {
    match cmd {
        "validate" => validate(a),
        "run" => run_cmd(a),
        "play" => play(a),
        "debrief" => debrief(a),
        "help" | "-h" | "--help" => {
            println!("{USAGE}");
            0
        }
        other => {
            eprintln!("error: unknown command '{other}'\n\n{USAGE}");
            EXIT_INVALID
        }
    }
}

fn invalid(e: impl std::fmt::Display) -> i32 {
    eprintln!("error: {e}");
    EXIT_INVALID
}

fn check_bus(a: &Args) -> Result<(), i32> {
    match a.bus.as_deref() {
        None | Some("memory") => Ok(()),
        Some("mqtt") => {
            eprintln!("bus error: the mqtt bus is implemented in M1; use --bus memory");
            Err(EXIT_BUS)
        }
        Some(o) => {
            eprintln!("error: unknown bus '{o}' (memory, mqtt)");
            Err(EXIT_INVALID)
        }
    }
}

fn scenario_arg(a: &Args) -> Result<&Path, i32> {
    a.pos
        .first()
        .map(Path::new)
        .ok_or_else(|| invalid("missing <scenario.toml>"))
}

fn validate(a: &Args) -> i32 {
    let Ok(path) = scenario_arg(a) else {
        return EXIT_INVALID;
    };
    let sc = match load_scenario(path) {
        Ok(s) => s,
        Err(e) => return invalid(e),
    };
    println!(
        "ok: {} ({}): {} ticks of {} min, budget {}, target eff {:.0}%, CO2 cap {} kg, {} catalog entries, {} contract(s)",
        sc.round.name,
        sc.round.id,
        sc.ticks(),
        sc.round.tick_minutes,
        sc.round.budget,
        sc.round.target_efficiency * 100.0,
        sc.round.co2_cap_kg,
        sc.catalog.len(),
        sc.contracts.len()
    );
    if let Some(pp) = &a.plan {
        let vp = match load_plan(pp).and_then(|p| p.validate(&sc, &pp.display().to_string())) {
            Ok(v) => v,
            Err(e) => return invalid(e),
        };
        print!("{}", plan_summary(&sc, &vp));
    }
    0
}

fn run_cmd(a: &Args) -> i32 {
    if let Err(c) = check_bus(a) {
        return c;
    }
    let Ok(path) = scenario_arg(a) else {
        return EXIT_INVALID;
    };
    let Some(pp) = &a.plan else {
        return invalid("run needs --plan <plan.toml>");
    };
    let loaded = load_scenario(path).and_then(|sc| {
        let plan = load_plan(pp)?;
        let log = match &a.interactions {
            Some(l) => load_interactions(l)?,
            None => InteractionLog::default(),
        };
        Ok((sc, plan, log))
    });
    let (sc, plan, log) = match loaded {
        Ok(x) => x,
        Err(e) => return invalid(e),
    };
    let tm = sc.round.tick_minutes;
    let session = match run(sc, &plan, &pp.display().to_string(), &log) {
        Ok(s) => s,
        Err(e) => return invalid(e),
    };
    let e = session.engine();
    let vp = e.plan().expect("plan accepted");
    for w in &vp.warnings {
        eprintln!("warning: {w}");
    }
    let out = e.run_output().expect("run finished");
    print!("{}", header(e.scenario(), vp));
    if a.tick_ms.unwrap_or(0) > 0 {
        print!("{}", table_head());
        for r in &out.ticks {
            print!("{}", table_row(r, tm, &marks(r)));
            let _ = std::io::stdout().flush();
            std::thread::sleep(Duration::from_millis(a.tick_ms.unwrap_or(0)));
        }
        print!("{}", debrief_text(&out));
    } else {
        print!("{}", report(&out, tm));
    }
    if let Some(j) = &a.json {
        if let Err(err) = std::fs::write(j, to_json(&out)) {
            eprintln!("error: writing {}: {err}", j.display());
        }
    }
    if out.score.pass {
        EXIT_PASS
    } else {
        EXIT_FAIL
    }
}

fn debrief(a: &Args) -> i32 {
    let Some(p) = a.pos.first() else {
        return invalid("missing <run.json>");
    };
    let text = match std::fs::read_to_string(p) {
        Ok(t) => t,
        Err(e) => return invalid(format!("{p}: {e}")),
    };
    let out: RunOutput = match serde_json::from_str(&text) {
        Ok(o) => o,
        Err(e) => return invalid(format!("{p}: {e}")),
    };
    println!(
        "Debrief for {} ({} ticks played)",
        out.scenario, out.score.ticks_played
    );
    print!("{}", debrief_text(&out));
    if out.score.pass {
        EXIT_PASS
    } else {
        EXIT_FAIL
    }
}

// ---------------------------------------------------------------- play

enum Input {
    Line(String),
    Eof,
}

fn play(a: &Args) -> i32 {
    if let Err(c) = check_bus(a) {
        return c;
    }
    let Ok(path) = scenario_arg(a) else {
        return EXIT_INVALID;
    };
    let Some(pp) = &a.plan else {
        return invalid("play needs --plan <plan.toml>");
    };
    let (sc, plan) = match load_scenario(path).and_then(|sc| Ok((sc, load_plan(pp)?))) {
        Ok(x) => x,
        Err(e) => return invalid(e),
    };
    let tm = sc.round.tick_minutes;
    let mut s = Session::new(Engine::new(sc), MemoryBus::new(), "");
    if let Err(e) = s.submit_plan(&plan, &pp.display().to_string()) {
        return invalid(e);
    }
    {
        let e = s.engine();
        let vp = e.plan().expect("plan accepted");
        print!("{}", plan_summary(e.scenario(), vp));
        print!("{}", header(e.scenario(), vp));
    }
    println!("Enter = next tick, b = boost, u <id> = start upgrade, q = quit");
    if let Some(ms) = a.tick_ms {
        println!("auto-advancing every {ms} ms; type commands and press Enter");
    }

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    if tx.send(Input::Line(l)).is_err() {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(Input::Eof);
    });

    print!("{}", table_head());
    let mut eof = false;
    while s.engine().phase() == Phase::Play {
        // Wait for a command or the clock.
        let input = if eof {
            None
        } else {
            match a.tick_ms {
                Some(ms) => rx.recv_timeout(Duration::from_millis(ms)).ok(),
                None => rx.recv().ok(),
            }
        };
        let cmd = match input {
            Some(Input::Line(l)) => l.trim().to_string(),
            Some(Input::Eof) | None if a.tick_ms.is_none() || eof => {
                eof = true;
                String::new()
            }
            Some(Input::Eof) => {
                eof = true;
                String::new()
            }
            None => String::new(),
        };
        let action = match cmd.as_str() {
            "" => None,
            "q" | "quit" => break,
            "b" | "boost" => Some(Action::Boost),
            c if c.starts_with("u ") => Some(Action::Upgrade(c[2..].trim().to_string())),
            c if c.starts_with("upgrade ") => Some(Action::Upgrade(c[8..].trim().to_string())),
            other => {
                println!("  ? unknown command '{other}'");
                continue;
            }
        };
        if let Some(act) = action {
            if s.engine().t() == 0 {
                println!("  play the first tick before acting");
                continue;
            }
            s.send(&act);
            for (act, res) in s.pump() {
                match res {
                    Ok(()) => println!("  {} {} accepted", act.name(), act.arg().unwrap_or("")),
                    Err(r) => println!("  {} {} refused: {r}", act.name(), act.arg().unwrap_or("")),
                }
            }
            if a.tick_ms.is_none() {
                continue;
            }
        }
        let t = s.step().expect("in Play");
        let e = s.engine();
        let r = &e.records()[t as usize];
        print!("{}", table_row(r, tm, &marks(r)));
        let mut status = format!(
            "      boost charges {}{}",
            e.boost_charges(),
            if e.boost_armed() { " (armed)" } else { "" }
        );
        if e.busy_until() > t {
            status += &format!(" | busy until {}", clock(e.busy_until(), tm));
        }
        println!("{status}");
        if let Some(m) = s.bus().retained().get("forecast/actual_window") {
            let w: serde_json::Value = serde_json::from_slice(&m.payload).unwrap_or_default();
            if let Some(d) = w
                .get("demand")
                .and_then(|d| d.as_array())
                .filter(|d| !d.is_empty())
            {
                let fmt = |k: &str| -> String {
                    w.get(k)
                        .and_then(|x| x.as_array())
                        .map(|xs| {
                            xs.iter()
                                .map(|x| format!("{:.0}", x.as_f64().unwrap_or(0.0)))
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default()
                };
                let _ = d;
                println!(
                    "      ahead demand [{}] pv [{}] wind [{}]",
                    fmt("demand"),
                    fmt("pv"),
                    fmt("wind")
                );
            }
        }
        let _ = std::io::stdout().flush();
    }
    let e = s.engine();
    match e.run_output() {
        Some(out) => {
            print!("{}", debrief_text(&out));
            if out.score.pass {
                EXIT_PASS
            } else {
                EXIT_FAIL
            }
        }
        None => {
            println!("quit at tick {}", e.t());
            EXIT_FAIL
        }
    }
}
