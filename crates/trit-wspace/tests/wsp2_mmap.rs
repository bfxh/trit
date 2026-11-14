//! WSP2 mmap：映射读取与内存切片读取逐位一致；坏输入报错。

use std::fs;
use std::path::PathBuf;

use trit_wspace::wsp2::BlockIndex;
use trit_wspace::wsp2_map::MappedArtifact;
use trit_wspace::{Layer, Net, to_wsp2};

fn sample_net() -> Net {
    Net::new(vec![
        Layer::with_scales(2, 3, &[1, 0, -1, 0, 1, -1], &[1, -1], vec![0.5, 1.5]).expect("l0"),
        Layer::new(1, 2, &[1, -1], &[0]).expect("l1"),
    ])
    .expect("net")
}

fn write_artifact(tag: &str) -> (PathBuf, Vec<u8>) {
    let dir = std::env::temp_dir().join(format!("trit-mmap-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("model.wsp2");
    let bytes = to_wsp2(&sample_net()).expect("encode");
    fs::write(&path, &bytes).expect("write");
    (path, bytes)
}

fn same_layer(a: &Layer, b: &Layer) -> bool {
    a.trits() == b.trits()
        && a.bias() == b.bias()
        && a.scales() == b.scales()
        && a.bias_f32() == b.bias_f32()
}

#[test]
fn mmap_layer_and_rows_match_in_memory() {
    let (path, bytes) = write_artifact("read");
    let index = BlockIndex::scan(&bytes).expect("scan");
    let map = MappedArtifact::open(&path).expect("mmap");
    assert_eq!(map.len(), bytes.len());
    assert!(!map.is_empty());
    let net = sample_net();
    for layer in 0..net.layers().len() {
        let from_map = map.read_layer(&index, layer).expect("map layer");
        let direct = index.read_layer(&bytes, layer).expect("direct layer");
        assert!(same_layer(&from_map, &direct));
        assert!(same_layer(&from_map, &net.layers()[layer]));
    }
    let rows = map.read_rows(&index, 0, 1, 1).expect("rows");
    let direct = index.read_rows(&bytes, 0, 1, 1).expect("direct rows");
    assert!(same_layer(&rows, &direct));
    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[test]
fn mmap_rejects_missing_file() {
    let error = MappedArtifact::open("no-such-artifact.wsp2").expect_err("必须失败");
    assert!(format!("{error}").contains("mmap"), "{error}");
}

#[test]
fn mmap_bytes_are_the_file_bytes() {
    let (path, bytes) = write_artifact("bytes");
    let map = MappedArtifact::open(&path).expect("mmap");
    assert_eq!(map.bytes(), bytes.as_slice());
    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[test]
fn mmap_rejects_truncated_source() {
    let (path, bytes) = write_artifact("trunc");
    fs::write(&path, &bytes[..bytes.len() / 2]).expect("truncate");
    let map = MappedArtifact::open(&path).expect("mmap");
    let index = BlockIndex::scan(&bytes).expect("scan");
    // 源文件变短后，按索引读取必须报错而不是越界 panic
    assert!(map.read_layer(&index, 0).is_err());
    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}
