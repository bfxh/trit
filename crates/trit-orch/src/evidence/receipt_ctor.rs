use super::{Exit, Receipt, ReceiptProvenance};

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
            plan: None,
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
            plan: None,
        }
    }
}
