use serde::Serialize;
use trit_belnap::B4;

use crate::Status;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Face {
    pub face: String,
    pub status: Status,
    pub evidence: String,
    pub detail: String,
}

impl Face {
    #[must_use]
    pub fn new(name: &str, status: Status, evidence: &str, detail: String) -> Self {
        Self {
            face: name.to_string(),
            status,
            evidence: evidence.to_string(),
            detail,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub tool: &'static str,
    pub at: String,
    pub root: String,
    pub faces: Vec<Face>,
}

impl Report {
    /// Belnap 四值聚合：整体不是“平均分”，而是最弱证据。
    #[must_use]
    pub fn verdict(&self) -> B4 {
        self.faces
            .iter()
            .fold(B4::T, |acc, face| acc.and(face.status.as_b4()))
    }

    #[must_use]
    pub fn counts(&self) -> [usize; 4] {
        let mut counts = [0; 4];
        for face in &self.faces {
            let index = match face.status {
                Status::Ok => 0,
                Status::Warn => 1,
                Status::Fail => 2,
                Status::Unavailable => 3,
            };
            counts[index] += 1;
        }
        counts
    }
}
