//! WAL truncation, corruption, reset, and record-shape tests.

use std::fs;
use std::io::Write;
use std::path::Path;

use tritdb_testkit::TempDir;
use tritdb_wal::{
    parse_wal, RecordKind, TailState, WalError, WalReader, WalWriter, MAX_RECORD_PAYLOAD,
    RECORD_HEADER_SIZE, SEGMENT_HEADER_SIZE,
};

fn write_records(writer: &mut WalWriter, count: u64) {
    for index in 0..count {
        let payload = format!("payload-{index}").into_bytes();
        writer
            .append(RecordKind::Put, index + 1, &payload, true)
            .expect("append record");
    }
}

fn build_wal(dir: &TempDir, name: &str, count: u64) -> (std::path::PathBuf, Vec<u8>) {
    let path = dir.join(name);
    let mut writer = WalWriter::open(&path).expect("open writer").0;
    write_records(&mut writer, count);
    writer.sync().expect("sync");
    drop(writer);
    let bytes = fs::read(&path).expect("read wal");
    (path, bytes)
}

#[test]
fn every_prefix_is_either_valid_or_torn_without_panic() {
    let dir = TempDir::new("wal-prefix");
    let (_path, raw) = build_wal(&dir, "prefix.wal", 4);
    assert!(raw.len() > SEGMENT_HEADER_SIZE + RECORD_HEADER_SIZE);

    for cut in 0..SEGMENT_HEADER_SIZE {
        let error = parse_wal(&raw[..cut]).expect_err("short segment");
        assert!(matches!(
            error,
            WalError::BadSegmentHeader { .. } | WalError::Io(_)
        ));
    }
    for cut in SEGMENT_HEADER_SIZE..=raw.len() {
        let scan = parse_wal(&raw[..cut]).expect("valid prefix");
        assert_eq!(scan.first_lsn, 1);
        assert!(scan.valid_end <= cut as u64);
        if cut == raw.len() {
            assert_eq!(scan.tail, TailState::Clean);
            assert_eq!(scan.records.len(), 4);
        }
    }
}

#[test]
fn short_or_partial_segment_files_are_repaired() {
    let dir = TempDir::new("wal-repair");
    for length in 0..SEGMENT_HEADER_SIZE {
        let path = dir.join(&format!("short-{length}.wal"));
        fs::write(&path, vec![0u8; length]).expect("write short file");
        let (mut writer, scan) = WalWriter::open(&path).expect("repair");
        assert_eq!(scan.first_lsn, 1);
        assert!(scan.records.is_empty());
        assert_eq!(fs::metadata(&path).expect("metadata").len(), 32);
        writer
            .append(RecordKind::Begin, 9, b"", true)
            .expect("append after repair");
        drop(writer);
        let reread = WalReader::open(&path).expect("read repaired");
        assert_eq!(reread.records.len(), 1);
        assert_eq!(reread.records[0].txn_id, 9);
    }
}

#[test]
fn torn_tail_is_trimmed_before_append() {
    let dir = TempDir::new("wal-torn");
    let path = dir.join("torn.wal");
    let mut writer = WalWriter::open(&path).expect("open").0;
    write_records(&mut writer, 3);
    writer.sync().expect("sync");
    drop(writer);

    let mut bytes = fs::read(&path).expect("read");
    bytes.truncate(bytes.len() - 3);
    fs::write(&path, &bytes).expect("truncate");
    let torn = WalReader::open(&path).expect("scan torn");
    assert!(matches!(torn.tail, TailState::Torn { .. }));

    let (mut writer, scan) = WalWriter::open(&path).expect("repair torn");
    assert_eq!(scan.records.len(), 2, "complete records must survive");
    assert!(matches!(scan.tail, TailState::Torn { .. }));
    assert_eq!(scan.valid_end, fs::metadata(&path).expect("metadata").len());
    let next = writer
        .append(RecordKind::Commit, 3, b"", true)
        .expect("append after torn");
    assert_eq!(next, 3, "torn record is discarded and its LSN is reused");
    drop(writer);
    let reread = WalReader::open(&path).expect("reread");
    assert_eq!(reread.records.len(), 3);
    assert_eq!(reread.tail, TailState::Clean);
}

#[test]
fn all_record_kinds_preserve_txn_payload_and_lsn() {
    let dir = TempDir::new("wal-kinds");
    let path = dir.join("kinds.wal");
    let mut writer = WalWriter::open(&path).expect("open").0;
    let records = [
        (RecordKind::Begin, 1u64, Vec::new()),
        (RecordKind::Put, 2u64, b"put".to_vec()),
        (RecordKind::Delete, 3u64, b"delete".to_vec()),
        (RecordKind::Commit, 4u64, Vec::new()),
        (RecordKind::Abort, 5u64, b"abort".to_vec()),
    ];
    for (kind, txn, payload) in &records {
        writer.append(*kind, *txn, payload, false).expect("append");
    }
    writer.sync().expect("sync");
    drop(writer);
    let scan = WalReader::open(&path).expect("read");
    assert_eq!(scan.records.len(), records.len());
    for (index, (record, (kind, txn, payload))) in
        scan.records.iter().zip(records.iter()).enumerate()
    {
        assert_eq!(record.lsn, index as u64 + 1);
        assert_eq!(record.kind, *kind);
        assert_eq!(record.txn_id, *txn);
        assert_eq!(record.payload, *payload);
    }
}

#[test]
fn payload_limit_is_enforced_before_writing() {
    let dir = TempDir::new("wal-limit");
    let path = dir.join("limit.wal");
    let mut writer = WalWriter::open(&path).expect("open").0;
    let oversized = vec![0u8; MAX_RECORD_PAYLOAD + 1];
    let error = writer
        .append(RecordKind::Put, 1, &oversized, false)
        .expect_err("oversized payload");
    assert!(matches!(
        error,
        WalError::PayloadTooLarge { len, max }
            if len == MAX_RECORD_PAYLOAD + 1 && max == MAX_RECORD_PAYLOAD
    ));
    assert_eq!(fs::metadata(&path).expect("metadata").len(), 32);

    let large = vec![7u8; 1024 * 1024];
    writer
        .append(RecordKind::Put, 2, &large, true)
        .expect("1 MiB record");
    drop(writer);
    let scan = WalReader::open(&path).expect("read large");
    assert_eq!(scan.records[0].payload, large);
}

#[test]
fn lsn_is_continuous_across_reopen() {
    let dir = TempDir::new("wal-lsn");
    let path = dir.join("lsn.wal");
    let mut writer = WalWriter::open(&path).expect("open").0;
    write_records(&mut writer, 100);
    drop(writer);

    let (mut writer, scan) = WalWriter::open(&path).expect("reopen");
    assert_eq!(scan.records.len(), 100);
    let next = writer
        .append(RecordKind::Commit, 101, b"", true)
        .expect("append");
    assert_eq!(next, 101);
    drop(writer);
    let scan = WalReader::open(&path).expect("read");
    for (index, record) in scan.records.iter().enumerate() {
        assert_eq!(record.lsn, index as u64 + 1);
    }
}

#[test]
fn reset_replaces_the_segment_and_lsn_origin() {
    let dir = TempDir::new("wal-reset");
    let path = dir.join("reset.wal");
    let mut writer = WalWriter::open(&path).expect("open").0;
    write_records(&mut writer, 4);
    writer.reset(1000).expect("reset");
    assert_eq!(writer.next_lsn(), 1000);
    writer
        .append(RecordKind::Begin, 7, b"", true)
        .expect("append after reset");
    drop(writer);
    let scan = WalReader::open(&path).expect("read reset");
    assert_eq!(scan.first_lsn, 1000);
    assert_eq!(scan.records.len(), 1);
    assert_eq!(scan.records[0].lsn, 1000);
    assert_eq!(scan.valid_end, 32 + 32);
}

#[test]
fn critical_header_corruptions_are_structured() {
    let dir = TempDir::new("wal-corrupt");
    let path = dir.join("corrupt.wal");
    let mut writer = WalWriter::open(&path).expect("open").0;
    writer
        .append(RecordKind::Put, 5, b"payload", true)
        .expect("append");
    drop(writer);
    let raw = fs::read(&path).expect("read");

    let mut segment_magic = raw.clone();
    segment_magic[0] ^= 0xff;
    assert!(matches!(
        parse_wal(&segment_magic),
        Err(WalError::BadSegmentMagic { .. })
    ));

    let mut segment_version = raw.clone();
    segment_version[4] = 9;
    assert!(matches!(
        parse_wal(&segment_version),
        Err(WalError::BadSegmentVersion { found: 9 })
    ));

    let mut record_magic = raw.clone();
    record_magic[32] ^= 0xff;
    assert!(matches!(
        parse_wal(&record_magic),
        Err(WalError::BadRecordMagic { .. })
    ));

    let mut record_kind = raw.clone();
    record_kind[32 + 5] = 0;
    assert!(matches!(
        parse_wal(&record_kind),
        Err(WalError::BadRecordKind { found: 0, .. })
    ));

    let mut record_flags = raw.clone();
    record_flags[32 + 6] = 1;
    assert!(matches!(
        parse_wal(&record_flags),
        Err(WalError::BadFlags { found: 1, .. })
    ));

    let mut payload = raw.clone();
    payload[64] ^= 0x01;
    assert!(matches!(
        parse_wal(&payload),
        Err(WalError::ChecksumMismatch { .. })
    ));
}

#[test]
fn every_byte_mutation_does_not_panic() {
    let dir = TempDir::new("wal-mutate");
    let (path, raw) = build_wal(&dir, "mutate.wal", 2);
    assert!(Path::new(&path).exists());
    for index in 0..raw.len() {
        let mut mutated = raw.clone();
        mutated[index] ^= 0xff;
        let _ = parse_wal(&mutated);
    }
}

#[test]
fn reader_rejects_short_file_but_writer_repairs_it() {
    let dir = TempDir::new("wal-reader");
    for length in [0usize, 1, 16, 31] {
        let path = dir.join(&format!("reader-{length}.wal"));
        let mut file = fs::File::create(&path).expect("create");
        file.write_all(&vec![0u8; length]).expect("write");
        drop(file);
        assert!(matches!(
            WalReader::open(&path),
            Err(WalError::BadSegmentHeader { .. })
        ));
        let (_, scan) = WalWriter::open(&path).expect("writer repairs");
        assert!(scan.records.is_empty());
    }
}
