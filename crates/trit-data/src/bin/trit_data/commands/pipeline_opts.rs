//! `pipeline` 的参数解析：仓库模式与轨迹模式共用一段扫描，避免两边各写一套。

use std::collections::BTreeMap;
use std::path::PathBuf;

use trit_data::gate::Config as GateConfig;
use trit_data::repo_gate::Config as RepoConfig;

use super::die;

pub struct TrajectoryOpts {
    pub input: PathBuf,
    pub gate: GateConfig,
    pub rewrite_cmd: Option<String>,
    pub rewrite_timeout: u64,
    pub mix_total: Option<usize>,
    pub mix_weights: BTreeMap<String, f64>,
}

pub struct Opts {
    pub dir: PathBuf,
    pub root: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub promote: bool,
    pub repo_cfg: RepoConfig,
    pub trajectory: Option<TrajectoryOpts>,
}

pub fn parse(argv: &[String]) -> Opts {
    let mut root: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut config: Option<PathBuf> = None;
    let mut trajectories: Option<PathBuf> = None;
    let mut rewrite_cmd: Option<String> = None;
    let mut rewrite_timeout = 30u64;
    let mut mix_total: Option<usize> = None;
    let mut mix_weights: BTreeMap<String, f64> = BTreeMap::new();
    let mut promote = false;
    let mut repo_cfg = RepoConfig::default();
    let mut repo_flag = false;
    let mut gate_argv: Vec<String> = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--repo" => {
                root = argv.get(i + 1).map(PathBuf::from);
                repo_flag = true;
                i += 2;
            }
            "--config" => {
                config = argv.get(i + 1).map(PathBuf::from);
                repo_flag = true;
                i += 2;
            }
            "--promote" => {
                promote = true;
                repo_flag = true;
                i += 1;
            }
            "--require-license" => {
                repo_cfg.require_license = true;
                repo_flag = true;
                i += 1;
            }
            "--strict-license" => {
                repo_cfg.reject_missing_license = true;
                repo_flag = true;
                i += 1;
            }
            "--require-tests" => {
                repo_cfg.require_tests = true;
                repo_flag = true;
                i += 1;
            }
            "--allow-generated" => {
                repo_cfg.reject_generated = false;
                repo_flag = true;
                i += 1;
            }
            "--allow-duplicates" => {
                repo_cfg.reject_duplicates = false;
                repo_flag = true;
                i += 1;
            }
            "--trajectories" => {
                trajectories = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--cmd" => {
                rewrite_cmd = argv.get(i + 1).cloned();
                i += 2;
            }
            "--rewrite-timeout" => {
                rewrite_timeout = argv
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| die("--rewrite-timeout 需要整数秒"));
                i += 2;
            }
            "--mix-total" => {
                mix_total = Some(
                    argv.get(i + 1)
                        .and_then(|v| v.parse().ok())
                        .unwrap_or_else(|| die("--mix-total 需要整数")),
                );
                i += 2;
            }
            "--mix-weights" => {
                for part in argv.get(i + 1).map(|s| s.as_str()).unwrap_or("").split(',') {
                    let (key, value) = part
                        .split_once('=')
                        .unwrap_or_else(|| die("--mix-weights 形如 code=0.7,writing=0.3"));
                    mix_weights.insert(
                        key.to_string(),
                        value.parse().unwrap_or_else(|_| die("权重需要小数")),
                    );
                }
                i += 2;
            }
            "--out" => {
                out = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            other => {
                gate_argv.push(other.to_string());
                i += 1;
            }
        }
    }
    let dir = out.unwrap_or_else(|| PathBuf::from("pipeline-out"));

    let trajectory = trajectories.map(|input| {
        if repo_flag || config.is_some() || root.is_some() {
            die("--trajectories 与 --repo/--config/--promote/仓库门参数不能混用");
        }
        let mut args = vec!["--jsonl".to_string(), input.display().to_string()];
        args.extend(gate_argv.clone());
        let parsed = super::gate::gate_opts::parse(&args);
        TrajectoryOpts {
            input,
            gate: parsed.cfg,
            rewrite_cmd: rewrite_cmd.clone(),
            rewrite_timeout,
            mix_total,
            mix_weights: mix_weights.clone(),
        }
    });
    if trajectory.is_none() {
        if rewrite_cmd.is_some() || mix_total.is_some() || !mix_weights.is_empty() {
            die("--cmd/--mix-total/--mix-weights 只用于 --trajectories");
        }
        if !gate_argv.is_empty() {
            die(&format!("未知参数 {}", gate_argv[0]));
        }
    }
    Opts {
        dir,
        root,
        config,
        promote,
        repo_cfg,
        trajectory,
    }
}
