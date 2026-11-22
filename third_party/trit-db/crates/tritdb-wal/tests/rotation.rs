//! Segment rotation inside one WAL file.

use std::fs;
use std::io::Write;

use tritdb_testkit::TempDir;
use tritdb_wal::{RecordKind, TailState, WalReader, WalWriter, SEGMENT_MAGIC};

fn headers(bytes: &[u8]) -> usize {
    bytes
        .windows(4)
        .filter(|window| *window == SEGMENT_MAGIC)
        .count()
}

fn rotated_segments(path: &std::path::Path) -> usize {
    let parent = path.parent().expect("parent");
    std::fs::read_dir(parent)
        .expect("read dir")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".seg"))
        .count()
}

#[test]
fn rotation_adds_segment_headers_and_keeps_records_readable() {
    let dir = TempDir::new("wal-rotation");
    let path = dir.join("rotate.wal");
    let (mut writer, _) = WalWriter::open_with_limit(&path, 2048).expect("open");
    let payload = vec![7u8; 200];

    for i in 0..40 {
        let lsn = writer
            .append(RecordKind::Put, i as u64, &payload, false)
            .expect("append");
        assert_eq!(lsn, i as u64 + 1);
    }
    drop(writer);

    let bytes = fs::read(&path).expect("read");
    assert_eq!(headers(&bytes), 1, "active segment has one header");
    assert!(
        rotated_segments(&path) > 0,
        "expected rotated .seg files, saw none"
    );

    let scan = WalReader::open(&path).expect("scan");
    assert_eq!(scan.records.len(), 40);
    assert!(scan.segments > 1);
    assert_eq!(scan.last_lsn(), 40);
    for (i, record) in scan.records.iter().enumerate() {
        assert_eq!(record.lsn, i as u64 + 1);
        assert_eq!(record.payload, payload);
    }

    // Reopen continues from the last record, not from the last segment start.
    let (mut writer, scan) = WalWriter::open_with_limit(&path, 2048).expect("reopen");
    assert_eq!(scan.records.len(), 40);
    let lsn = writer
        .append(RecordKind::Commit, 99, &[], true)
        .expect("append after reopen");
    assert_eq!(lsn, 41);
}

#[test]
fn reset_drops_all_segments() {
    let dir = TempDir::new("wal-reset-rotate");
    let path = dir.join("rotate.wal");
    let (mut writer, _) = WalWriter::open_with_limit(&path, 2048).expect("open");
    for _ in 0..40 {
        writer
            .append(RecordKind::Put, 1, &[1u8; 200], false)
            .expect("append");
    }
    writer.reset(100).expect("reset");
    drop(writer);

    let bytes = fs::read(&path).expect("read");
    assert_eq!(headers(&bytes), 1);
    assert_eq!(rotated_segments(&path), 0);
    let scan = WalReader::open(&path).expect("scan");
    assert_eq!(scan.first_lsn, 100);
    assert!(scan.records.is_empty());
    assert_eq!(scan.segments, 1);
}

#[test]
fn torn_new_segment_header_is_repaired() {
    let dir = TempDir::new("wal-torn-rotate");
    let path = dir.join("rotate.wal");
    let (mut writer, _) = WalWriter::open_with_limit(&path, 2048).expect("open");
    for _ in 0..20 {
        writer
            .append(RecordKind::Put, 1, &[1u8; 200], false)
            .expect("append");
    }
    drop(writer);

    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open for append");
    file.write_all(b"TRWS\x00").expect("write partial header");
    drop(file);

    let scan = WalReader::open(&path).expect("scan torn");
    assert!(matches!(scan.tail, TailState::Torn { .. }));

    let (mut writer, _) = WalWriter::open_with_limit(&path, 2048).expect("reopen");
    let lsn = writer
        .append(RecordKind::Commit, 1, &[], true)
        .expect("append after repair");
    assert_eq!(lsn, 21);
}
