use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::lock::{
    OWNERLESS_STALE_AFTER, dir_age, local_owner, process_alive, read_owner, write_owner,
};
use super::{
    CHECKPOINT_VERSION, Checkpoint, CheckpointEntry, CheckpointError, CheckpointIndex,
    CheckpointLock, CheckpointSource,
};

/// 文件检查点存储；布局 `<root>/v<CHECKPOINT_VERSION>/<hex(task_id)>/<generation>.json`，临时文件不以 `.json` 结尾。
#[derive(Debug, Clone)]
pub struct FileCheckpointStore {
    root: PathBuf,
}

impl FileCheckpointStore {
    /// 用根目录构造；目录在首次保存时创建。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 当前版本目录。
    pub fn version_dir(&self) -> PathBuf {
        self.root.join(format!("v{CHECKPOINT_VERSION}"))
    }

    /// 任务目录（任务 id 十六进制转义，避免路径分隔符/非法字符）。
    pub fn task_dir(&self, task_id: &str) -> PathBuf {
        self.version_dir().join(hex_encode(task_id.as_bytes()))
    }

    /// 保存检查点并返回生成的代号。不自带锁：并发写同一任务须由调用方用
    /// [`Self::lock`] 串行化，否则两写者算出同一代号（rename 覆盖）。
    pub fn save(&self, checkpoint: &Checkpoint) -> Result<u64, CheckpointError> {
        checkpoint.validate()?;
        checked_task_id(&checkpoint.task_id)?;
        let dir = self.task_dir(&checkpoint.task_id);
        fs::create_dir_all(&dir).map_err(io_error)?;
        let latest = best_candidate(&dir)?.map(|(generation, _)| generation);
        let generation = match latest {
            None => 1u64,
            Some(latest) => latest.checked_add(1).ok_or_else(|| {
                CheckpointError::InvalidState(format!(
                    "检查点代号已达上限 u64::MAX（{latest}），无法再分配新代号"
                ))
            })?,
        };
        let bytes = serde_json::to_vec(checkpoint)
            .map_err(|error| CheckpointError::InvalidState(error.to_string()))?;
        let temp = dir.join(format!(".tmp-{}-{}", std::process::id(), nanos()));
        fs::write(&temp, &bytes).map_err(io_error)?;
        let target = dir.join(format!("{generation:020}.json"));
        if let Err(error) = fs::rename(&temp, &target) {
            let _ = fs::remove_file(&temp);
            return Err(io_error(error));
        }
        Ok(generation)
    }

    /// 加载最新检查点；候选按代号数值从大到小尝试，最新代损坏回退次新代，全部不可用返回最后错误。
    pub fn load(&self, task_id: &str) -> Result<Option<Checkpoint>, CheckpointError> {
        checked_task_id(task_id)?;
        let dir = self.task_dir(task_id);
        if !dir.exists() {
            return Ok(None);
        }
        let mut candidates = checkpoint_candidates(&dir)?;
        candidates.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.file_name().cmp(&a.1.file_name()))
        });
        let mut last_error: Option<CheckpointError> = None;
        for (_, path) in candidates {
            match load_checkpoint_file(&path, task_id) {
                Ok(checkpoint) => return Ok(Some(checkpoint)),
                Err(error) => last_error = Some(error),
            }
        }
        last_error.map_or(Ok(None), Err)
    }

    /// 删除某任务的全部检查点；不存在返回 `false`。
    pub fn remove(&self, task_id: &str) -> Result<bool, CheckpointError> {
        checked_task_id(task_id)?;
        let dir = self.task_dir(task_id);
        if !dir.exists() {
            return Ok(false);
        }
        fs::remove_dir_all(&dir).map_err(io_error)?;
        Ok(true)
    }

    /// 只保留某任务最新 `keep` 代，返回删除的文件数。
    pub fn prune(&self, task_id: &str, keep: usize) -> Result<usize, CheckpointError> {
        checked_task_id(task_id)?;
        let dir = self.task_dir(task_id);
        if !dir.exists() {
            return Ok(0);
        }
        let mut files = checkpoint_files(&dir)?;
        files.sort();
        if files.len() <= keep {
            return Ok(0);
        }
        let remove = files.len() - keep;
        for path in files.into_iter().take(remove) {
            fs::remove_file(path).map_err(io_error)?;
        }
        Ok(remove)
    }

    /// 锁目录路径。
    pub fn lock_path(&self) -> PathBuf {
        self.version_dir().join(".lock")
    }

    /// 尝试获取跨进程锁；已被占用时返回 `None`。
    pub fn try_lock(&self) -> Result<Option<CheckpointLock>, CheckpointError> {
        fs::create_dir_all(self.version_dir()).map_err(io_error)?;
        let path = self.lock_path();
        match fs::create_dir(&path) {
            Ok(()) => {
                let owner = local_owner();
                // create_new 保证记录唯一：停顿过久被回收后会失败，而不是覆盖新持有者记录。
                match write_owner(&path, &owner) {
                    Ok(()) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::NotFound
                        ) =>
                    {
                        return Ok(None);
                    }
                    Err(error) => {
                        let _ = fs::remove_dir_all(&path);
                        return Err(io_error(error));
                    }
                }
                // 写完记录后目录若已被隔离走，owner 就不在原位——那不是我们的锁。
                if read_owner(&path).as_ref() != Some(&owner) {
                    return Ok(None);
                }
                Ok(Some(CheckpointLock { path, owner }))
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
            Err(error) => Err(io_error(error)),
        }
    }

    /// 带超时获取跨进程锁；每秒探测残留锁，确定已死/超龄无主才清理重试，不永久卡住。
    pub fn lock_timeout(&self, timeout: Duration) -> Result<CheckpointLock, CheckpointError> {
        let start = Instant::now();
        let mut probed: Option<Instant> = None;
        loop {
            if let Some(lock) = self.try_lock()? {
                return Ok(lock);
            }
            if probed.is_none_or(|last| last.elapsed() >= Duration::from_secs(1)) {
                probed = Some(Instant::now());
                if self.break_stale_lock()? {
                    continue;
                }
            }
            if start.elapsed() >= timeout {
                return Err(CheckpointError::LockBusy);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 获取跨进程锁（最多等 5 秒）。
    pub fn lock(&self) -> Result<CheckpointLock, CheckpointError> {
        self.lock_timeout(Duration::from_secs(5))
    }

    /// 清理确定已死持有者的锁目录；先原子改名隔离、再复核 owner，绝不删活锁。
    /// 跨主机记录一律视为占有；无主锁须目录年龄超过 `OWNERLESS_STALE_AFTER` 才清理。
    pub fn break_stale_lock(&self) -> Result<bool, CheckpointError> {
        let path = self.lock_path();
        if !path.is_dir() {
            return Ok(false);
        }
        let owner = read_owner(&path);
        let stale = match &owner {
            Some(owner) => {
                owner.host == local_owner().host && matches!(process_alive(owner.pid), Some(false))
            }
            None => dir_age(&path).is_some_and(|age| age >= OWNERLESS_STALE_AFTER),
        };
        if !stale {
            return Ok(false);
        }
        let quarantine =
            self.version_dir()
                .join(format!(".lock.reap-{}-{}", std::process::id(), nanos()));
        if fs::rename(&path, &quarantine).is_err() {
            return Ok(false);
        }
        if read_owner(&quarantine) != owner {
            // 记录变了就还回去；绝不覆盖已存在目录，还不了就留隔离目录。
            if !path.exists() {
                let _ = fs::rename(&quarantine, &path);
            }
            return Ok(false);
        }
        fs::remove_dir_all(&quarantine).map_err(io_error)?;
        Ok(true)
    }

    /// 扫描版本目录，生成按 task id 排序的多任务索引。
    pub fn index(&self) -> Result<CheckpointIndex, CheckpointError> {
        let dir = self.version_dir();
        let mut entries = Vec::new();
        if dir.exists() {
            for entry in fs::read_dir(&dir).map_err(io_error)? {
                let path = entry.map_err(io_error)?.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                // M04：`.lock`/`.lock.reap-*` 等噪声目录由 hex_decode 判据跳过。
                let Ok(task_id) = hex_decode(name) else {
                    continue;
                };
                let Some((generation, file)) = best_candidate(&path)? else {
                    continue;
                };
                let state_bytes = fs::metadata(&file).map_err(io_error)?.len();
                entries.push(CheckpointEntry {
                    task_id,
                    generation,
                    state_bytes,
                });
            }
        }
        entries.sort_by(|left, right| left.task_id.cmp(&right.task_id));
        Ok(CheckpointIndex {
            version: CHECKPOINT_VERSION,
            entries,
        })
    }

    /// 任务 id 列表（按索引顺序）。
    pub fn task_ids(&self) -> Result<Vec<String>, CheckpointError> {
        Ok(self
            .index()?
            .entries
            .into_iter()
            .map(|entry| entry.task_id)
            .collect())
    }

    /// 索引的 JSON 表示（便于外部工具消费）。
    pub fn index_json(&self) -> Result<String, CheckpointError> {
        serde_json::to_string_pretty(&self.index()?)
            .map_err(|error| CheckpointError::InvalidState(error.to_string()))
    }

    /// 已保存的任务数。
    pub fn len(&self) -> Result<usize, CheckpointError> {
        let dir = self.version_dir();
        if !dir.exists() {
            return Ok(0);
        }
        let mut count = 0;
        for entry in fs::read_dir(&dir).map_err(io_error)? {
            let path = entry.map_err(io_error)?.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            // 与 `index()` 用同一判据，避免两个公开 API 对「有几个任务」给出不同答案。
            if path.is_dir() && hex_decode(name).is_ok() {
                count += 1;
            }
        }
        Ok(count)
    }

    /// 是否没有任何任务。
    pub fn is_empty(&self) -> Result<bool, CheckpointError> {
        Ok(self.len()? == 0)
    }
}

impl CheckpointSource for FileCheckpointStore {
    fn checkpoint(&self, task_id: &str) -> Result<Option<Checkpoint>, CheckpointError> {
        self.load(task_id)
    }
}

fn io_error(error: std::io::Error) -> CheckpointError {
    CheckpointError::Io(error.to_string())
}

/// 空 id 的十六进制为空串，任务目录会落在版本目录本身；公开入口一律拒绝空 id。
fn checked_task_id(task_id: &str) -> Result<(), CheckpointError> {
    if task_id.is_empty() {
        return Err(CheckpointError::InvalidState(
            "任务 id 不能为空".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// 合法任务目录名：偶数长度全十六进制；`.lock` 等保留项含 `.`，无需保留名清单。
fn hex_decode(text: &str) -> Result<String, CheckpointError> {
    if !text.len().is_multiple_of(2) {
        return Err(CheckpointError::InvalidState(format!(
            "任务目录名不是十六进制: {text}"
        )));
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    for pair in text.as_bytes().chunks(2) {
        let pair = std::str::from_utf8(pair)
            .map_err(|error| CheckpointError::InvalidState(error.to_string()))?;
        let byte = u8::from_str_radix(pair, 16)
            .map_err(|_| CheckpointError::InvalidState(format!("非法十六进制目录: {text}")))?;
        bytes.push(byte);
    }
    String::from_utf8(bytes).map_err(|error| CheckpointError::InvalidState(error.to_string()))
}

fn checkpoint_files(dir: &Path) -> Result<Vec<PathBuf>, CheckpointError> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(io_error)? {
        let path = entry.map_err(io_error)?.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            files.push(path);
        }
    }
    Ok(files)
}

/// R40-S8-2：候选须 `.json` 且 stem 可解析为 u64；非数字 stem/无扩展名/`.json.bak`/`.tmp-*` 不计入。
fn checkpoint_candidates(dir: &Path) -> Result<Vec<(u64, PathBuf)>, CheckpointError> {
    Ok(checkpoint_files(dir)?
        .into_iter()
        .filter_map(|path| {
            let stem = path.file_stem()?.to_str()?;
            stem.parse::<u64>()
                .ok()
                .map(|generation| (generation, path))
        })
        .collect())
}

fn load_checkpoint_file(path: &Path, task_id: &str) -> Result<Checkpoint, CheckpointError> {
    let bytes = fs::read(path).map_err(io_error)?;
    let checkpoint: Checkpoint = serde_json::from_slice(&bytes)
        .map_err(|error| CheckpointError::InvalidState(error.to_string()))?;
    checkpoint.validate()?;
    if checkpoint.task_id != task_id {
        return Err(CheckpointError::InvalidState(format!(
            "任务 id 不匹配: 期望 {task_id}，实际 {}",
            checkpoint.task_id
        )));
    }
    Ok(checkpoint)
}

fn best_candidate(dir: &Path) -> Result<Option<(u64, PathBuf)>, CheckpointError> {
    if !dir.exists() {
        return Ok(None);
    }
    let mut candidates = checkpoint_candidates(dir)?;
    candidates.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.file_name().cmp(&b.1.file_name()))
    });
    Ok(candidates.pop())
}
