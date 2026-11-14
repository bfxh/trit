//! 格式注册清单：名字/扩展名唯一、读写能力与实现一致、JSON 形状稳定。

use std::collections::{BTreeMap, BTreeSet};

use trit_wspace::formats::{registry, registry_json};

#[test]
fn registry_is_non_empty_and_names_unique() {
    let entries = registry();
    assert!(!entries.is_empty());
    let mut names = BTreeSet::new();
    for entry in entries {
        assert!(names.insert(entry.name), "重复格式名 {}", entry.name);
        assert!(!entry.note.is_empty(), "{} 缺边界说明", entry.name);
    }
}

#[test]
fn extensions_are_unique() {
    let mut seen = BTreeMap::new();
    for entry in registry() {
        for extension in entry.extensions {
            assert!(extension.starts_with('.'), "扩展名 {extension} 缺少前导点");
            if let Some(previous) = seen.insert(*extension, entry.name) {
                panic!("扩展名 {extension} 同时属于 {previous} 与 {}", entry.name);
            }
        }
    }
}

#[test]
fn claimed_io_matches_implemented_set() {
    let io: Vec<&str> = registry()
        .iter()
        .filter(|entry| entry.read && entry.write)
        .map(|entry| entry.name)
        .collect();
    assert_eq!(
        io,
        [
            "wsp1",
            "wsp1g",
            "wsp2",
            "wsp2g",
            "wsp3",
            "wsp4",
            "safetensors",
            "gguf"
        ],
        "可读写清单变化时必须同步改这里（清单就是承诺）"
    );
}

#[test]
fn unimplemented_formats_are_listed_honestly() {
    let missing: Vec<&str> = registry()
        .iter()
        .filter(|entry| !entry.read && !entry.write)
        .map(|entry| entry.name)
        .collect();
    assert_eq!(missing, ["pytorch-state-dict", "llama.cpp-gguf"]);
}

#[test]
fn json_has_schema_and_all_fields() {
    let value = registry_json();
    assert_eq!(value["schema"], "trit-wspace.formats.v1");
    let entries = value["formats"].as_array().expect("formats");
    assert_eq!(entries.len(), registry().len());
    for entry in entries {
        assert!(entry["name"].is_string());
        assert!(entry["extensions"].is_array());
        assert!(entry["read"].is_boolean());
        assert!(entry["write"].is_boolean());
        assert!(entry["note"].is_string());
    }
}
