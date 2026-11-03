use std::path::PathBuf;

use trit_learn::Ste;

pub(crate) struct Args {
    pub(crate) data: Vec<PathBuf>,
    pub(crate) init: Option<PathBuf>,
    pub(crate) resume: Option<PathBuf>,
    pub(crate) save: Option<PathBuf>,
    /// 进程存档目录：存在则自动续跑，运行中按 save-every 原子落盘。
    pub(crate) state: Option<PathBuf>,
    pub(crate) selfcheck: Option<PathBuf>,
    pub(crate) eval: Option<PathBuf>,
    pub(crate) k: usize,
    pub(crate) dump: Option<PathBuf>,
    pub(crate) res: u32,
    pub(crate) manifest: Option<PathBuf>,
    pub(crate) steps: usize,
    pub(crate) lr: f32,
    /// 终值学习率（线性衰减）；缺省等于 lr。
    pub(crate) lr_final: Option<f32>,
    pub(crate) accum: usize,
    pub(crate) ste: Ste,
    pub(crate) adam: bool,
    pub(crate) seed: u64,
    pub(crate) limit: usize,
    pub(crate) report: usize,
    pub(crate) save_every: usize,
    pub(crate) eval_every: usize,
    /// 从零训练时的通道宽度倍数（1.0 = 32/64/128；1.5 = 48/96/192）。
    pub(crate) width: f32,
    /// net2net：从这份断点把通道搬进按 `--width` 放大的新网络（再精调）。
    pub(crate) expand_from: Option<PathBuf>,
    /// 块内残差（cin==cout 的卷积层）。
    pub(crate) residual: bool,
    /// 可拆卸加速层（opt-in）：把单样本内的所有 dispatch 收进一个 encoder 再提交。
    /// 数学完全不变；实测在 GPU 被占用时无加速，默认关，GPU 空闲时再评估。
    pub(crate) batch: bool,
    /// 合成数据：光度抖动 + 噪声（开关）。
    pub(crate) augment: bool,
    /// 额外做带标签平移（像素上限，0 = 不做）。
    pub(crate) augment_shift: i32,
    /// 深度头：附加的三值平面数（0 = 关）。
    pub(crate) depth_planes: usize,
    /// 深度项权重。
    pub(crate) depth_weight: f32,
    /// 视差区间（1/米）。
    pub(crate) depth_lo: f32,
    pub(crate) depth_hi: f32,
    /// head 的学习率倍率。
    pub(crate) head_lr: f32,
    /// 冻结除最后一层外的所有层。
    pub(crate) freeze: bool,
    /// 只保留最后 N 层可训练（0 = 不额外冻结）。
    pub(crate) unfreeze_last: usize,
    /// 冻结分割 head。
    pub(crate) freeze_head: bool,
    /// 标签子目录（缺省 `seg`；实例标签用 `inst`、边界标签用 `bnd`）。
    pub(crate) seg_sub: String,
    /// 稀疏类加权：标签 id 等于该值的像素 loss × 倍数（0 = 不加权）。
    pub(crate) bnd_label: u32,
    pub(crate) bnd_weight: f32,
    pub(crate) id_weight: f32,
}

pub(crate) fn parse() -> Result<Args, String> {
    let mut a = Args {
        data: Vec::new(),
        init: None,
        resume: None,
        save: None,
        state: None,
        selfcheck: None,
        eval: None,
        k: 5,
        dump: None,
        res: 384,
        manifest: None,
        steps: 100,
        lr: 1e-3,
        lr_final: None,
        accum: 1,
        ste: Ste::Identity,
        adam: true,
        seed: 1,
        limit: 0,
        report: 10,
        save_every: 1000,
        eval_every: 0,
        width: 1.0,
        expand_from: None,
        residual: false,
        batch: false,
        augment: false,
        augment_shift: 0,
        depth_planes: 0,
        depth_weight: 0.5,
        depth_lo: 0.15,
        depth_hi: 0.65,
        head_lr: 1.0,
        freeze: false,
        unfreeze_last: 0,
        freeze_head: false,
        seg_sub: "seg".into(),
        bnd_label: 0,
        bnd_weight: 0.0,
        id_weight: 0.0,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let key = argv[i].as_str();
        let val = || {
            argv.get(i + 1)
                .cloned()
                .ok_or_else(|| format!("{key} 缺值"))
        };
        match key {
            "--data" => a.data = val()?.split(',').map(PathBuf::from).collect(),
            "--init" => a.init = Some(PathBuf::from(val()?)),
            "--resume" => a.resume = Some(PathBuf::from(val()?)),
            "--save" => a.save = Some(PathBuf::from(val()?)),
            "--state" => a.state = Some(PathBuf::from(val()?)),
            "--selfcheck" => a.selfcheck = Some(PathBuf::from(val()?)),
            "--eval" => a.eval = Some(PathBuf::from(val()?)),
            "--k" => a.k = val()?.parse().map_err(|_| "--k 非整数")?,
            "--dump" => a.dump = Some(PathBuf::from(val()?)),
            "--res" => a.res = val()?.parse().map_err(|_| "--res 非整数")?,
            "--manifest" => a.manifest = Some(PathBuf::from(val()?)),
            "--steps" => a.steps = val()?.parse().map_err(|_| "--steps 非整数")?,
            "--lr" => a.lr = val()?.parse().map_err(|_| "--lr 非数字")?,
            "--lr-final" => a.lr_final = Some(val()?.parse().map_err(|_| "--lr-final 非数字")?),
            "--accum" => a.accum = val()?.parse().map_err(|_| "--accum 非整数")?,
            "--seed" => a.seed = val()?.parse().map_err(|_| "--seed 非整数")?,
            "--limit" => a.limit = val()?.parse().map_err(|_| "--limit 非整数")?,
            "--report" => a.report = val()?.parse().map_err(|_| "--report 非整数")?,
            "--save-every" => a.save_every = val()?.parse().map_err(|_| "--save-every 非整数")?,
            "--eval-every" => a.eval_every = val()?.parse().map_err(|_| "--eval-every 非整数")?,
            "--width" => a.width = val()?.parse().map_err(|_| "--width 非数字")?,
            "--expand-from" => a.expand_from = Some(PathBuf::from(val()?)),
            "--residual" => a.residual = true,
            "--batch" => a.batch = true,
            "--augment" => a.augment = true,
            "--augment-shift" => {
                a.augment_shift = val()?.parse().map_err(|_| "--augment-shift 非整数")?
            }
            "--ste" => {
                a.ste = match val()?.as_str() {
                    "identity" => Ste::Identity,
                    "clipped" => Ste::Clipped { ratio: 1.0 },
                    "soft" => Ste::Soft,
                    other => return Err(format!("未知 STE {other}")),
                }
            }
            "--depth-planes" => {
                a.depth_planes = val()?.parse().map_err(|_| "--depth-planes 非整数")?
            }
            "--depth-weight" => {
                a.depth_weight = val()?.parse().map_err(|_| "--depth-weight 非数字")?
            }
            "--depth-lo" => a.depth_lo = val()?.parse().map_err(|_| "--depth-lo 非数字")?,
            "--depth-hi" => a.depth_hi = val()?.parse().map_err(|_| "--depth-hi 非数字")?,
            "--head-lr" => a.head_lr = val()?.parse().map_err(|_| "--head-lr 非数字")?,
            "--freeze-backbone" => a.freeze = true,
            "--freeze-head" => a.freeze_head = true,
            "--unfreeze-last" => {
                a.unfreeze_last = val()?.parse().map_err(|_| "--unfreeze-last 非整数")?
            }
            "--seg-sub" => a.seg_sub = val()?,
            "--bnd-label" => a.bnd_label = val()?.parse().map_err(|_| "--bnd-label 非整数")?,
            "--id-weight" => a.id_weight = val()?.parse().map_err(|_| "--id-weight 非数字")?,
            "--bnd-weight" => a.bnd_weight = val()?.parse().map_err(|_| "--bnd-weight 非数字")?,
            "--sgd" => a.adam = false,
            other => return Err(format!("未知参数 {other}")),
        }
        // 纯开关类参数只前进 1，否则会吞掉下一个参数（`--residual --steps 4000` 会把 --steps 吃掉）
        i += match key {
            "--sgd" | "--residual" | "--batch" | "--augment" | "--freeze-backbone"
            | "--freeze-head" => 1,
            _ => 2,
        };
    }
    if a.data.is_empty()
        && a.resume.is_none()
        && a.state.is_none()
        && a.selfcheck.is_none()
        && a.eval.is_none()
    {
        return Err("需要 --data / --resume / --selfcheck".into());
    }
    Ok(a)
}
