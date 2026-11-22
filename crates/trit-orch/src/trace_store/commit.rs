//! 日志与 sidecar（水位/锚点）的尽力事务：sidecar 失败时把日志回滚到写入前长度，
//! 避免出现“日志已含新事件、水位/锚点还是旧的”半提交状态。

use std::fs::File;

use crate::{Error, TraceEvent};

use super::TraceEventStore;

impl TraceEventStore {
    pub(super) fn commit_sidecars(
        &self,
        file: &mut File,
        previous_len: u64,
        previous_watermark: Option<u64>,
        last: &TraceEvent,
    ) -> Result<(), Error> {
        let watermark = self.write_watermark(last.seq);
        let anchor = self.anchor_update(last);
        if watermark.is_ok() && anchor.is_ok() {
            return Ok(());
        }

        let primary = watermark
            .err()
            .or_else(|| anchor.err())
            .unwrap_or_else(|| Error::TraceIo {
                path: self.path.display().to_string(),
                message: "sidecar update failed".into(),
            });
        let rollback_log = file.set_len(previous_len).and_then(|_| file.sync_all());
        let rollback_watermark = match previous_watermark {
            Some(seq) => self.write_watermark(seq),
            None => remove_watermark(&self.watermark_path()),
        };
        if rollback_log.is_err() || rollback_watermark.is_err() {
            return Err(Error::TraceIo {
                path: self.path.display().to_string(),
                message: format!(
                    "{primary}; rollback failed (log={:?}, watermark={:?})",
                    rollback_log.err(),
                    rollback_watermark.err()
                ),
            });
        }
        Err(primary)
    }
}

fn remove_watermark(path: &std::path::Path) -> Result<(), Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::TraceIo {
            path: path.display().to_string(),
            message: error.to_string(),
        }),
    }
}
