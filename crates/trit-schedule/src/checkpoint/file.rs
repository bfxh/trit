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

/// 版本化目录的文件检查点存储。
///
/// 布局：`<root>/v<CHECKPOINT_VERSION>/<hex(task_id)>/<generation>.json`。
/// 每次保存写同目录临时文件，再 `rename` 到**新**代文件；不覆盖旧文件，加载取最大代。
/// 临时文件不以 `.json` 结尾，崩溃残留不会参与加载。
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

    /// 保存检查点并返回生成的代号。
    ///
    /// 不自带锁：并发写**同一任务**时必须由调用方用 [`Self::lock`] 串行化，否则两个
    /// 写者会算出同一代号（`rename` 在 unix 上静默覆盖、Windows 上报错）。
    pub fn save(&self, checkpoint: &Checkpoint) -> Result<u64, CheckpointError> {
        checkpoint.validate()?;
        checked_task_id(&checkpoint.task_id)?;
        let dir = self.task_dir(&checkpoint.task_id);
        fs::create_dir_all(&dir).map_err(io_error)?;
        let generation = latest_generation(&dir)?.map_or(1, |latest| latest + 1);
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

    /// 加载某任务最新检查点；不存在返回 `None`。
    pub fn load(&self, task_id: &str) -> Result<Option<Checkpoint>, CheckpointError> {
        checked_task_id(task_id)?;
        let dir = self.task_dir(task_id);
        let Some(path) = latest_checkpoint_file(&dir)? else {
            return Ok(None);
        };
        let bytes = fs::read(&path).map_err(io_error)?;
        let checkpoint: Checkpoint = serde_json::from_slice(&bytes)
            .map_err(|error| CheckpointError::InvalidState(error.to_string()))?;
        checkpoint.validate()?;
        if checkpoint.task_id != task_id {
            return Err(CheckpointError::InvalidState(format!(
                "任务 id 不匹配: 期望 {task_id}，实际 {}",
                checkpoint.task_id
            )));
        }
        Ok(Some(checkpoint))
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
                // `create_new` 保证记录只有一份：建目录与写记录之间停顿过久、目录被
                // 当成无主锁回收后，这里会失败而不是覆盖新持有者的记录。
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

    /// 带超时获取跨进程锁；默认 `lock()` 等 5 秒。
    ///
    /// 等待期间每秒探测一次残留锁：只有持有进程**确定已死**（或锁目录无 `owner.json`
    /// 且已超过 `OWNERLESS_STALE_AFTER`）才清理并重试——崩溃/kill 不会再永久卡住后续进程。
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

    /// 清理**确定已死**的持有者留下的锁目录；返回是否清掉了。
    ///
    /// 判定规则：`owner.json` 存在且主机名一致 ⇒ 用平台命令探测 pid；pid 确定不在
    /// 或探测不可用 ⇒ 只清前者。`owner.json` 缺失 ⇒ 仅在目录年龄超过
    /// `OWNERLESS_STALE_AFTER` 时清理。跨主机记录一律视为占有。
    ///
    /// 清理走「先原子改名隔离、再复核 owner 记录」：如果拿到锁的进程刚建目录还没写
    /// 记录，复核会发现内容变了并原样还回去，绝不删活锁。
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
            // 记录变了（新锁刚写完 owner）⇒ 还回去；但绝不覆盖已存在的目录：
            // POSIX 的 rename 能把「已 create_dir、记录未落盘」的空目录顶掉，
            // 那就把别人的锁弄丢了。还不了就留着隔离目录，宁可不删。
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
                // M04：`.lock` 锁目录、`.lock.reap-*` 隔离目录不是任务目录，外来噪声目录
                // 也不该让整张索引报错——`hex_decode` 是唯一判据，解不出来就跳过。
                let Ok(task_id) = hex_decode(name) else {
                    continue;
                };
                let Some(generation) = latest_generation(&path)? else {
                    continue;
                };
                let Some(file) = latest_checkpoint_file(&path)? else {
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

/// 空 task id 的十六进制是空串，`task_dir("")` 会落在版本目录本身——
/// `remove("")` 因此能一次删掉全部任务与锁目录。公开入口一律拒绝空 id。
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

/// 版本目录下的合法任务目录名：偶数长度全十六进制。
///
/// 保留项（`.lock`、`.lock.reap-*`）含 `.`，不可能满足十六进制约束，因此不需要
/// 额外的保留名清单。
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

fn latest_checkpoint_file(dir: &Path) -> Result<Option<PathBuf>, CheckpointError> {
    if !dir.exists() {
        return Ok(None);
    }
    let mut files = checkpoint_files(dir)?;
    files.sort();
    Ok(files.pop())
}

fn latest_generation(dir: &Path) -> Result<Option<u64>, CheckpointError> {
    let mut latest = None;
    for path in checkpoint_files(dir)? {
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(generation) = stem.parse::<u64>() else {
            continue;
        };
        latest = Some(latest.map_or(generation, |current: u64| current.max(generation)));
    }
    Ok(latest)
}
