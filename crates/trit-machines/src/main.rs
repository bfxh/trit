//! `trit-machines`：三个三值小部件的演示 / 测量入口。

use std::process::ExitCode;

use trit_core::value::Ternary;
use trit_machines::{ca, snn, vm};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("vm") => vm_demo(),
        Some("ca") => ca_demo(&args[1..]),
        Some("snn") => snn_demo(&args[1..]),
        _ => Err(usage()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> String {
    "用法：trit-machines <vm|ca|snn> [--width N] [--steps N] [--size N] [--ticks N] [--seed N] [--kick N] [--drive N] [--threshold N]"
        .to_string()
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1).cloned())
}

fn number(args: &[String], name: &str, default: usize) -> Result<usize, String> {
    match flag(args, name) {
        Some(raw) => raw.parse().map_err(|_| format!("{name} 不是整数: {raw}")),
        None => Ok(default),
    }
}

fn vm_demo() -> Result<(), String> {
    let masking = [
        vm::Op::Push(Ternary::Unknown),
        vm::Op::Push(Ternary::False),
        vm::Op::And,
        vm::Op::Halt,
    ];
    let correlated = [
        vm::Op::Push(Ternary::Unknown),
        vm::Op::Dup,
        vm::Op::Not,
        vm::Op::And,
        vm::Op::Halt,
    ];
    let branch = [
        vm::Op::Push(Ternary::Unknown),
        vm::Op::Jz(5),
        vm::Op::Push(Ternary::True),
        vm::Op::Halt,
        vm::Op::Halt,
        vm::Op::Push(Ternary::False),
        vm::Op::Halt,
    ];
    let describe = |report: &vm::RunReport| {
        serde_json::json!({
            "verdict": report.verdict().map(|value| value.as_char().to_string()),
            "outputs": report.outputs.iter().map(|(value, paths)| {
                json_pair(value.as_char().to_string(), *paths)
            }).collect::<serde_json::Map<_, _>>(),
            "paths": report.paths,
            "steps": report.steps,
            "halted": report.halted,
            "truncated": report.truncated,
        })
    };
    let obj = serde_json::json!({
        "x_and_zero": describe(&vm::run(&masking, vm::Limits::default())),
        "x_and_not_x_kleene": describe(&vm::run(&correlated, vm::Limits::default())),
        "branch_on_x": describe(&vm::run(&branch, vm::Limits::default())),
    });
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    Ok(())
}

fn json_pair(key: String, value: usize) -> (String, serde_json::Value) {
    (key, serde_json::Value::from(value))
}

fn ca_demo(args: &[String]) -> Result<(), String> {
    let width = number(args, "--width", 64)?;
    let steps = number(args, "--steps", 200)?;
    let survey = ca::survey(width, steps);
    let best = ca::Rule::from_index(survey.max_entropy_rule).expect("合法规则编号");
    let run = ca::run(&best, width, steps);
    let obj = serde_json::json!({
        "rules": 2187,
        "width": width,
        "steps": steps,
        "classes": {
            "dead": survey.dead,
            "fixed": survey.fixed,
            "periodic": survey.periodic,
            "complex": survey.complex,
        },
        "max_entropy_rule": survey.max_entropy_rule,
        "max_entropy": survey.max_entropy,
        "max_entropy_run": {
            "density": run.density,
            "entropy": run.entropy,
            "period": run.period,
        },
    });
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    Ok(())
}

fn snn_demo(args: &[String]) -> Result<(), String> {
    let size = number(args, "--size", 100)?;
    let ticks = number(args, "--ticks", 200)?;
    let seed = number(args, "--seed", 7)? as u64;
    let kick = number(args, "--kick", 3)? as i32;
    let drive = number(args, "--drive", 0)? as i32;
    let threshold = number(args, "--threshold", 3)? as i32;
    let mut network = snn::Network::random(size, seed);
    network.threshold = threshold;
    let external: Vec<i32> = if drive > 0 {
        vec![drive; ticks]
    } else {
        vec![kick]
    };
    let stats =
        snn::simulate_with_drive(&network, ticks, &external).map_err(|error| error.to_string())?;
    let obj = serde_json::json!({
        "neurons": size,
        "ticks": ticks,
        "threshold": network.threshold,
        "decay": network.decay,
        "refractory": network.refractory,
        "kick": kick,
        "spikes": stats.spikes,
        "positive": stats.positive,
        "negative": stats.negative,
        "active_fraction": stats.active_fraction,
        "spikes_per_neuron_tick": stats.spikes_per_neuron_tick(size),
    });
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    Ok(())
}
