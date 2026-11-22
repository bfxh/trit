//! 轨迹水位：最后一次成功 append 的 seq，用于检测尾部截断。
//!
//! 水位写用 tmp + rename；写一半崩溃不会留下半截水位，最坏只是落后（少报，
//! 不会误报）。

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::Error;

use super::TraceEventStore;

impl TraceEventStore {
    pub(super) fn write_watermark(&self, seq: u64) -> Result<(), Error> {
        let path = self.watermark_path();
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let serial = SEQ.fetch_add(1, Ordering::Relaxed);
        let mut temporary = path.as_os_str().to_os_string();
        temporary.push(format!(".{}.{}.tmp", std::process::id(), serial));
        let temporary = PathBuf::from(temporary);
        fs::write(&temporary, format!("{seq}\n")).map_err(|error| Error::TraceIo {
            path: temporary.display().to_string(),
            message: error.to_string(),
        })?;
        match fs::rename(&temporary, &path) {
            Ok(()) => Ok(()),
            Err(_) => {
                let _ = fs::remove_file(&path);
                fs::rename(&temporary, &path).map_err(|error| Error::TraceIo {
                    path: path.display().to_string(),
                    message: error.to_string(),
                })
            }
        }
    }

    pub(super) fn read_watermark(&self) -> Result<Option<u64>, Error> {
        let path = self.watermark_path();
        match fs::read_to_string(&path) {
            Ok(text) => text
                .trim()
                .parse::<u64>()
                .map(Some)
                .map_err(|_| Error::TraceWatermark {
                    path: path.display().to_string(),
                    message: format!("bad watermark {text:?}"),
                }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(Error::TraceIo {
                path: path.display().to_string(),
                message: error.to_string(),
            }),
        }
    }
}
