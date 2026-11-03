//! 现场采集（N3）的测试：路径抽取纪律（去重/坏行/非 module 计数）、
//! 采集判据（≥N 条记录、汇总行可复算）、单文件失败不中断。
//! `read` 闭包注入内存字节——不碰真实文件系统。

use std::io;
use std::path::{Path, PathBuf};

use trit_re::collect::{collect, module_paths};
use trit_re::ledger;
use trit_re::{CollectSummary, analyze};

/// 一个 Triage 在台账里占的行数（ledger::jsonl 的长度）。
fn ledger_row_count(triage: &trit_re::Triage) -> usize {
    ledger::jsonl(triage).len()
}

/// 内存文件系统：路径 → 字节；未登记的路径 = 读取失败。
fn memory_fs<'a>(files: &'a [(&str, &[u8])]) -> impl Fn(&Path) -> io::Result<Vec<u8>> + 'a {
    let entries: Vec<(PathBuf, Vec<u8>)> = files
        .iter()
        .map(|(path, bytes)| (PathBuf::from(path), bytes.to_vec()))
        .collect();
    move |path: &Path| {
        entries
            .iter()
            .find(|(candidate, _)| candidate == path)
            .map(|(_, bytes)| bytes.clone())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "not in memory fs"))
    }
}

/// 最小合法 PE：MZ 头 + e_lfanew 指到 0x40 的 PE 签名（analyze 能判出格式事实）。
fn tiny_image() -> Vec<u8> {
    let mut bytes = vec![0u8; 0x40];
    bytes[0] = b'M';
    bytes[1] = b'Z';
    bytes[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
    bytes.extend_from_slice(b"PE\0\0");
    bytes[0x4 + 4] = 0x64; // machine = ARM64（0xaa64 低字节）
    bytes[0x4 + 5] = 0xaa;
    bytes
}

fn event_line(path: &str) -> String {
    // JSON 字符串里反斜杠要双写（Windows 路径 `C:\a.dll` → `C:\\a.dll`）。
    let escaped = path.replace('\\', "\\\\");
    format!(
        "{{\"type\":\"module\",\"ts_ms\":1,\"pid\":100,\"process\":\"p\",\"path\":\"{escaped}\",\"exports\":0}}"
    )
}

#[test]
fn module_paths_dedup_keep_order_and_count_noise() {
    let events = format!(
        "{{\"type\":\"process\",\"pid\":1}}\n{}\nnot json\n{}\n{}\n{{\"type\":\"connection\",\"pid\":2}}\n{{\"type\":\"module\",\"path\":\"\"}}\n",
        event_line(r"C:\a.dll"),
        event_line(r"C:\b.dll"),
        event_line(r"C:\a.dll"),
    );
    let (paths, module_events, deduped, non_module, bad_lines) = module_paths(&events);
    // 去重保序：a 先出现，b 其次，重复的 a 被跳过。
    assert_eq!(
        paths,
        vec![PathBuf::from(r"C:\a.dll"), PathBuf::from(r"C:\b.dll")]
    );
    // module_events 按类型计数（4 = 3 有路径 + 1 空 path）；空 path 额外计坏行。
    assert_eq!(module_events, 4);
    assert_eq!(deduped, 1);
    assert_eq!(non_module, 2, "process/connection 各一条");
    assert_eq!(bad_lines, 2, "坏 JSON 行 + 空 path 各计 1");
}

#[test]
fn collect_meets_the_n3_criterion() {
    // 判据：一次采集产出 ≥ N 条记录——每个模块至少 1 行台账（xref/function
    // 记录随模块内容产生），末尾一条汇总行可直接复算模块数/字节/耗时。
    let image = tiny_image();
    let events = format!(
        "{}\n{}\n",
        event_line(r"C:\mod\a.dll"),
        event_line(r"C:\mod\b.dll")
    );
    let (rows, summary) = collect(
        &events,
        memory_fs(&[(r"C:\mod\a.dll", &image), (r"C:\mod\b.dll", &image[..0x40])]),
    );
    // ≥ N 条记录：两个模块各产出至少一行 + 汇总行。
    assert!(rows.len() >= 3, "rows={rows:?}");
    // 汇总行是合法 JSON 且字段自洽。
    let last = rows.last().expect("汇总行存在");
    let parsed: CollectSummary = serde_json::from_str(last).expect("汇总行是合法 JSON");
    assert_eq!(parsed.kind, "collect-summary");
    assert_eq!(parsed.modules, 2);
    assert_eq!(parsed.analyzed, 2);
    assert_eq!(parsed.unreadable, 0);
    assert_eq!(parsed.deduped, 0);
    assert_eq!(parsed.bad_lines, 0);
    assert_eq!(parsed.module_events, 2);
    // 字节汇总 = 两个模块的实际大小（0x44 + 0x40）。
    assert_eq!(parsed.bytes, 0x44 + 0x40);
    // 汇总行与返回值一致（同一份数据，两种消费方式）。
    assert_eq!(*last, serde_json::to_string(&summary).unwrap());
}

#[test]
fn unreadable_modules_do_not_abort_the_collection() {
    let image = tiny_image();
    let events = format!(
        "{}\n{}\n{}\n",
        event_line(r"C:\gone.dll"),
        event_line(r"C:\ok.dll"),
        event_line(r"C:\gone2.dll")
    );
    let (rows, summary) = collect(&events, memory_fs(&[(r"C:\ok.dll", &image)]));
    // 1 个成功 + 2 个失败，顺序不受影响。
    assert_eq!(summary.analyzed, 1);
    assert_eq!(summary.unreadable, 2);
    assert_eq!(summary.modules, 3);
    // 失败原因如实上报（≤8 条），带 OS 错误文本。
    assert_eq!(summary.errors.len(), 2);
    assert!(
        summary.errors[0].contains(r"C:\gone.dll"),
        "{:?}",
        summary.errors
    );
    // 失败不占台账行：只有 ok 模块的台账行（多行：事实/判定各一条）+ 汇总行。
    let triage = analyze(&image);
    let expected = ledger_row_count(&triage);
    assert_eq!(rows.len(), expected + 1, "rows={rows:?}");
    // 汇总行在末尾。
    assert!(rows.last().expect("汇总行").contains("collect-summary"));
    // 记录能独立复核：ok 模块的 sha256 与 analyze 直算一致。
    assert!(
        rows[0].contains(&triage.sha256),
        "首行应为 ok 模块的台账：{}",
        rows[0]
    );
}

#[test]
fn collect_of_empty_stream_is_a_green_summary() {
    let (rows, summary) = collect("", memory_fs(&[]));
    assert_eq!(rows.len(), 1);
    assert_eq!(summary.modules, 0);
    assert_eq!(summary.analyzed, 0);
    assert_eq!(summary.module_events, 0);
    let parsed: serde_json::Value = serde_json::from_str(&rows[0]).unwrap();
    assert_eq!(parsed["kind"], "collect-summary");
    assert!(summary.elapsed_ms <= 5_000, "空流不应耗时");
}
