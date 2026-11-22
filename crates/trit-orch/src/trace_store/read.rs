//! 读取轨迹文件：返回事件与每个事件的**物理行号**（空行跳过，行号仍是文件里真实的位置）。

use std::fs;
use std::io::{BufRead, BufReader};

use crate::Error;
use crate::trace::TraceEvent;
use crate::trace_store::{MAX_TRACE_BYTES, TraceEventStore};

impl TraceEventStore {
    /// 解析文件为 `(事件, 物理行号)`；不校验摘要，供 load 与迁移共用。
    pub(in crate::trace_store) fn read_events(
        &self,
    ) -> Result<(Vec<TraceEvent>, Vec<usize>), Error> {
        let metadata = match fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Vec::new(), Vec::new()));
            }
            Err(error) => return Err(self.io(error)),
        };
        if metadata.len() > MAX_TRACE_BYTES {
            return Err(Error::TraceTooLarge {
                path: self.path.display().to_string(),
                size: metadata.len(),
                max: MAX_TRACE_BYTES,
            });
        }
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Vec::new(), Vec::new()));
            }
            Err(error) => return Err(self.io(error)),
        };
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        let mut lines = Vec::new();
        for (index, line) in reader.lines().enumerate() {
            let line = line.map_err(|error| Error::TraceParse {
                path: self.path.display().to_string(),
                line: index + 1,
                message: error.to_string(),
            })?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let payload = self.decode_stored_line(index, line)?;
            let event: TraceEvent =
                serde_json::from_str(&payload).map_err(|error| Error::TraceParse {
                    path: self.path.display().to_string(),
                    line: index + 1,
                    message: error.to_string(),
                })?;
            if event.schema != TraceEvent::CURRENT_SCHEMA {
                return Err(Error::TraceSchema {
                    found: event.schema,
                    expected: TraceEvent::CURRENT_SCHEMA,
                });
            }
            events.push(event);
            lines.push(index + 1);
        }
        Ok((events, lines))
    }

    /// 只要事件（丢掉行号），供迁移与重封使用。
    pub(in crate::trace_store) fn read_plain_events(&self) -> Result<Vec<TraceEvent>, Error> {
        Ok(self.read_events()?.0)
    }
}
