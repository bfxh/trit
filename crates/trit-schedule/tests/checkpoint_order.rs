//! 检查点代号选取顺序回归（第 40 轮 CI 修复轮增补）。
//!
//! 目标：`load` 必须按**数值**代号取最新，多代共存时取最大、且非零填充代号
//! 不得因字典序被错误取舍（旧缺陷为 `files.sort(); files.pop()`）。

use std::fs;

use tempfile::TempDir;
use trit_schedule::checkpoint::FileCheckpointStore;

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

#[test]
fn load_returns_latest_among_multiple_valid_generations() {
    let (_dir, store) = new_store();
    plant(&store, "latest", 5, &[21]);
    plant(&store, "latest", 9, &[22]);

    let loaded = store
        .load("latest")
        .expect("多代共存时加载不应失败")
        .expect("应加载最新代");
    assert_eq!(loaded.state, vec![22], "必须取数值最大的有效代号");
}

#[test]
fn load_prefers_numeric_generation_over_lexical_filename_order() {
    let (_dir, store) = new_store();
    let dir = store.task_dir("order");
    fs::create_dir_all(&dir).expect("任务目录创建失败");
    // 非零填充代号：字典序上 "9" 大于 "20"，但数值上 20 更大。
    fs::write(
        dir.join("9.json"),
        r#"{"task_id":"order","version":1,"state":[42],"metrics":null}"#,
    )
    .unwrap();
    fs::write(
        dir.join("20.json"),
        r#"{"task_id":"order","version":1,"state":[7],"metrics":null}"#,
    )
    .unwrap();

    let loaded = store
        .load("order")
        .expect("加载不应失败")
        .expect("应有检查点");
    assert_eq!(loaded.state, vec![7], "不得按文件名字典序取舍代号");
}
