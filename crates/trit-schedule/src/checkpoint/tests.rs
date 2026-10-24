use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use crate::{Progress, StepContext};

    struct Counter {
        done: usize,
        target: usize,
    }

    impl ResumableTask for Counter {
        fn step(&mut self, _ctx: &StepContext) -> Progress {
            self.done += 1;
            if self.done >= self.target {
                Progress::Done
            } else {
                Progress::Yield
            }
        }
    }

    impl CheckpointTask for Counter {
        fn save_state(&self) -> Result<Vec<u8>, CheckpointError> {
            Ok(self.done.to_le_bytes().to_vec())
        }
    }

    struct CounterRestorer {
        target: usize,
    }

    impl TaskRestorer for CounterRestorer {
        fn restore(
            &self,
            task_id: &str,
            state: &[u8],
        ) -> Result<Box<dyn CheckpointTask>, CheckpointError> {
            if task_id != "counter" {
                return Err(CheckpointError::UnknownTask(task_id.to_string()));
            }
            let bytes: [u8; 8] = state.try_into().map_err(|_| {
                CheckpointError::InvalidState(format!("期望 8 字节，得到 {}", state.len()))
            })?;
            Ok(Box::new(Counter {
                done: usize::from_le_bytes(bytes),
                target: self.target,
            }))
        }
    }

    #[test]
    fn checkpoint_round_trips_progress() {
        let ctx = StepContext {
            cancel: crate::CancelToken::new(),
        };
        let mut task = Counter {
            done: 0,
            target: 10,
        };
        for _ in 0..3 {
            assert_eq!(task.step(&ctx), Progress::Yield);
        }
        let checkpoint = save_checkpoint("counter", &task).unwrap();
        assert_eq!(checkpoint.version, CHECKPOINT_VERSION);
        assert_eq!(checkpoint.state, 3u64.to_le_bytes());
        let mut restored =
            restore_checkpoint(&CounterRestorer { target: 10 }, &checkpoint).unwrap();
        for _ in 3..9 {
            assert_eq!(restored.step(&ctx), Progress::Yield);
        }
        assert_eq!(restored.step(&ctx), Progress::Done);
    }

    #[test]
    fn store_overwrites_and_removes() {
        let mut store = CheckpointStore::new();
        assert!(store.is_empty());
        store.insert(Checkpoint::new("a", vec![1])).unwrap();
        store.insert(Checkpoint::new("a", vec![2])).unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(store.get("a").unwrap().state, vec![2]);
        assert!(store.remove("a").is_some());
        assert!(store.get("a").is_none());
    }

    #[test]
    fn rejects_unknown_version_and_unknown_task() {
        let mut store = CheckpointStore::new();
        let mut bad = Checkpoint::new("a", vec![]);
        bad.version = CHECKPOINT_VERSION + 1;
        assert!(matches!(
            store.insert(bad),
            Err(CheckpointError::UnsupportedVersion { .. })
        ));
        assert!(store.is_empty());
        assert!(matches!(
            store.restore(&CounterRestorer { target: 1 }, "missing"),
            Err(CheckpointError::UnknownTask(_))
        ));
        let mut bad_restore = Checkpoint::new("counter", 0u64.to_le_bytes().to_vec());
        bad_restore.version = CHECKPOINT_VERSION + 1;
        assert!(matches!(
            restore_checkpoint(&CounterRestorer { target: 1 }, &bad_restore),
            Err(CheckpointError::UnsupportedVersion { .. })
        ));
    }
}

#[cfg(test)]
mod checkpointed_task_tests {
    use super::*;
    use crate::{CancelToken, ResumableSpec, RuntimeConfig, run_resumable};

    struct Counter {
        done: usize,
        target: usize,
    }

    impl ResumableTask for Counter {
        fn step(&mut self, _ctx: &StepContext) -> Progress {
            self.done += 1;
            if self.done >= self.target {
                Progress::Done
            } else {
                Progress::Yield
            }
        }
    }

    impl CheckpointTask for Counter {
        fn save_state(&self) -> Result<Vec<u8>, CheckpointError> {
            Ok(self.done.to_le_bytes().to_vec())
        }
    }

    struct Restorer;

    impl TaskRestorer for Restorer {
        fn restore(
            &self,
            task_id: &str,
            state: &[u8],
        ) -> Result<Box<dyn CheckpointTask>, CheckpointError> {
            if task_id != "counter" {
                return Err(CheckpointError::UnknownTask(task_id.to_string()));
            }
            let bytes: [u8; 8] = state
                .try_into()
                .map_err(|_| CheckpointError::InvalidState("bad length".to_string()))?;
            Ok(Box::new(Counter {
                done: usize::from_le_bytes(bytes),
                target: 4,
            }))
        }
    }

    #[test]
    fn runtime_persists_each_step_and_can_restore() {
        let store = Arc::new(Mutex::new(CheckpointStore::new()));
        let wrapped = CheckpointedTask::new(
            "counter",
            Counter { done: 0, target: 4 },
            Arc::clone(&store),
        );
        let report = run_resumable(
            vec![ResumableSpec::new("counter", vec![], wrapped)],
            RuntimeConfig::new(1),
            CancelToken::new(),
        )
        .unwrap();
        assert_eq!(report.completed, vec!["counter"]);
        let stored = {
            let store = store.lock().unwrap();
            store.get("counter").cloned().unwrap()
        };
        assert_eq!(stored.state, 4u64.to_le_bytes());
        assert_eq!(stored.version, CHECKPOINT_VERSION);
        let task = {
            let store = store.lock().unwrap();
            store.restore(&Restorer, "counter").unwrap()
        };
        assert_eq!(task.save_state().unwrap(), 4u64.to_le_bytes());
    }
}

#[cfg(test)]
mod file_store_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "trit-checkpoints-{}-{}-{}",
            std::process::id(),
            nanos(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn cleanup(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn file_store_round_trips_versions_and_escapes_ids() {
        let root = temp_root();
        let store = FileCheckpointStore::new(&root);
        let id = "../任务/a";
        let first = store.save(&Checkpoint::new(id, vec![1])).unwrap();
        let second = store.save(&Checkpoint::new(id, vec![2])).unwrap();
        assert_eq!((first, second), (1, 2));
        assert_eq!(store.load(id).unwrap().unwrap().state, vec![2]);
        assert_eq!(store.len().unwrap(), 1);
        assert!(!store.task_dir(id).to_string_lossy().contains(".."));
        assert!(store.task_dir(id).starts_with(store.version_dir()));
        cleanup(&root);
    }

    #[test]
    fn file_store_prunes_and_removes() {
        let root = temp_root();
        let store = FileCheckpointStore::new(&root);
        for state in 0..3 {
            store.save(&Checkpoint::new("a", vec![state])).unwrap();
        }
        assert_eq!(store.prune("a", 1).unwrap(), 2);
        assert_eq!(store.load("a").unwrap().unwrap().state, vec![2]);
        assert!(store.remove("a").unwrap());
        assert!(store.load("a").unwrap().is_none());
        assert!(!store.remove("a").unwrap());
        cleanup(&root);
    }

    #[test]
    fn file_store_rejects_bad_version_without_writing() {
        let root = temp_root();
        let store = FileCheckpointStore::new(&root);
        let mut bad = Checkpoint::new("a", vec![]);
        bad.version = CHECKPOINT_VERSION + 1;
        assert!(matches!(
            store.save(&bad),
            Err(CheckpointError::UnsupportedVersion { .. })
        ));
        assert!(store.is_empty().unwrap());
        cleanup(&root);
    }
}

#[cfg(test)]
mod index_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "trit-index-{}-{}-{}",
            std::process::id(),
            nanos(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn index_lists_latest_generations_sorted() {
        let root = temp_root();
        let store = FileCheckpointStore::new(&root);
        assert!(store.index().unwrap().is_empty());
        store.save(&Checkpoint::new("b", vec![1, 2, 3])).unwrap();
        store.save(&Checkpoint::new("a/任务", vec![4])).unwrap();
        store.save(&Checkpoint::new("a/任务", vec![5, 6])).unwrap();
        let index = store.index().unwrap();
        assert_eq!(index.version, CHECKPOINT_VERSION);
        assert_eq!(index.len(), 2);
        assert_eq!(
            index
                .entries
                .iter()
                .map(|entry| entry.task_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a/任务", "b"]
        );
        let entry = index.get("a/任务").unwrap();
        assert_eq!(entry.generation, 2);
        assert!(entry.state_bytes > 10, "{entry:?}");
        assert_eq!(store.task_ids().unwrap(), vec!["a/任务", "b"]);
        assert!(store.index_json().unwrap().contains("a/任务"));
        assert!(store.index().unwrap().get("missing").is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn index_ignores_the_lock_directory_while_held() {
        // 复审 M04：持锁时 `.lock/` 曾被当成任务目录，`hex_decode(".lock")` 让 index 报错。
        let root = temp_root();
        let store = FileCheckpointStore::new(&root);
        store.save(&Checkpoint::new("t1", vec![9])).unwrap();
        let lock = store.lock().unwrap();
        let index = store.index().unwrap();
        assert_eq!(index.len(), 1);
        assert_eq!(index.get("t1").unwrap().generation, 1);
        assert_eq!(store.len().unwrap(), 1);
        assert_eq!(store.task_ids().unwrap(), vec!["t1"]);
        drop(lock);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_task_id_is_rejected_by_every_entry() {
        // 空 id 的十六进制是空串 ⇒ `task_dir("")` 就是版本目录，`remove("")` 会删掉全部任务。
        let root = temp_root();
        let store = FileCheckpointStore::new(&root);
        store.save(&Checkpoint::new("keep", vec![1])).unwrap();
        assert!(matches!(
            store.save(&Checkpoint::new("", vec![2])),
            Err(CheckpointError::InvalidState(_))
        ));
        assert!(matches!(
            store.load(""),
            Err(CheckpointError::InvalidState(_))
        ));
        assert!(matches!(
            store.remove(""),
            Err(CheckpointError::InvalidState(_))
        ));
        assert!(matches!(
            store.prune("", 1),
            Err(CheckpointError::InvalidState(_))
        ));
        assert_eq!(store.task_ids().unwrap(), vec!["keep"], "既有任务必须还在");
        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod lock_tests {
    use super::super::lock::{LockOwner, local_owner, write_owner};
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "trit-lock-{}-{}-{}",
            std::process::id(),
            nanos(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn cross_process_lock_excludes_and_releases() {
        let root = temp_root();
        let first = FileCheckpointStore::new(&root);
        let second = FileCheckpointStore::new(&root);
        let lock = first.try_lock().unwrap().unwrap();
        assert_eq!(lock.path(), first.lock_path());
        assert!(second.try_lock().unwrap().is_none());
        assert!(matches!(
            first.lock_timeout(Duration::from_millis(20)),
            Err(CheckpointError::LockBusy)
        ));
        drop(lock);
        drop(second.try_lock().unwrap().expect("释放后可获取"));
        let _ = fs::remove_dir_all(&root);
    }

    /// 一个确定已经退出的进程 pid。
    fn dead_pid() -> u32 {
        #[cfg(windows)]
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "exit", "0"])
            .spawn()
            .expect("应能启动 cmd");
        #[cfg(unix)]
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("应能启动 true");
        let pid = child.id();
        child.wait().expect("等待子进程退出");
        pid
    }

    #[test]
    fn stale_lock_from_a_dead_process_is_recovered() {
        // 复审 M05：崩溃/kill 之后 `.lock/` 永久残留，后续进程只能一直超时。
        let root = temp_root();
        let store = FileCheckpointStore::new(&root);
        let path = store.lock_path();
        fs::create_dir_all(&path).unwrap();
        let owner = LockOwner {
            pid: dead_pid(),
            host: local_owner().host,
            acquired_unix_ms: 0,
        };
        write_owner(&path, &owner).unwrap();
        assert!(store.try_lock().unwrap().is_none(), "残留目录仍占位");
        assert!(store.break_stale_lock().unwrap(), "死进程的锁应判为陈旧");
        let lock = store.try_lock().unwrap().expect("清理后应拿得到锁");
        assert_eq!(lock.owner_pid(), std::process::id());
        drop(lock);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fresh_ownerless_lock_is_not_stolen() {
        // `create_dir` 成功、`owner.json` 未落盘的窗口里，绝不能把新锁当残骸删掉。
        let root = temp_root();
        let store = FileCheckpointStore::new(&root);
        fs::create_dir_all(store.lock_path()).unwrap();
        assert!(!store.break_stale_lock().unwrap(), "刚建的无主锁不能回收");
        assert!(store.try_lock().unwrap().is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn drop_does_not_delete_a_taken_over_lock() {
        // 锁目录被别人重建后，旧持有者的 Drop 不能把新锁删掉。
        let root = temp_root();
        let store = FileCheckpointStore::new(&root);
        let lock = store.try_lock().unwrap().unwrap();
        let other = LockOwner {
            pid: lock.owner_pid().wrapping_add(1),
            host: local_owner().host,
            acquired_unix_ms: 1,
        };
        fs::remove_file(store.lock_path().join(super::super::lock::OWNER_FILE)).unwrap();
        write_owner(&store.lock_path(), &other).unwrap();
        drop(lock);
        assert!(store.lock_path().is_dir(), "别人的锁不能被 Drop 删掉");
        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod restore_tests {
    use super::*;
    use crate::{CancelToken, RuntimeConfig, run_resumable};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    struct Counter {
        done: usize,
        target: usize,
    }

    impl ResumableTask for Counter {
        fn step(&mut self, _ctx: &StepContext) -> Progress {
            self.done += 1;
            if self.done >= self.target {
                Progress::Done
            } else {
                Progress::Yield
            }
        }
    }

    impl CheckpointTask for Counter {
        fn save_state(&self) -> Result<Vec<u8>, CheckpointError> {
            Ok(self.done.to_le_bytes().to_vec())
        }
    }

    struct Restorer;

    impl TaskRestorer for Restorer {
        fn restore(
            &self,
            task_id: &str,
            state: &[u8],
        ) -> Result<Box<dyn CheckpointTask>, CheckpointError> {
            if task_id != "counter" {
                return Err(CheckpointError::UnknownTask(task_id.to_string()));
            }
            let bytes: [u8; 8] = state
                .try_into()
                .map_err(|_| CheckpointError::InvalidState("bad length".to_string()))?;
            Ok(Box::new(Counter {
                done: usize::from_le_bytes(bytes),
                target: 10,
            }))
        }
    }

    struct Factory;

    impl TaskFactory for Factory {
        fn create(&self, task_id: &str) -> Result<Box<dyn CheckpointTask>, CheckpointError> {
            if task_id != "counter" {
                return Err(CheckpointError::UnknownTask(task_id.to_string()));
            }
            Ok(Box::new(Counter {
                done: 0,
                target: 10,
            }))
        }
    }

    fn defs() -> Vec<ResumableSpecDef> {
        vec![ResumableSpecDef::new("counter", vec![])]
    }

    fn run(specs: Vec<ResumableSpec>) -> crate::ResumableReport {
        run_resumable(specs, RuntimeConfig::new(1), CancelToken::new()).unwrap()
    }

    #[test]
    fn memory_source_resumes_mid_task() {
        let mut source = CheckpointStore::new();
        source
            .insert(
                save_checkpoint(
                    "counter",
                    &Counter {
                        done: 3,
                        target: 10,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        let specs = restore_specs(defs(), &source, &Restorer, &Factory).unwrap();
        let report = run(specs);
        assert_eq!(report.completed, vec!["counter"]);
        assert_eq!(report.steps, 7, "从第 3 步恢复后只应再走 7 步");
    }

    #[test]
    fn file_source_resumes_mid_task() {
        let root = std::env::temp_dir().join(format!(
            "trit-restore-{}-{}-{}",
            std::process::id(),
            nanos(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let store = FileCheckpointStore::new(&root);
        store
            .save(
                &save_checkpoint(
                    "counter",
                    &Counter {
                        done: 3,
                        target: 10,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        let specs = restore_specs(defs(), &store, &Restorer, &Factory).unwrap();
        let report = run(specs);
        assert_eq!(report.completed, vec!["counter"]);
        assert_eq!(report.steps, 7);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_checkpoint_uses_factory() {
        let source = CheckpointStore::new();
        let specs = restore_specs(defs(), &source, &Restorer, &Factory).unwrap();
        let report = run(specs);
        assert_eq!(report.completed, vec!["counter"]);
        assert_eq!(report.steps, 10);
    }
}
