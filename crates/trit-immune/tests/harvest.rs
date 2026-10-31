//! 项目收割：构建产物清理、语言普查贴标签、源码路径注册表。

use std::fs;
use std::path::PathBuf;
use trit_immune::harvest::{harvest, Registry};

fn temp_root(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("trit-immune-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn harvest_cleans_build_artifacts_and_registers() {
    let root = temp_root("harvest");

    // 造一个假项目：源码 + 构建产物
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
    fs::write(root.join("lib.py"), "x = 1").unwrap();
    fs::create_dir_all(root.join("target/debug")).unwrap();
    fs::write(root.join("target/debug/app.exe"), b"\x00\x01").unwrap();
    fs::create_dir_all(root.join("node_modules/left-pad")).unwrap();
    fs::write(root.join("node_modules/left-pad/index.js"), "module.exports=1").unwrap();
    fs::create_dir_all(root.join("__pycache__")).unwrap();
    fs::write(root.join("__pycache__/lib.cpython-311.pyc"), b"\x00").unwrap();

    // 收割：先删构建
    let report = harvest(&root).expect("harvest");
    assert!(!root.join("target").exists(), "target 应整棵删除");
    assert!(!root.join("node_modules").exists());
    assert!(!root.join("__pycache__").exists());
    assert!(root.join("src/main.rs").exists(), "源码不能动");
    assert!(root.join("lib.py").exists());
    assert_eq!(report.removed.len(), 3, "三个构建目录");
    assert!(report.refused.is_empty());

    // 语言普查贴标签：node_modules 已删 → JS 不计；Python 与 Rust 各 1 → 同数按名字序
    assert_eq!(
        report.languages,
        vec![("Python".to_string(), 1), ("Rust".to_string(), 1)]
    );

    // 注册表：源码路径正式入账（"源代码在某某路径"）
    let reg_path = root.parent().unwrap().join(format!(
        "trit-immune-harvest-log-{}.log",
        std::process::id()
    ));
    let registry = Registry::open(&reg_path);
    let line = registry.record(&report).unwrap();
    assert!(line.starts_with("HARVEST\t"), "{line}");
    assert!(line.contains("Python=1,Rust=1"), "{line}");
    let entries = registry.entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].root.ends_with("harvest-proj") || entries[0].root.contains("harvest"));
    assert_eq!(entries[0].removed, 3);

    // 再收割一次（幂等）：无构建产物可删，注册表再记一行
    let report2 = harvest(&root).unwrap();
    assert!(report2.removed.is_empty());
    registry.record(&report2).unwrap();
    assert_eq!(registry.entries().unwrap().len(), 2);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_file(&reg_path);
}

#[test]
fn harvest_refuses_symlinks_without_deleting() {
    let root = temp_root("harvest-symlink");
    fs::write(root.join("keep.rs"), "fn main() {}").unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc", root.join("target")).unwrap();
        let report = harvest(&root).unwrap();
        assert_eq!(report.refused.len(), 1, "符号链接必须如实上报且不删");
        assert!(root.join("keep.rs").exists());
    }
    #[cfg(windows)]
    {
        // Windows 上创建目录联接（junction）不需要特权，且不是符号链接文件类型——
        // 这里退而验证围栏：删除被拒时至少不越界。junction 用 symlink_dir 创建，
        // 若无特权则跳过该断言。
        if std::os::windows::fs::symlink_dir(root.join("src"), root.join("target")).is_ok() {
            let report = harvest(&root).unwrap();
            assert_eq!(report.refused.len(), 1, "目录符号链接应被拒");
            assert!(root.join("src").exists(), "符号链接被拒，源码不受影响");
        }
    }
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn harvest_rejects_non_directory() {
    let p = temp_root("harvest-file");
    let file = p.join("f.txt");
    fs::write(&file, "x").unwrap();
    assert!(harvest(&file).is_err());
    let _ = fs::remove_dir_all(&p);
}
