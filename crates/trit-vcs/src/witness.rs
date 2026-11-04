//! witness：一次测试运行的离线可验证见证（in-toto Statement 形状，v0 不签名）。
//!
//! 目的不是"记录跑过什么"，而是让**合并准入**有可机械复核的输入：
//! 见证绑定一棵树（`subject.digest.blake3`），改名/换行/改一个字节都会让树变、绑定失效；
//! 见证文件本身以它自己的 BLAKE3 记进提交，改动见证文件同样会被拒。
//!
//! v0 的信任边界要说清楚：没有签名，所以它防的是**事后篡改与错配**，不防"谁都能自己造一份"。
//! 签名与公钥分发在 v1；在那之前，跨机器只把它当"未签名证据"，不当"可信命题"。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::hash::Hash;

/// in-toto Statement v1 的类型标识。
pub const STATEMENT_TYPE: &str = "https://in-toto.io/Statement/v1";
/// 本仓自定义的谓词类型：测试/命令运行。
pub const PREDICATE_TYPE: &str = "https://trit.dev/attestation/test-run/v1";
/// 见证结构版本。
pub const SCHEMA: u32 = 1;
/// 摘要算法名（与全仓一致用 BLAKE3，不是 SHA-256）。
pub const DIGEST_ALG: &str = "blake3";

/// in-toto subject：这里恰好一个，名字固定为被见证对象的角色。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    pub name: String,
    pub digest: BTreeMap<String, String>,
}

/// 运行的谓词：命令、退出码、输出摘要、时间。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Predicate {
    pub schema: u32,
    /// 运行前的 HEAD 树；第一条提交没有基线。
    pub base: Option<String>,
    pub command: Vec<String>,
    pub exit: i32,
    /// stdout/stderr 的 BLAKE3（十六进制）。
    pub stdout_hash: String,
    pub stderr_hash: String,
    pub started_ms: u64,
    pub elapsed_ms: u64,
    pub tool: String,
}

/// 一份完整的见证。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Witness {
    #[serde(rename = "_type")]
    pub statement_type: String,
    pub subject: Vec<Subject>,
    #[serde(rename = "predicateType")]
    pub predicate_type: String,
    pub predicate: Predicate,
}

/// 一次命令运行的原始结果（与进程启动解耦，方便测试）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOutput {
    pub exit: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub started_ms: u64,
    pub elapsed_ms: u64,
}

impl RunOutput {
    pub fn new(
        exit: i32,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        started_ms: u64,
        elapsed_ms: u64,
    ) -> Self {
        Self {
            exit,
            stdout,
            stderr,
            started_ms,
            elapsed_ms,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WitnessError {
    #[error("witness 不是合法 JSON：{0}")]
    Json(String),
    #[error("不是 in-toto Statement（_type={0}）")]
    WrongStatementType(String),
    #[error("不是测试运行谓词（predicateType={0}）")]
    WrongPredicateType(String),
    #[error("不支持的 witness schema：{0}")]
    UnsupportedSchema(u32),
    #[error("subject 必须恰好一个树，实际 {0} 个")]
    BadSubjectCount(usize),
    #[error("subject 摘要不是 64 位 blake3 十六进制：{0}")]
    BadDigest(String),
    #[error("见证绑定的树 {subject} 与目标树 {actual} 不一致")]
    TreeMismatch { subject: String, actual: String },
    #[error("见证记录的命令退出码是 {0}；测试没通过不能当证据")]
    Failed(i32),
    #[error("见证文件被改动：提交里记的是 {recorded}，实际是 {actual}")]
    FileTampered { recorded: String, actual: String },
}

impl Witness {
    /// 构造一份见证：`tree` 是被见证的树，`base` 是运行前的 HEAD 树。
    pub fn from_run(
        tree: Hash,
        base: Option<Hash>,
        command: Vec<String>,
        output: RunOutput,
    ) -> Self {
        let mut digest = BTreeMap::new();
        digest.insert(DIGEST_ALG.to_string(), tree.to_hex());
        Self {
            statement_type: STATEMENT_TYPE.to_string(),
            subject: vec![Subject {
                name: "tree".to_string(),
                digest,
            }],
            predicate_type: PREDICATE_TYPE.to_string(),
            predicate: Predicate {
                schema: SCHEMA,
                base: base.map(|hash| hash.to_hex()),
                command,
                exit: output.exit,
                stdout_hash: Hash::of(&output.stdout).to_hex(),
                stderr_hash: Hash::of(&output.stderr).to_hex(),
                started_ms: output.started_ms,
                elapsed_ms: output.elapsed_ms,
                tool: format!("trit-vcs/{}", env!("CARGO_PKG_VERSION")),
            },
        }
    }

    /// canonical JSON 字节（字段顺序由结构体固定，digest 用 BTreeMap 排序）。
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("witness 只含可序列化字段")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WitnessError> {
        serde_json::from_slice(bytes).map_err(|e| WitnessError::Json(e.to_string()))
    }

    /// 解析 subject 里绑定的树。
    pub fn subject_tree(&self) -> Result<Hash, WitnessError> {
        if self.subject.len() != 1 {
            return Err(WitnessError::BadSubjectCount(self.subject.len()));
        }
        let digest = self.subject[0]
            .digest
            .get(DIGEST_ALG)
            .ok_or_else(|| WitnessError::BadDigest(format!("缺少 {DIGEST_ALG}")))?;
        Hash::from_hex(digest).ok_or_else(|| WitnessError::BadDigest(digest.clone()))
    }

    pub fn check_tree(&self, tree: Hash) -> Result<(), WitnessError> {
        let subject = self.subject_tree()?;
        if subject != tree {
            return Err(WitnessError::TreeMismatch {
                subject: subject.to_hex(),
                actual: tree.to_hex(),
            });
        }
        Ok(())
    }

    /// 见证记录的是不是一次通过（退出码 0）。
    pub fn check_passed(&self) -> Result<(), WitnessError> {
        if self.predicate.exit != 0 {
            return Err(WitnessError::Failed(self.predicate.exit));
        }
        Ok(())
    }
}

/// 离线校验：形状 + 版本 + 绑定到 `tree`。不判断"通过"，也不验签名。
pub fn verify(bytes: &[u8], tree: Hash) -> Result<Witness, WitnessError> {
    let witness = Witness::decode(bytes)?;
    if witness.statement_type != STATEMENT_TYPE {
        return Err(WitnessError::WrongStatementType(
            witness.statement_type.clone(),
        ));
    }
    if witness.predicate_type != PREDICATE_TYPE {
        return Err(WitnessError::WrongPredicateType(
            witness.predicate_type.clone(),
        ));
    }
    if witness.predicate.schema != SCHEMA {
        return Err(WitnessError::UnsupportedSchema(witness.predicate.schema));
    }
    witness.check_tree(tree)?;
    Ok(witness)
}

/// 见证文件与提交里记录的摘要是否一致。
pub fn check_file(recorded: Hash, bytes: &[u8]) -> Result<(), WitnessError> {
    let actual = Hash::of(bytes);
    if actual != recorded {
        return Err(WitnessError::FileTampered {
            recorded: recorded.to_hex(),
            actual: actual.to_hex(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Hash {
        Hash::of(b"tree-v1")
    }

    fn sample() -> Witness {
        Witness::from_run(
            tree(),
            Some(Hash::of(b"base")),
            vec!["cargo".into(), "test".into()],
            RunOutput::new(0, b"ok".to_vec(), Vec::new(), 1_000, 42),
        )
    }

    #[test]
    fn witness_has_in_toto_shape_and_roundtrips() {
        let witness = sample();
        let bytes = witness.encode();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["_type"], STATEMENT_TYPE);
        assert_eq!(json["predicateType"], PREDICATE_TYPE);
        assert_eq!(json["subject"][0]["name"], "tree");
        assert_eq!(json["subject"][0]["digest"]["blake3"], tree().to_hex());
        assert_eq!(json["predicate"]["stdout_hash"], Hash::of(b"ok").to_hex());
        assert_eq!(Witness::decode(&bytes).unwrap(), witness);
    }

    #[test]
    fn binding_to_the_wrong_tree_is_rejected() {
        let err = verify(&sample().encode(), Hash::of(b"other")).unwrap_err();
        match err {
            WitnessError::TreeMismatch { subject, actual } => {
                assert_eq!(subject, tree().to_hex());
                assert_eq!(actual, Hash::of(b"other").to_hex());
            }
            other => panic!("期望树不一致，得到 {other:?}"),
        }
    }

    #[test]
    fn unsupported_schema_is_rejected() {
        let mut witness = sample();
        witness.predicate.schema = SCHEMA + 1;
        let err = verify(&witness.encode(), tree()).unwrap_err();
        assert_eq!(err, WitnessError::UnsupportedSchema(SCHEMA + 1));
    }

    #[test]
    fn bad_digest_and_subject_count_are_rejected() {
        let mut witness = sample();
        witness.subject[0]
            .digest
            .insert(DIGEST_ALG.to_string(), "xyz".to_string());
        assert!(matches!(
            verify(&witness.encode(), tree()).unwrap_err(),
            WitnessError::BadDigest(_)
        ));
        let mut two = sample();
        two.subject.push(two.subject[0].clone());
        assert_eq!(
            verify(&two.encode(), tree()).unwrap_err(),
            WitnessError::BadSubjectCount(2)
        );
    }

    #[test]
    fn wrong_statement_or_predicate_type_is_rejected() {
        let mut witness = sample();
        witness.statement_type = "https://example.com/other".to_string();
        assert!(matches!(
            verify(&witness.encode(), tree()).unwrap_err(),
            WitnessError::WrongStatementType(_)
        ));
        let mut witness = sample();
        witness.predicate_type = "https://example.com/other".to_string();
        assert!(matches!(
            verify(&witness.encode(), tree()).unwrap_err(),
            WitnessError::WrongPredicateType(_)
        ));
    }

    #[test]
    fn failed_run_is_not_a_passing_witness() {
        let mut witness = sample();
        witness.predicate.exit = 101;
        let parsed = verify(&witness.encode(), tree()).unwrap();
        assert_eq!(
            parsed.check_passed().unwrap_err(),
            WitnessError::Failed(101)
        );
    }

    #[test]
    fn file_digest_catches_edits_that_keep_the_binding() {
        let bytes = sample().encode();
        let recorded = Hash::of(&bytes);
        assert!(check_file(recorded, &bytes).is_ok());
        let mut edited = bytes.clone();
        let at = edited
            .windows(9)
            .position(|window| window == b"trit-vcs/")
            .expect("sample 里应有 tool 字段");
        edited[at] = b'X';
        // 树绑定仍然成立，但文件摘要变了 —— 这一层由提交里记录的摘要来抓。
        assert!(verify(&edited, tree()).is_ok());
        assert!(matches!(
            check_file(recorded, &edited).unwrap_err(),
            WitnessError::FileTampered { .. }
        ));
    }
}
