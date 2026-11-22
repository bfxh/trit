use crate::{CheckReport, CheckResult, Ternary, TokenUsage};

use super::{EvidenceSpec, Exit, Receipt, ReceiptProvenance};

impl Receipt {
    /// 宿主自己测量出的回执。这是默认完成闸唯一接受的构造路径。
    pub fn host_measured(
        epoch: u64,
        exit: Exit,
        assertions_passed: u32,
        assertions_failed: u32,
    ) -> Self {
        Self {
            epoch,
            exit,
            assertions_passed,
            assertions_failed,
            provenance: ReceiptProvenance::HostMeasured,
            nonce: String::new(),
            command: Vec::new(),
            authorization: None,
            freshness: None,
            usage: None,
            test_digest: None,
        }
    }

    /// 显式构造一条自报回执。默认完成闸会拒绝它；只用于外部已有独立信任锚的场景。
    pub fn self_reported(
        epoch: u64,
        exit: Exit,
        assertions_passed: u32,
        assertions_failed: u32,
    ) -> Self {
        Self {
            epoch,
            exit,
            assertions_passed,
            assertions_failed,
            provenance: ReceiptProvenance::SelfReported,
            nonce: String::new(),
            command: Vec::new(),
            authorization: None,
            freshness: None,
            usage: None,
            test_digest: None,
        }
    }

    /// 绑定宿主挑战与执行命令，供审计与跨轮重放检查。
    pub fn with_audit(mut self, nonce: impl Into<String>, command: Vec<String>) -> Self {
        self.nonce = nonce.into();
        self.command = command;
        self
    }

    /// 绑定宿主对 observation 的授权/新鲜度/用量判定。
    ///
    /// 默认完成闸要求三者都存在且与 snapshot 一致：模型不能再靠
    /// `authorization: True` / `freshness: True` / 低报 usage 解锁完成。
    pub fn with_observation(
        mut self,
        authorization: Ternary,
        freshness: Ternary,
        usage: TokenUsage,
    ) -> Self {
        self.authorization = Some(authorization);
        self.freshness = Some(freshness);
        self.usage = Some(usage);
        self
    }

    /// 绑定测试工件摘要（宿主对 `test_files` 计算的内容哈希）。
    ///
    /// 它固定“这次跑的是哪份测试代码”，不证明测试有语义价值；后者仍是测试
    /// 质量门的职责。
    pub fn with_test_digest(mut self, digest: Option<String>) -> Self {
        self.test_digest = digest;
        self
    }

    pub fn is_host_measured(&self) -> bool {
        self.provenance == ReceiptProvenance::HostMeasured
    }

    /// 把回执折算成验收报告。只有「有失败断言」才算 `Refuted`；
    /// 崩溃、超时、零断言、覆盖不足一律 `Inconclusive`，因为都不能证伪命题。
    ///
    /// 零断言在这里和 `CheckReport::verdict` 各有一道守卫（双保险）：删掉任一道，
    /// 零检查仍无法变成 Support。
    pub fn report(&self, spec: &EvidenceSpec) -> CheckReport {
        let checks = self
            .assertions_passed
            .saturating_add(self.assertions_failed);
        let result = if self.assertions_failed > 0 {
            CheckResult::Refuted
        } else {
            match self.exit {
                Exit::Completed { code: 0 } if self.assertions_passed >= spec.min_assertions => {
                    CheckResult::Passed
                }
                _ => CheckResult::Inconclusive,
            }
        };
        CheckReport {
            epoch: self.epoch,
            checks,
            result,
        }
    }
}
