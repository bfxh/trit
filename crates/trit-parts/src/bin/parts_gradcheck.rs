//! 反向内核的 GPU ↔ CPU 对拍。
//!
//! 判据：同一组输入下，GPU 的 `dx / dE / db` 与 CPU（f64）参考实现的**最大相对误差 < 2e-3**
//! （f32 累加噪声量级；真错误会是 O(1)）。同时检查三值路径（`E = q·scale`）与稠密路径在
//! 同一 `E` 下一致。
//!
//! 跑法：`cargo run -p trit-parts --features gpu --bin parts-gradcheck`

use trit_parts::backward::{
    ConvSpec, cat_backward_f64, cat_f64, conv_backward_f64, conv_f64, convt_backward_f64,
    convt_f64, pool_backward_f64, pool_f64,
};
use trit_parts::gpu::Gpu;
use trit_parts::weights::ConvW;

fn rng(seed: &mut u64) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    ((*seed % 2001) as f32 - 1000.0) / 1000.0
}

fn fill(seed: &mut u64, n: usize) -> Vec<f32> {
    (0..n).map(|_| rng(seed)).collect()
}

fn to_f64(v: &[f32]) -> Vec<f64> {
    v.iter().map(|&x| x as f64).collect()
}

fn max_rel(gpu: &[f32], cpu: &[f64]) -> f64 {
    assert_eq!(gpu.len(), cpu.len(), "长度不一致");
    let mut worst = 0.0f64;
    for (&g, &c) in gpu.iter().zip(cpu) {
        let scale = (g.abs() as f64).max(c.abs()).max(1e-6);
        worst = worst.max((g as f64 - c).abs() / scale);
    }
    worst
}

struct Check {
    name: &'static str,
    worst: f64,
}

fn main() {
    let gpu = match Gpu::new() {
        Some(g) => g,
        None => {
            eprintln!("没有可用的 GPU 适配器");
            std::process::exit(2);
        }
    };
    let mut checks: Vec<Check> = Vec::new();
    let mut seed = 0x5eed_1234_9abc_def1u64;

    // ---------------- 卷积（kk=3、有 ReLU、三值权重路径）
    // vec4 内核要求 cout 是 4 的倍数（真实网络的 cout ∈ {16,32,64,128}）
    let (h, w, cin, cout, kk) = (4u32, 5u32, 4u32, 4u32, 3u32);
    let spec = ConvSpec {
        h: h as usize,
        w: w as usize,
        cin: cin as usize,
        cout: cout as usize,
        kk: kk as usize,
        relu: true,
    };
    let x = fill(&mut seed, (h * w * cin) as usize);
    let bias = fill(&mut seed, cout as usize);
    let nw = (kk * kk * cin * cout) as usize;
    let latent = fill(&mut seed, nw);
    let mut qt = vec![0i32; nw];
    let mut ws = vec![0f32; cout as usize];
    let mut e = vec![0f32; nw];
    for (oc, slot) in ws.iter_mut().enumerate() {
        *slot = 0.4 + 0.2 * oc as f32;
    }
    for wi in 0..nw {
        let oc = wi % cout as usize;
        let s = ws[oc];
        let q = (latent[wi] / s).round().clamp(-1.0, 1.0) as i32;
        qt[wi] = q;
        e[wi] = q as f32 * s;
    }
    let cw = ConvW {
        wf: e.clone(),
        wt: qt.clone(),
        ws: ws.clone(),
        bias: bias.clone(),
        cin,
        cout,
        kk,
    };

    let xb = gpu.buf(&x, "x");
    let wfb = gpu.buf(&cw.wf, "wf");
    let wtb = gpu.ibuf(&cw.wt, "wt");
    let wsb = gpu.buf(&cw.ws, "ws");
    let bb = gpu.buf(&cw.bias, "b");
    let yb = gpu.conv(&xb, h, w, &cw, 1, 1, &wfb, &wtb, &wsb, &bb);
    let y = gpu.read(&yb, (h * w * cout) as usize);
    let dy = fill(&mut seed, y.len());
    let dyb = gpu.buf(&dy, "dy");

    // CPU 参考：用同一份 E / x / b，f64 重算
    let (x64, e64, b64, dy64) = (to_f64(&x), to_f64(&e), to_f64(&bias), to_f64(&dy));
    let y64 = conv_f64(&x64, &e64, &b64, &spec);
    let grad = conv_backward_f64(&x64, &y64, &dy64, &e64, &spec);

    // dx 走转置权重布局 [k·cout + oc]·cin + ic
    let mut et = vec![0f32; nw];
    for k in 0..(kk * kk) as usize {
        for ic in 0..cin as usize {
            for oc in 0..cout as usize {
                et[(k * cout as usize + oc) * cin as usize + ic] =
                    e[(k * cin as usize + ic) * cout as usize + oc];
            }
        }
    }
    let etb = gpu.buf(&et, "et");
    let dym = gpu.mask_dy(&yb, &dyb, (h * w * cout) as usize, true);
    let dxb = gpu.bwd_conv_dx(h, w, &cw, 1, &dym, &wfb, &etb);
    let dwb = gpu.bwd_conv_dw(h, w, &cw, 1, &xb, &yb, &dyb);
    let dbb = gpu.bwd_conv_db(h, w, &cw, 1, &yb, &dyb);
    checks.push(Check {
        name: "conv dx (trit)",
        worst: max_rel(&gpu.read(&dxb, (h * w * cin) as usize), &grad.dx),
    });
    checks.push(Check {
        name: "conv dE (trit)",
        worst: max_rel(&gpu.read(&dwb, nw), &grad.dw),
    });
    checks.push(Check {
        name: "conv db (trit)",
        worst: max_rel(&gpu.read(&dbb, cout as usize), &grad.db),
    });

    // 前向 vec4 vs 标量：同一个 E 必须一致（parity 门曾抓到过不一致）
    let y_scalar = gpu.conv(&xb, h, w, &cw, 0, 1, &wfb, &wtb, &wsb, &bb);
    let y_vec4 = gpu.conv_t(&xb, h, w, &cw, true, &wfb, &bb);
    let ys = gpu.read(&y_scalar, (h * w * cout) as usize);
    let yv = gpu.read(&y_vec4, (h * w * cout) as usize);
    let mut worst_fwd = 0f64;
    for (a, b) in ys.iter().zip(&yv) {
        let sc = (a.abs().max(b.abs()).max(1e-6)) as f64;
        worst_fwd = worst_fwd.max((*a as f64 - *b as f64).abs() / sc);
    }
    checks.push(Check {
        name: "forward vec4 ≡ scalar",
        worst: worst_fwd,
    });

    // split-K 版 dw/db 必须与 CPU 参考一致（训练走的就是这条路径）
    let dwk = gpu.bwd_conv_dw_split(h, w, &cw, 1, &xb, &dym);
    let dbk = gpu.bwd_conv_db_split(h, w, &cw, 1, &dym);
    checks.push(Check {
        name: "conv dE (split-K)",
        worst: max_rel(&gpu.read(&dwk, nw), &grad.dw),
    });
    checks.push(Check {
        name: "conv db (split-K)",
        worst: max_rel(&gpu.read(&dbk, cout as usize), &grad.db),
    });
    // 累加器语义：add_into 两次 == 两份梯度之和
    let acc = gpu.zeros(nw, "acc");
    gpu.add_into(&acc, &dwb, nw);
    gpu.add_into(&acc, &dwk, nw);
    let sum_gpu = gpu.read(&acc, nw);
    let want: Vec<f64> = grad.dw.iter().map(|v| v * 2.0).collect();
    checks.push(Check {
        name: "add_into 两次 = 2×dw",
        worst: max_rel(&sum_gpu, &want),
    });

    // 稠密路径：同一个 E，mode=0 必须给出同一组梯度
    let cw0 = ConvW {
        wf: e.clone(),
        wt: vec![0; nw],
        ws: vec![1.0; cout as usize],
        bias: bias.clone(),
        cin,
        cout,
        kk,
    };
    let _ = gpu.conv(&xb, h, w, &cw0, 0, 1, &wfb, &wtb, &wsb, &bb);
    let dx0 = gpu.bwd_conv_dx(h, w, &cw0, 1, &dym, &wfb, &etb);
    let dx0v = gpu.read(&dx0, (h * w * cin) as usize);
    let dxv = gpu.read(&dxb, (h * w * cin) as usize);
    let worst_mode = dxv
        .iter()
        .zip(&dx0v)
        .map(|(a, b)| ((a - b).abs() / a.abs().max(b.abs()).max(1e-6)) as f64)
        .fold(0.0f64, f64::max);
    checks.push(Check {
        name: "conv dx: mode1 ≡ mode0",
        worst: worst_mode,
    });

    // ---------------- 转置卷积（三值）
    let (ch, cw_, ccin, ccout) = (3u32, 4u32, 4u32, 4u32);
    let cx = fill(&mut seed, (ch * cw_ * ccin) as usize);
    let cbias = fill(&mut seed, ccout as usize);
    let cnw = (4 * ccin * ccout) as usize;
    let clatent = fill(&mut seed, cnw);
    let mut cqt = vec![0i32; cnw];
    let mut cws = vec![0f32; ccout as usize];
    let mut ce = vec![0f32; cnw];
    for (co, slot) in cws.iter_mut().enumerate() {
        *slot = 0.3 + 0.15 * co as f32;
    }
    for wi in 0..cnw {
        let co = wi % ccout as usize;
        let s = cws[co];
        let q = (clatent[wi] / s).round().clamp(-1.0, 1.0) as i32;
        cqt[wi] = q;
        ce[wi] = q as f32 * s;
    }
    let cwv = ConvW {
        wf: ce.clone(),
        wt: cqt.clone(),
        ws: cws.clone(),
        bias: cbias.clone(),
        cin: ccin,
        cout: ccout,
        kk: 2,
    };
    let cxb = gpu.buf(&cx, "cx");
    let cwfb = gpu.buf(&cwv.wf, "cwf");
    let cwtb = gpu.ibuf(&cwv.wt, "cwt");
    let cwsb = gpu.buf(&cwv.ws, "cws");
    let cbb = gpu.buf(&cwv.bias, "cb");
    let cyb = gpu.convt(&cxb, ch, cw_, &cwv, 1, &cwfb, &cwtb, &cwsb, &cbb);
    let cy = gpu.read(&cyb, ((ch * 2) * (cw_ * 2) * ccout) as usize);
    let cdy = fill(&mut seed, cy.len());
    let cdyb = gpu.buf(&cdy, "cdy");
    let cy64 = convt_f64(
        &to_f64(&cx),
        &to_f64(&ce),
        &to_f64(&cbias),
        ch as usize,
        cw_ as usize,
        ccin as usize,
        ccout as usize,
    );
    let cgrad = convt_backward_f64(
        &to_f64(&cx),
        &to_f64(&cdy),
        &to_f64(&ce),
        ch as usize,
        cw_ as usize,
        ccin as usize,
        ccout as usize,
    );
    let _ = cy64;
    let mut cet = vec![0f32; cnw];
    for s in 0..4usize {
        for ci in 0..ccin as usize {
            for co in 0..ccout as usize {
                cet[(s * ccout as usize + co) * ccin as usize + ci] =
                    ce[(s * ccin as usize + ci) * ccout as usize + co];
            }
        }
    }
    let cetb = gpu.buf(&cet, "cet");
    let cdxb = gpu.bwd_convt_dx(ch, cw_, &cwv, 1, &cdyb, &cetb);
    let cdwb = gpu.bwd_convt_dw(ch, cw_, &cwv, &cxb, &cdyb);
    let cdbb = gpu.bwd_convt_db(ch, cw_, &cwv, &cdyb);
    checks.push(Check {
        name: "convt dx (trit)",
        worst: max_rel(&gpu.read(&cdxb, (ch * cw_ * ccin) as usize), &cgrad.dx),
    });
    checks.push(Check {
        name: "convt dE (trit)",
        worst: max_rel(&gpu.read(&cdwb, cnw), &cgrad.dw),
    });
    checks.push(Check {
        name: "convt db (trit)",
        worst: max_rel(&gpu.read(&cdbb, ccout as usize), &cgrad.db),
    });

    // ---------------- 池化
    let (ph, pw, pc) = (4u32, 6u32, 2u32);
    let px = fill(&mut seed, (ph * pw * pc) as usize);
    let py = pool_f64(&to_f64(&px), ph as usize, pw as usize, pc as usize);
    let pdy = fill(&mut seed, py.len());
    let pdx_ref = pool_backward_f64(
        &to_f64(&px),
        &to_f64(&pdy),
        ph as usize,
        pw as usize,
        pc as usize,
    );
    let pxb = gpu.buf(&px, "px");
    let pdyb = gpu.buf(&pdy, "pdy");
    let pdxb = gpu.bwd_pool_dx(ph, pw, pc, &pxb, &pdyb);
    checks.push(Check {
        name: "pool dx",
        worst: max_rel(&gpu.read(&pdxb, (ph * pw * pc) as usize), &pdx_ref),
    });

    // ---------------- 拼接
    let (hw, ca, cb) = (6u32, 2u32, 3u32);
    let a = fill(&mut seed, (hw * ca) as usize);
    let b = fill(&mut seed, (hw * cb) as usize);
    let cy2 = cat_f64(
        &to_f64(&a),
        &to_f64(&b),
        hw as usize,
        ca as usize,
        cb as usize,
    );
    let cdy2 = fill(&mut seed, cy2.len());
    let (da_ref, db_ref) = cat_backward_f64(&to_f64(&cdy2), hw as usize, ca as usize, cb as usize);
    let cdyb2 = gpu.buf(&cdy2, "cdy2");
    let (dab, dbb2) = gpu.bwd_cat_dx(hw, ca, cb, &cdyb2);
    checks.push(Check {
        name: "cat dx (a)",
        worst: max_rel(&gpu.read(&dab, (hw * ca) as usize), &da_ref),
    });
    checks.push(Check {
        name: "cat dx (b)",
        worst: max_rel(&gpu.read(&dbb2, (hw * cb) as usize), &db_ref),
    });

    // ---------------- 汇总
    const LIMIT: f64 = 2e-3;
    println!("反向内核 GPU ↔ CPU 对拍（f64 参考，判据 max 相对误差 < {LIMIT:e}）");
    println!("{:<26} {:>12}  判定", "检查", "max rel");
    let mut failed = 0;
    for c in &checks {
        let ok = c.worst < LIMIT;
        if !ok {
            failed += 1;
        }
        println!(
            "{:<26} {:>12.3e}  {}",
            c.name,
            c.worst,
            if ok { "PASS" } else { "FAIL" }
        );
    }
    if failed > 0 {
        println!("{failed} 项失败");
        std::process::exit(1);
    }
    println!("全部通过（{} 项）", checks.len());
}
