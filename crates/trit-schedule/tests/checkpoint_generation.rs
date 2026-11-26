//! R40-S8 回归：检查点代号溢出（S8-1）与最新代选取顺序（S8-2）。
//!
//! 覆盖：代号 `u64::MAX` 保存被结构化拒绝且不落盘；`u64::MAX-1` 推进到 `MAX`；
//! 目录混入外来 `.json`/无扩展名/`.json.bak`/`.tmp-*` 时按代号数值取真实最大代；
//! 空目录、仅临时残留返回 `None`；最新代损坏回退次新代。

use std::fs;

use tempfile::TempDir;
use trit_schedule::{Checkpoint, CheckpointError, FileCheckpointStore};

const MAX: u64 = u64::MAX;

fn new_store() -> (TempDir, FileCheckpointStore) {
    let dir = TempDir::new().expect("临时目录创建失败");
    let store = FileCheckpointStore::new(dir.path());
    (dir, store)
}

fn plant(store: &FileCheckpointStore, task_id: &str, generation: u64, state: &[u8]) {
    let dir = store.task_dir(task_id);
    fs::create_dir_all(&dir).expect("任务目录创建失败");
    let body = format!(
        "{{\"task_id\":\"{task_id}\",\"version\":1,\"state\":[{}],\"metrics\":null}}",
        state
            .iter()
            .map(|byte| byte.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    fs::write(dir.join(format!("{generation:020}.json")), body).expect("预置检查点写入失败");
}

fn dir_entries(store: &FileCheckpointStore, task_id: &str) -> Vec<String> {
    fs::read_dir(store.task_dir(task_id))
        .expect("读取任务目录失败")
        .map(|entry| {
            entry
                .expect("目录项读取失败")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

#[test]
fn save_at_max_generation_rejects_without_writing() {
    let (_dir, store) = new_store();
    plant(&store, "maxgen", MAX, &[9, 9, 9]);

    let result = store.save(&Checkpoint::new("maxgen", vec![1, 2, 3]));
    let Err(CheckpointError::InvalidState(_)) = result else {
        panic!("MAX 边界保存应返回 InvalidState，实测 {result:?}");
    };

    // 溢出拒绝后不得落盘新文件：目录里仍只有预置的 MAX 代号文件。
    let entries = dir_entries(&store, "maxgen");
    assert_eq!(entries, vec![format!("{MAX:020}.json")]);
    assert!(
        !entries
            .iter()
            .any(|name| *name == "00000000000000000000.json"),
        "不得回绕生成 gen=0 的文件: {entries:?}"
    );

    // 旧状态仍可读，未被半截写入污染。
    let loaded = store
        .load("maxgen")
        .expect("加载不应失败")
        .expect("应有检查点");
    assert_eq!(loaded.state, vec![9, 9, 9]);
}

#[test]
fn save_at_max_minus_one_advances_to_max() {
    let (_dir, store) = new_store();
    plant(&store, "edge", MAX - 1, &[9, 9, 9]);

    let generation = store
        .save(&Checkpoint::new("edge", vec![1, 2, 3]))
        .expect("MAX-1 下一次保存应成功");
    assert_eq!(generation, MAX, "新代号应为 MAX，实测 {generation}");

    let loaded = store
        .load("edge")
        .expect("加载不应失败")
        .expect("应有检查点");
    assert_eq!(loaded.state, vec![1, 2, 3]);
}

#[test]
fn load_picks_numeric_max_despite_noise_files() {
    let (_dir, store) = new_store();
    let dir = store.task_dir("f");
    fs::create_dir_all(&dir).expect("任务目录创建失败");

    // 字典序排在零填充代号之后的外来合法文档。
    fs::write(
        dir.join("zzz.json"),
        r#"{"task_id":"f","version":1,"state":[42],"metrics":null}"#,
    )
    .unwrap();
    // stem 可解析为 0，但数值上小于真实代号。
    fs::write(
        dir.join("000.json"),
        r#"{"task_id":"f","version":1,"state":[0],"metrics":null}"#,
    )
    .unwrap();
    // 无扩展名与 `.json.bak` 都不是候选。
    fs::write(dir.join("notes"), "garbage").unwrap();
    fs::write(dir.join("state.json.bak"), "garbage").unwrap();

    let generation = store
        .save(&Checkpoint::new("f", vec![5]))
        .expect("混入噪声文件后保存应成功");
    assert_eq!(generation, 1);

    let loaded = store.load("f").expect("加载不应失败").expect("应有检查点");
    assert_eq!(loaded.state, vec![5], "不得被字典序靠后的外来文档遮蔽");

    // 噪声文件原样保留，未被存储当作检查点消费。
    let entries = dir_entries(&store, "f");
    assert!(entries.contains(&"zzz.json".to_string()));
    assert!(entries.contains(&"000.json".to_string()));
}

#[test]
fn load_empty_dir_returns_none() {
    let (_dir, store) = new_store();
    assert!(store.load("nobody").expect("空目录不应报错").is_none());
}

#[test]
fn load_tmp_residue_only_returns_none_and_does_not_count_as_generation() {
    let (_dir, store) = new_store();
    let dir = store.task_dir("tmp-only");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(".tmp-123-456789"), "partial").unwrap();

    assert!(
        store
            .load("tmp-only")
            .expect("临时残留不应参与加载")
            .is_none()
    );

    // `.tmp-*` 不计入代号计算：首次保存仍从 1 起。
    let generation = store
        .save(&Checkpoint::new("tmp-only", vec![7]))
        .expect("残留临时文件旁保存应成功");
    assert_eq!(generation, 1);
}

#[test]
fn load_falls_back_to_previous_generation_when_latest_corrupt() {
    let (_dir, store) = new_store();
    plant(&store, "fallback", 1, &[11]);

    let dir = store.task_dir("fallback");
    fs::write(
        dir.join("00000000000000000002.json"),
        r#"{"task_id":"fallback","version":1,"state":[1,2"#,
    )
    .unwrap();

    let loaded = store
        .load("fallback")
        .expect("回退次新代不应报错")
        .expect("次新代应可读");
    assert_eq!(loaded.state, vec![11]);
}

#[test]
fn load_all_candidates_corrupt_returns_invalid_state() {
    let (_dir, store) = new_store();
    let dir = store.task_dir("broken");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("00000000000000000001.json"),
        r#"{"task_id":"broken","version":1,"state":["#,
    )
    .unwrap();

    let result = store.load("broken");
    assert!(
        matches!(result, Err(CheckpointError::InvalidState(_))),
        "全部候选损坏应返回 InvalidState，实测 {result:?}"
    );
}
