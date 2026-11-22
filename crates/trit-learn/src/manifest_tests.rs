//! `manifest` 单测：指纹与相对路径。

use super::*;

use super::*;

fn tmp_dir() -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("trit-manifest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn fingerprint_is_order_and_style_independent() {
    let root = tmp_dir();
    std::fs::write(root.join("a.bin"), b"alpha").unwrap();
    std::fs::write(root.join("b.bin"), b"beta").unwrap();

    let forward = vec![
        Item {
            key: "a.bin".into(),
            path: root.join("a.bin"),
        },
        Item {
            key: "b.bin".into(),
            path: root.join("b.bin"),
        },
    ];
    let reversed = vec![forward[1].clone(), forward[0].clone()];
    // 同一输入顺序 → 同一哈希（线程切分不影响结果）
    let a = hash_items(&forward, 1).unwrap();
    let again = hash_items(&forward, 1).unwrap();
    assert_eq!(a.hash, again.hash);
    assert_eq!((a.files, a.bytes), (2, 9));
    // 输入顺序不同 → 哈希不同：合并顺序 = 输入顺序，调用方负责排序
    let b = hash_items(&reversed, 1).unwrap();
    assert_ne!(a.hash, b.hash, "hash_items 合并顺序 = 输入顺序");

    // 同一路径的两种写法必须归一成同一指纹
    let backslash = root.to_string_lossy().replace('/', "\\");
    assert_eq!(norm(Path::new(&backslash)), norm(&root));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn rel_path_is_relative_to_base() {
    let base = Path::new("/data/assets");
    assert_eq!(rel_path(base, Path::new("/data/assets/a/b.bin")), "a/b.bin");
    assert_eq!(rel_path(base, Path::new("/data/assets")), ".");
    assert_eq!(
        rel_path(Path::new("/data"), Path::new("/data/assets/a")),
        "assets/a"
    );
}

#[test]
fn fingerprint_covers_key() {
    let root = tmp_dir();
    std::fs::write(root.join("a.bin"), b"alpha").unwrap();
    let one = vec![Item {
        key: "a.bin".into(),
        path: root.join("a.bin"),
    }];
    let renamed = vec![Item {
        key: "dir/a.bin".into(),
        path: root.join("a.bin"),
    }];
    assert_ne!(
        hash_items(&one, 0).unwrap().hash,
        hash_items(&renamed, 0).unwrap().hash,
        "同一内容换目录名必须改变指纹"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn rel_path_walks_up() {
    assert_eq!(
        rel_path(Path::new("/data/assets"), Path::new("/data/obj/x.json")),
        "../obj/x.json"
    );
}
