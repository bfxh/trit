//! 并行格式化与串行逐字节一致、且顺序稳定。
//!
//! 用 fixtures 里的真实语料（含 84 KB 大文档），对 jobs = 2/3/8 与串行结果逐项比较。

use std::path::{Path, PathBuf};

use trit_fmt::{format_json, format_many};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/json")
}

fn docs() -> Vec<String> {
    let dir = fixtures();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("读 fixtures").flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if !name.ends_with(".json") || name.ends_with(".expected.json") {
            continue;
        }
        out.push(std::fs::read_to_string(&path).expect("读输入"));
    }
    out.sort();
    assert!(out.len() >= 10, "语料太少：{}", out.len());
    out
}

#[test]
fn parallel_output_is_byte_identical_to_serial() {
    let sources = docs();
    let refs: Vec<&str> = sources.iter().map(String::as_str).collect();
    let serial: Vec<_> = refs.iter().map(|doc| format_json(doc)).collect();
    for jobs in [2usize, 3, 8] {
        let parallel = format_many(&refs, jobs);
        assert_eq!(parallel.len(), serial.len(), "jobs={jobs}");
        for (index, (left, right)) in parallel.iter().zip(&serial).enumerate() {
            match (left, right) {
                (Ok(left), Ok(right)) => assert_eq!(left, right, "jobs={jobs} 第 {index} 份不一致"),
                (Err(_), Err(_)) => {}
                _ => panic!("jobs={jobs} 第 {index} 份：成功/失败状态不一致"),
            }
        }
    }
}

#[test]
fn parallel_preserves_order_with_mixed_content() {
    // 长短交替，确保分块边界处不会错位。
    let docs: Vec<String> = (0..17)
        .map(|index| {
            if index % 3 == 0 {
                format!(r#"{{"index":{index},"rows":[1,2,3,4,5,6,7,8,9,10]}}"#)
            } else {
                format!(r#"{{"index":{index}}}"#)
            }
        })
        .collect();
    let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
    for jobs in [2usize, 4, 7, 16] {
        let out = format_many(&refs, jobs);
        for (index, result) in out.iter().enumerate() {
            let text = result.as_ref().expect("合法文档");
            assert!(
                text.contains(&format!("\"index\": {index}")),
                "jobs={jobs} 第 {index} 份被错位：{text}"
            );
        }
    }
}

#[test]
fn single_document_and_serial_fallbacks() {
    assert_eq!(format_many(&[], 4).len(), 0);
    let one = [r#"{"a":1}"#];
    assert_eq!(format_many(&one, 8)[0].as_ref().unwrap(), "{ \"a\": 1 }\n");
    assert_eq!(format_many(&one, 0)[0].as_ref().unwrap(), "{ \"a\": 1 }\n");
}
