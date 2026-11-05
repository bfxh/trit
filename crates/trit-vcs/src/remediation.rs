//! 每条判决都附"下一步能直接执行的命令"。
//!
//! 为什么要这一层：智能体撞到门之后，原本只读到一句中文（`未认领：[…]`），
//! 于是它要么猜参数再撞一次，要么回头翻 README——两者都是额外的往返轮，而且猜错的轮更贵。
//! 这里把"解锁动作"变成判决的一部分：命令带真实参数（路径、会话名、ref 名、树 id），拿来就能跑。
//!
//! 口径：只给**仓库里真实存在**的入口（`cargo run -p trit-vcs -- …` 与
//! `python scripts/agent_claims.py …`），不虚构一个装到哪都未知的短命令。
//! 唯一保留的占位符是认领命令里的 `--repo <owner/repo>`：认领账本是**跨仓**的账，
//! 平台 slug 不在 trit-vcs 的仓目录里，猜出来就是假命令。

use std::path::Path;

use crate::repo::RepoError;
use crate::witness::WitnessError;

/// trit-vcs CLI 的真实调用前缀（在仓根跑）。
pub const VCS: &str = "cargo run -p trit-vcs --";
/// 跨智能体认领账本的入口（与库里的 `ClaimStore::gate` 读同一张表）。
pub const CLAIMS: &str = "python scripts/agent_claims.py";

/// 造命令需要的上下文：错误本身不带这些，但命令不带就跑不动。
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    /// 被操作的仓库目录（进 `--repo`）。
    pub dir: &'a Path,
    /// 当前会话名（`宿主:会话id`，进认领命令）。
    pub session: &'a str,
}

/// 判决的机器可读名字：给 JSON 分支用，与 Display 的中文文案解耦。
pub fn kind(err: &RepoError) -> &'static str {
    match err {
        RepoError::Io(_) => "io",
        RepoError::Store(_) => "store",
        RepoError::Journal(_) => "journal",
        RepoError::NotInitialized(_) => "not_initialized",
        RepoError::NothingStaged => "nothing_staged",
        RepoError::Unclaimed { .. } => "unclaimed",
        RepoError::Conflict { .. } => "claim_conflict",
        RepoError::UnknownCommit(_) => "unknown_commit",
        RepoError::BadHead(_) => "bad_head",
        RepoError::WitnessRequired => "witness_required",
        RepoError::Witness(inner) => match inner {
            WitnessError::Failed(_) => "witness_failed",
            WitnessError::TreeMismatch { .. } => "witness_tree_mismatch",
            WitnessError::FileTampered { .. } => "witness_file_tampered",
            _ => "witness_invalid",
        },
        RepoError::WitnessFailed(_) => "witness_command_failed",
        RepoError::Unwitnessed(_) => "commit_unwitnessed",
        RepoError::BadRefName(_) => "bad_ref_name",
        RepoError::BadRef(_) => "bad_ref",
        RepoError::UnknownRef(_) => "unknown_ref",
        RepoError::NonFastForward { .. } => "non_fast_forward",
        RepoError::IntentRequired => "intent_required",
        RepoError::IntentInvalid(_) => "intent_invalid",
        RepoError::IntentBaseMismatch { .. } => "intent_base_mismatch",
    }
}

/// `err` 的解锁命令，按优先级排（一般第一条就够）。
///
/// 只对**有对应动作**的判决给命令；纯协调类的（"等谁到期"）也直说是协调，不编跑不通的参数。
pub fn commands(err: &RepoError, ctx: Context<'_>) -> Vec<String> {
    let repo = ctx.dir.display().to_string();
    let session = ctx.session;
    match err {
        RepoError::NotInitialized(dir) => vec![format!("{VCS} init --repo {dir}")],
        RepoError::NothingStaged => vec![format!("{VCS} add <文件或目录> --repo {repo}")],
        RepoError::Unclaimed { paths } => {
            let listed = join_paths(paths);
            vec![
                format!(
                    "{CLAIMS} claim --who {session} --repo <owner/repo> --paths {listed} --ttl 90"
                ),
                format!("{VCS} commit --repo {repo} --claims <claims.json> …"),
                format!(
                    "{VCS} commit --repo {repo} --allow-unclaimed …   # 只在确认没人同时动这些路径时用"
                ),
            ]
        }
        RepoError::Conflict { others, paths } => {
            let listed = join_paths(paths);
            let mut out = vec![format!(
                "{CLAIMS} gate --who {session} --paths {listed} --json   # 看是谁挡着、还剩多少秒"
            )];
            for other in others.iter().take(4) {
                out.push(format!(
                    "等 {other} 的认领到期再重来（别替它 release；能拆就先把触碰集拆到不相交）"
                ));
            }
            out
        }
        RepoError::UnknownCommit(id) => {
            vec![format!("{VCS} log --json --repo {repo}   # 里面没有 {id}")]
        }
        RepoError::BadHead(_) | RepoError::Store(_) | RepoError::Journal(_) | RepoError::Io(_) => {
            vec![
                format!("{VCS} verify --repo {repo}"),
                format!("{VCS} stats --repo {repo}"),
            ]
        }
        RepoError::WitnessRequired => vec![format!(
            "{VCS} witness --repo {repo} -- <判据命令，如 cargo test -p trit-vcs>，然后 commit --witness <文件>"
        )],
        RepoError::Witness(inner) => match inner {
            WitnessError::TreeMismatch { .. } => vec![format!(
                "{VCS} witness --repo {repo} -- <命令>   # 重新生成：这份见证绑的树和要提交的树不是同一棵"
            )],
            WitnessError::FileTampered { .. } => vec![
                format!(
                    "{VCS} witness --repo {repo} -- <命令>   # 重新生成：见证文件在记录之后被改过"
                ),
                format!("{VCS} status --repo {repo}"),
            ],
            WitnessError::Failed(_) => vec![format!(
                "{VCS} verify --repo {repo}   # 先让判据命令本身跑绿；红测试的见证不能当证据"
            )],
            _ => vec![format!("{VCS} witness --repo {repo} -- <命令>")],
        },
        RepoError::WitnessFailed(_) => vec![format!(
            "{VCS} witness --repo {repo} -- <同一条判据命令>   # 这次记录里命令是红的"
        )],
        RepoError::Unwitnessed(id) => vec![format!(
            "{VCS} witness --repo {repo} -- <命令>，然后 {VCS} admit {id} --witness <文件> --repo {repo}"
        )],
        RepoError::BadRefName(name) => vec![format!(
            "换个 ref 名（{name:?} 不合规）：1–64 位 ASCII 字母数字或 . _ -，不含 /、不以 . 开头"
        )],
        RepoError::BadRef(_) => vec![format!("{VCS} verify --repo {repo}")],
        RepoError::UnknownRef(name) => vec![format!(
            "{VCS} publish {name} --central <仓目录> --repo <源仓>   # 中央仓还没有这个 ref"
        )],
        RepoError::NonFastForward {
            name,
            current,
            incoming,
        } => vec![
            format!(
                "{VCS} pull {name} --central <仓目录> --repo <本地>   # 先把别人的工作拉下来再接着自己的做"
            ),
            format!(
                "在 {current} 之上重做这次提交（{incoming} 与它分叉，直接顶会把别人的历史甩掉）"
            ),
            format!("{VCS} publish {name} --central <仓目录> --force   # 只在确定要改写这条线时用"),
        ],
        RepoError::IntentRequired => vec![format!(
            "{VCS} commit --repo {repo} --goal <这次要达成什么> --accept <可核验的判据> …"
        )],
        RepoError::IntentInvalid(_) => vec![format!(
            "{VCS} log --json --repo {repo}   # 照已有提交的 intent 形状改：goal 非空、至少一条 accept"
        )],
        RepoError::IntentBaseMismatch { actual, .. } => vec![
            format!("把 intent.base 换成父树的 {actual}，或留空让提交自己绑父树"),
            format!("{VCS} log --json --repo {repo}"),
        ],
    }
}

/// 打成"结论 + 编号的下一步"，CLI 的 stderr 与 MCP 的 content 共用一份格式。
///
/// 智能体拿到这段就能直接执行，不需要再猜一次参数——所以命令必须是可跑的，
/// 且第一条永远是最不破坏别人工作的那条。
pub fn explain(err: &RepoError, ctx: Context<'_>) -> String {
    let mut out = format!("{}（{}）", err, kind(err));
    for (index, command) in commands(err, ctx).iter().enumerate() {
        out.push_str(&format!("\n  {}. {}", index + 1, command));
    }
    out
}

/// 失败的结构化形状：`--json` 下每条失败都是一个对象，智能体按 `kind` 分支而不是抠文案。
pub fn explain_json(err: &RepoError, ctx: Context<'_>) -> serde_json::Value {
    serde_json::json!({
        "ok": false,
        "error": {
            "kind": kind(err),
            "message": err.to_string(),
        },
        "remediation": commands(err, ctx),
    })
}

/// 路径太多时点名前 6 条 + 给总数，不把几十条路径灌进一次报错。
fn join_paths(paths: &[String]) -> String {
    const SHOW: usize = 6;
    let shown: Vec<&str> = paths.iter().take(SHOW).map(String::as_str).collect();
    if paths.len() > shown.len() {
        format!("{} …（共 {} 条）", shown.join(" "), paths.len())
    } else {
        shown.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn ctx() -> Context<'static> {
        Context {
            dir: Path::new("/tmp/w"),
            session: "qoder:s1",
        }
    }

    fn every_variant() -> Vec<RepoError> {
        vec![
            RepoError::NotInitialized("/x/.tritvcs".to_string()),
            RepoError::NothingStaged,
            RepoError::Unclaimed {
                paths: vec!["a/src".to_string(), "b/src".to_string()],
            },
            RepoError::Conflict {
                others: vec!["qoder:other#c7".to_string()],
                paths: vec!["a/src".to_string()],
            },
            RepoError::UnknownCommit("deadbeef".to_string()),
            RepoError::BadHead("zzz".to_string()),
            RepoError::Store(crate::store::StoreError::BadMap("坏映射".to_string())),
            RepoError::Journal(crate::journal::JournalError::Broken { line: 3 }),
            RepoError::Io(std::io::Error::other("读不动")),
            RepoError::WitnessRequired,
            RepoError::Witness(WitnessError::TreeMismatch {
                subject: "aa".to_string(),
                actual: "bb".to_string(),
            }),
            RepoError::Witness(WitnessError::FileTampered {
                recorded: "aa".to_string(),
                actual: "bb".to_string(),
            }),
            RepoError::Witness(WitnessError::Failed(101)),
            RepoError::WitnessFailed(101),
            RepoError::Unwitnessed("cafe".to_string()),
            RepoError::BadRefName("a/b".to_string()),
            RepoError::BadRef("zzz".to_string()),
            RepoError::UnknownRef("main".to_string()),
            RepoError::NonFastForward {
                name: "main".to_string(),
                current: "aaaa".to_string(),
                incoming: "bbbb".to_string(),
            },
            RepoError::IntentRequired,
            RepoError::IntentInvalid("goal 不能为空".to_string()),
            RepoError::IntentBaseMismatch {
                declared: "aa".to_string(),
                actual: "bb".to_string(),
            },
        ]
    }

    #[test]
    fn every_verdict_ships_a_first_action() {
        for err in every_variant() {
            let lines = commands(&err, ctx());
            assert!(!lines.is_empty(), "{err} 没给下一步");
            let first = lines.first().unwrap();
            let actionable = first.starts_with(VCS)
                || first.starts_with(CLAIMS)
                || first.starts_with("换个 ref 名")
                || first.starts_with("在 ")
                || first.starts_with("把 intent.base");
            assert!(actionable, "{err} 的第一条不是可执行动作：{first}");
            assert!(
                !first.contains('{') && !first.contains('}'),
                "命令里漏了未格式化的占位符：{first}"
            );
        }
    }

    #[test]
    fn claim_commands_carry_the_real_paths_and_session() {
        let lines = commands(
            &RepoError::Unclaimed {
                paths: vec!["crates/x/src/a.rs".to_string()],
            },
            ctx(),
        );
        let claim = &lines[0];
        assert!(claim.contains("--who qoder:s1"), "{claim}");
        assert!(claim.contains("crates/x/src/a.rs"), "{claim}");
        assert!(claim.contains("--ttl"), "{claim}");
    }

    #[test]
    fn conflict_names_the_other_side_but_never_tells_me_to_release_it() {
        let joined = commands(
            &RepoError::Conflict {
                others: vec!["zcode:zoder#c12".to_string()],
                paths: vec!["crates/x".to_string()],
            },
            ctx(),
        )
        .join("\n");
        assert!(joined.contains("zcode:zoder#c12"), "{joined}");
        assert!(
            !joined.contains("release --who zcode"),
            "不该教智能体替别人撤销认领：{joined}"
        );
    }

    #[test]
    fn many_paths_are_counted_not_dumped() {
        let paths: Vec<String> = (0..40).map(|i| format!("gen/f{i}")).collect();
        let first = &commands(&RepoError::Unclaimed { paths }, ctx())[0];
        assert!(first.contains("共 40 条"), "{first}");
        assert!(first.matches("gen/f").count() <= 6, "{first}");
    }

    #[test]
    fn explain_text_numbers_every_command() {
        let text = explain(
            &RepoError::Unclaimed {
                paths: vec!["a.txt".to_string()],
            },
            ctx(),
        );
        assert!(text.contains("（unclaimed）"), "{text}");
        assert!(
            text.contains("\n  1. python scripts/agent_claims.py claim"),
            "{text}"
        );
        assert!(text.contains("\n  2. cargo run -p trit-vcs --"), "{text}");
    }

    #[test]
    fn explain_json_has_kind_and_remediation_array() {
        let value = explain_json(
            &RepoError::Conflict {
                others: vec!["zcode:z#c1".to_string()],
                paths: vec!["a.txt".to_string()],
            },
            ctx(),
        );
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["kind"], "claim_conflict");
        let list = value["remediation"].as_array().unwrap();
        assert!(!list.is_empty());
        assert!(list[0].as_str().unwrap().contains("--json"));
    }

    #[test]
    fn fast_forward_advice_offers_pull_before_force() {
        let lines = commands(
            &RepoError::NonFastForward {
                name: "main".to_string(),
                current: "aaaa".to_string(),
                incoming: "bbbb".to_string(),
            },
            ctx(),
        );
        assert!(lines[0].contains("pull main"), "{}", lines[0]);
        assert!(
            lines.last().unwrap().contains("--force"),
            "改写选项必须排最后：{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("aaaa")), "{lines:?}");
    }

    #[test]
    fn kinds_are_snake_case_and_no_two_verdicts_share_one() {
        let mut seen = BTreeSet::new();
        for err in every_variant() {
            let name = kind(&err);
            assert!(
                name.chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch == '_' || ch.is_ascii_digit()),
                "{name} 不是 snake_case"
            );
            assert!(seen.insert(name), "两条判决共用同一个 kind：{name}");
        }
        assert_eq!(kind(&RepoError::NothingStaged), "nothing_staged");
        assert_eq!(
            kind(&RepoError::Witness(WitnessError::Failed(1))),
            "witness_failed"
        );
    }
}
