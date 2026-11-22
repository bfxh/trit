//! `pipeline` 子命令：单仓库质量门、轨迹级 gate→rewrite→mix，或按配置批量跑并与基线对比。

use super::{die, pipeline_batch, pipeline_opts, pipeline_run, pipeline_trajectories, usage};

pub fn run(argv: &[String]) {
    let opts = pipeline_opts::parse(argv);
    if let Some(trajectory) = opts.trajectory {
        let t = pipeline_trajectories::Opts {
            gate: trajectory.gate,
            rewrite_cmd: trajectory.rewrite_cmd,
            rewrite_timeout: trajectory.rewrite_timeout,
            mix_total: trajectory.mix_total,
            mix_weights: trajectory.mix_weights,
        };
        let summary = pipeline_trajectories::execute(&trajectory.input, &opts.dir, &t)
            .unwrap_or_else(|e| die(&e));
        println!(
            "轨迹管线 {}：{} 个文件 / {} 条 → accept {} / defer {} / reject {}；重写 {}；混样 {}（{}）",
            trajectory.input.display(),
            summary.input_files,
            summary.records,
            summary.accept,
            summary.defer,
            summary.reject,
            if summary.rewrite_enabled {
                format!("{} 条", summary.rewritten)
            } else {
                "未启用".to_string()
            },
            summary.mixed,
            opts.dir.display()
        );
        return;
    }
    if let Some(path) = opts.config {
        pipeline_batch::run(&path, &opts.dir, opts.promote).unwrap_or_else(|e| die(&e));
        return;
    }
    let root = opts.root.unwrap_or_else(|| usage());
    let s = pipeline_run::execute(&root, &opts.dir, &opts.repo_cfg).unwrap_or_else(|e| die(&e));
    println!(
        "管线 {}：{} 文件 → accept {} / defer {} / reject {}（{}）",
        root.display(),
        s.files,
        s.accept,
        s.defer,
        s.reject,
        opts.dir.display()
    );
}
