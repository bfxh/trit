//! 轨迹存储的统一门面：JSONL（`TraceEventStore`）与 trit-db（`TritDbTraceStore`）共用。

use crate::{Error, TraceEvent, TraceEventStore, TraceLog};

/// 轨迹存储的最小读写面；CLI 与调用方通过 `Box<dyn TraceStore>` 选择后端。
pub trait TraceBackend {
    fn append(&self, event: &TraceEvent) -> Result<(), Error>;
    fn load(&self) -> Result<Vec<TraceEvent>, Error>;
    fn load_log(&self, trace_id: &str) -> Result<TraceLog, Error>;
}

impl TraceBackend for TraceEventStore {
    fn append(&self, event: &TraceEvent) -> Result<(), Error> {
        TraceEventStore::append(self, event)
    }

    fn load(&self) -> Result<Vec<TraceEvent>, Error> {
        TraceEventStore::load(self)
    }

    fn load_log(&self, trace_id: &str) -> Result<TraceLog, Error> {
        TraceEventStore::load_log(self, trace_id)
    }
}

#[cfg(feature = "tritdb")]
impl TraceBackend for crate::tritdb_backend::TritDbTraceStore {
    fn append(&self, event: &TraceEvent) -> Result<(), Error> {
        crate::tritdb_backend::TritDbTraceStore::append(self, event)
    }

    fn load(&self) -> Result<Vec<TraceEvent>, Error> {
        crate::tritdb_backend::TritDbTraceStore::load(self)
    }

    fn load_log(&self, trace_id: &str) -> Result<TraceLog, Error> {
        crate::tritdb_backend::TritDbTraceStore::load_log(self, trace_id)
    }
}
