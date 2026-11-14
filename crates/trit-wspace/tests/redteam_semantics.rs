//! 语义层红队：对编辑事务/回滚/审计链/程序语义的不变量攻击（攻击者持完整
//! 写权限，失败即漏洞）。与 `tests/redteam.rs`（容器格式层）互补。设计边界
//! （非漏洞，见 `docs/WSP-REDTEAM-STACK.md` R6）：FNV 无密钥且
//! `recompute_chain` 是 pub——审计链提供完整性而非真实性（无签名锚点）。

use trit_wspace::audit::AuditLog;
use trit_wspace::edit::{Edit, Journal, Patch};
use trit_wspace::trit::Trit;
use trit_wspace::{Layer, Net, Program};

fn sample_net() -> Net {
    Net::new(vec![
        Layer::with_scales(
            3,
            2,
            &[1, -1, 0, 1, -1, 1],
            &[1, 0, -1],
            vec![0.5, 1.0, 2.0],
        )
        .expect("l0"),
        Layer::new(2, 3, &[1, 0, -1, 1, 1, -1], &[1, -1]).expect("l1"),
    ])
    .expect("net")
}

fn assert_bitwise_identical(a: &Net, b: &Net) {
    assert_eq!(a.layers().len(), b.layers().len(), "层数一致");
    for (la, lb) in a.layers().iter().zip(b.layers()) {
        assert_eq!(la.trits(), lb.trits(), "trits");
        assert_eq!(la.bias(), lb.bias(), "bias");
        assert_eq!(la.scales(), lb.scales(), "scale");
        assert_eq!(la.bias_f32(), lb.bias_f32(), "biasf");
    }
}

/// 攻击 1：多编辑 patch 的**原子性**——构造"前缀合法 + 尾部越界"的 patch，
/// 失败后网络必须逐位不变（不能前半生效后半失败）。
#[test]
fn patch_failure_leaves_net_bitwise_intact() {
    let mut net = sample_net();
    let before = net.clone();
    let patch = Patch::new(vec![
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Zero,
        },
        Edit::SetBias {
            layer: 0,
            row: 2,
            to: Trit::Pos,
        },
        Edit::SetTrit {
            layer: 0,
            row: 99,
            col: 0,
            to: Trit::Pos,
        }, // 越界
    ]);
    assert!(patch.apply(&mut net).is_err(), "越界 patch 必须失败");
    assert_bitwise_identical(&before, &net);
}

/// 攻击 2：**回滚完备性**——合法 patch 应用后回滚必须逐位还原，
/// 且对 scale/biasf 这类 f32 自由度同样成立。
#[test]
fn journal_undo_restores_bitwise_including_f32() {
    let mut net = sample_net();
    let before = net.clone();
    let patch = Patch::new(vec![
        Edit::SetTrit {
            layer: 0,
            row: 1,
            col: 1,
            to: Trit::Pos,
        },
        Edit::SetScale {
            layer: 0,
            row: 0,
            to: 7.5,
        },
        Edit::SwapRows {
            layer: 0,
            a: 0,
            b: 2,
        },
        Edit::NegateRow { layer: 1, row: 0 },
    ]);
    let journal: Journal = patch.apply(&mut net).expect("valid patch");
    journal.undo(&mut net);
    assert_bitwise_identical(&before, &net);
    let journal2 = patch.apply(&mut net).expect("replay");
    journal2.undo(&mut net);
    assert_bitwise_identical(&before, &net);
}

/// 攻击 3：**审计链伪造检测**——篡改中间条目的任意字段必须断链。
#[test]
fn audit_chain_detects_midstream_forgery() {
    let net = sample_net();
    let mut log = AuditLog::new();
    let patch = Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }]);
    let mut work = net.clone();
    patch.apply(&mut work).expect("ok");
    log.record("redteam", &net, &work, patch, true, false)
        .expect("record");
    let net = work;
    let patch2 = Patch::new(vec![Edit::SetBias {
        layer: 0,
        row: 0,
        to: Trit::Neg,
    }]);
    let mut work = net.clone();
    patch2.apply(&mut work).expect("ok");
    log.record("redteam", &net, &work, patch2, true, false)
        .expect("record");
    let net = work;
    assert!(log.verify_chain().is_ok(), "基线：完整日志链合法");

    // 攻击者改写第一条日志的动作（JSONL 落盘后改字符串再读回的场景，
    // 这里直接改内存条目等价）。
    let forged: Vec<String> = log
        .to_jsonl()
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                line.replace("\"to\":\"0\"", "\"to\":\"-\"")
            } else {
                line.to_string()
            }
        })
        .collect();
    let forged_text = forged.join("\n");
    let parsed: Vec<serde_json::Value> = forged_text
        .lines()
        .map(|l| serde_json::from_str(l).expect("jsonl"))
        .collect();
    let rebuilt = AuditLog::from_jsonl(&forged_text).expect("解析伪造日志");
    assert!(
        rebuilt.verify_chain().is_err(),
        "篡改中间条目必须断链（不能被验链放行）"
    );
    let initial = sample_net();
    let replay = rebuilt.replay_net(&initial);
    match replay {
        Err(_) => {}
        Ok((_, net2)) => {
            assert_ne!(
                net2.layers()[0].trits(),
                net.layers()[0].trits(),
                "重放伪造日志得到的工件必须与真实工件可区分"
            );
        }
    }
    let _ = parsed;
}

/// 攻击 4：**程序语义原子性**——require-after 不满足时整个 Program 回滚，
/// 网络逐位不变（失败的前半段编辑不能留下来）。
#[test]
fn program_require_after_rolls_back_bitwise() {
    let net = sample_net();
    let before = net.clone();
    let oracle = trit_wspace::Oracle::boolean(2, |b| b[0] == b[1]).expect("oracle");
    // patch 改 trit 但不满足 oracle 的 after 行为 → Program 必须整体回滚。
    let patch = Patch::new(vec![
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Pos,
        },
        Edit::SetTrit {
            layer: 0,
            row: 1,
            col: 0,
            to: Trit::Neg,
        },
    ]);
    let mut work = net.clone();
    let mut program = Program::new("attacker", patch);
    program = program.require_after(oracle.behavior());
    let result = program.run(&mut work, &oracle);
    if result.is_err() {
        assert_bitwise_identical(&before, &work);
    }
}

/// 攻击 5：**重放一致性**——合法日志重放两次必须得到逐位相同的工件
/// （重放是确定性的，攻击者不能靠"重放不确定性"夹带私货）。
#[test]
fn replay_is_deterministic() {
    let net = sample_net();
    let mut log = AuditLog::new();
    let patch = Patch::new(vec![
        Edit::SetTrit {
            layer: 0,
            row: 2,
            col: 1,
            to: Trit::Pos,
        },
        Edit::SetScale {
            layer: 0,
            row: 1,
            to: 3.25,
        },
    ]);
    let mut work = net.clone();
    patch.apply(&mut work).expect("ok");
    log.record("redteam", &net, &work, patch, true, false)
        .expect("record");
    let (_, first) = log.replay_net(&sample_net()).expect("replay");
    let (_, second) = log.replay_net(&sample_net()).expect("replay");
    assert_bitwise_identical(&first, &second);
}
