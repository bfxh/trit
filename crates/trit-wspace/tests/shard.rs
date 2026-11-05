//! 审计日志分片：header / oracle 表 / 跨片链 / 重放 / 篡改。

use serde_json::{Value, json};
use trit_wspace::{
    AuditLog, Edit, Layer, Net, Oracle, Patch, ShardCursor, ShardManifest, ShardReplay, ShardedLog,
    Trit, checksum,
};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn break_patch() -> Patch {
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
}

fn fix_patch() -> Patch {
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Neg,
    }])
}

fn xor_oracle() -> Oracle {
    Oracle::boolean(2, |bits| bits[0] != bits[1]).expect("oracle")
}

/// 三条记录，共享同一个验证集：第 2/3 条写 `oracle_ref`。
fn sample_log() -> (AuditLog, Net) {
    let start = xor_net();
    let mut broken = start.clone();
    break_patch().apply(&mut broken).expect("break");
    let mut fixed = broken.clone();
    fix_patch().apply(&mut fixed).expect("fix");
    let oracle = xor_oracle();
    let mut log = AuditLog::new();
    log.record_with_oracle(
        "break",
        &start,
        &broken,
        break_patch(),
        true,
        false,
        Some(&oracle),
    )
    .expect("record break");
    log.record_with_oracle(
        "fix",
        &broken,
        &fixed,
        fix_patch(),
        true,
        false,
        Some(&oracle),
    )
    .expect("record fix");
    log.record_with_oracle(
        "noop",
        &fixed,
        &fixed,
        fix_patch(),
        false,
        false,
        Some(&oracle),
    )
    .expect("record noop");
    (log, fixed)
}

fn tamper_header(text: &str, field: &str, value: Value) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut header: Value = serde_json::from_str(&lines[0]).expect("header");
    header[field] = value;
    lines[0] = header.to_string();
    lines.join("\n") + "\n"
}

#[test]
fn shard_round_trip_preserves_chain_and_replay() {
    let (log, fixed) = sample_log();
    let shards = log.clone().into_shards(2).expect("split");
    assert_eq!(shards.shard_count(), 2);
    assert_eq!(shards.len(), 3);
    assert_eq!(shards.head(), log.head());
    shards.verify_chain().expect("shard chain");

    let texts = shards.to_texts();
    assert_eq!(texts.len(), 2);
    assert_eq!(texts[0].lines().count(), 3, "header + 2 条");
    assert_eq!(texts[1].lines().count(), 2, "header + 1 条");
    for text in &texts {
        let mut lines = text.lines();
        let header: Value = serde_json::from_str(lines.next().expect("header")).expect("json");
        assert_eq!(header["kind"], json!("wspace-shard"));
        for line in lines {
            assert!(!line.contains("\"oracle\":"), "分片记录不得内嵌验证集");
            assert!(
                line.contains("\"oracle_index\":"),
                "分片记录应引用 oracle 表"
            );
        }
    }

    let parsed = ShardedLog::from_texts(&texts).expect("parse shards");
    assert_eq!(parsed, shards);
    parsed.verify_chain().expect("parsed chain");
    assert_eq!(parsed.to_texts(), texts);
    let (replay, final_net) = parsed.replay_net(&xor_net()).expect("replay");
    assert_eq!(replay.entries, 3);
    assert_eq!(replay.applied, 2);
    assert_eq!(replay.skipped, 1);
    assert_eq!(replay.final_checksum, checksum(&fixed));
    assert_eq!(final_net, fixed);
    assert_eq!(replay, log.replay(&xor_net()).expect("single log replay"));
}

#[test]
fn oracle_table_holds_each_oracle_once_per_shard() {
    let (log, _) = sample_log();
    let shards = log.into_shards(10).expect("split");
    assert_eq!(shards.shard_count(), 1);
    let shard = &shards.shards()[0];
    assert_eq!(shard.header().oracles.len(), 1, "同一验证集只存一次");
    assert_eq!(shard.header().digest(), shard.header().digest());
    assert!(shard.entries()[0].oracle_ref.is_none(), "首条内嵌");
    assert_eq!(shard.entries()[1].oracle_ref, Some(0));
    assert_eq!(shard.entries()[1].oracle, shard.entries()[0].oracle);
}

#[test]
fn empty_log_and_zero_size() {
    let log = AuditLog::new();
    let shards = log.into_shards(4).expect("split empty");
    assert_eq!(shards.shard_count(), 0);
    assert!(shards.is_empty());
    assert_eq!(shards.head(), trit_wspace::audit::GENESIS);
    assert!(shards.to_texts().is_empty());
    let parsed = ShardedLog::from_texts(&[]).expect("parse empty");
    parsed.verify_chain().expect("empty chain");
    assert_eq!(
        parsed.replay(&xor_net()).expect("replay").final_checksum,
        checksum(&xor_net())
    );

    let (log, _) = sample_log();
    assert!(log.into_shards(0).is_err());
}

#[test]
fn shard_header_tampering_is_detected() {
    let (log, _) = sample_log();
    let texts = log.into_shards(2).expect("split").to_texts();

    let head = tamper_header(&texts[0], "head", json!("0000000000000000"));
    assert!(
        ShardedLog::from_texts(&[head, texts[1].clone()])
            .expect("parse")
            .verify_chain()
            .is_err()
    );

    let prev = tamper_header(&texts[1], "prev", json!("0000000000000000"));
    assert!(
        ShardedLog::from_texts(&[texts[0].clone(), prev])
            .expect("parse")
            .verify_chain()
            .is_err()
    );

    let count = tamper_header(&texts[0], "count", json!(1));
    assert!(ShardedLog::from_texts(&[count, texts[1].clone()]).is_err());

    let first = tamper_header(&texts[1], "first_seq", json!(9));
    assert!(
        ShardedLog::from_texts(&[texts[0].clone(), first])
            .expect("parse")
            .verify_chain()
            .is_err()
    );
}

#[test]
fn oracle_table_tampering_is_detected() {
    let (log, _) = sample_log();
    let texts = log.into_shards(10).expect("split").to_texts();
    let mut lines: Vec<String> = texts[0].lines().map(str::to_string).collect();
    let mut header: Value = serde_json::from_str(&lines[0]).expect("header");
    header["oracles"][0]["outputs"][0] = json!("+");
    lines[0] = header.to_string();
    let tampered = lines.join("\n") + "\n";
    let error = ShardedLog::from_texts(&[tampered]).expect_err("摘要应变化");
    assert!(error.to_string().contains("摘要"), "{error}");

    let mut lines: Vec<String> = texts[0].lines().map(str::to_string).collect();
    let entry = lines[1].replace("\"oracle_index\":0", "\"oracle_index\":9");
    assert_ne!(entry, lines[1]);
    lines[1] = entry;
    let tampered = lines.join("\n") + "\n";
    assert!(ShardedLog::from_texts(&[tampered]).is_err());
}

#[test]
fn cursor_streams_shards_and_checks_links() {
    let (log, _) = sample_log();
    let texts = log.clone().into_shards(2).expect("split").to_texts();
    let mut cursor = ShardCursor::new();
    for text in &texts {
        cursor.push(text).expect("shard");
    }
    assert_eq!(cursor.shard_count(), 2);
    assert_eq!(cursor.entries(), 3);
    assert_eq!(cursor.head(), log.head());

    let mut cursor = ShardCursor::new();
    assert!(cursor.push(&texts[1]).is_err(), "序号不对应拒绝");

    let mut cursor = ShardCursor::new();
    cursor.push(&texts[0]).expect("first");
    let broken = tamper_header(&texts[1], "prev", json!("0000000000000000"));
    assert!(cursor.push(&broken).is_err(), "父链头不符应拒绝");
}

#[test]
fn shard_replay_streams_to_same_result() {
    let (log, fixed) = sample_log();
    let texts = log.clone().into_shards(2).expect("split").to_texts();
    let mut replay = ShardReplay::new(&xor_net());
    for text in &texts {
        replay.push(text).expect("replay shard");
    }
    assert_eq!(replay.shard_count(), 2);
    assert_eq!(replay.entries(), 3);
    assert_eq!(replay.head(), log.head());
    let (report, net) = replay.finish();
    assert_eq!(report.entries, 3);
    assert_eq!(report.applied, 2);
    assert_eq!(report.skipped, 1);
    assert_eq!(report.final_checksum, checksum(&fixed));
    assert_eq!(net, fixed);
}

#[test]
fn manifest_matches_and_detects_wrong_text() {
    let (log, _) = sample_log();
    let shards = log.clone().into_shards(2).expect("split");
    let texts = shards.to_texts();
    let manifest = ShardManifest::of(&shards);
    assert_eq!(manifest.entries, 3);
    assert_eq!(manifest.head, log.head());
    assert_eq!(manifest.shards.len(), 2);
    manifest.verify_texts(&texts).expect("manifest match");
    let value = manifest.to_value();
    assert_eq!(ShardManifest::from_value(&value).expect("parse"), manifest);

    let mut tampered = texts.clone();
    tampered[1].push(' ');
    assert!(manifest.verify_texts(&tampered).is_err(), "摘要应变化");
    let swapped = vec![texts[1].clone(), texts[0].clone()];
    assert!(manifest.verify_texts(&swapped).is_err(), "错序应拒绝");
    assert!(manifest.verify_texts(&texts[..1]).is_err(), "丢片应拒绝");

    let mut value = manifest.to_value();
    value["shards"][0]["shard"] = json!(1);
    assert!(ShardManifest::from_value(&value).is_err(), "序号必须连续");

    let mut wrong = manifest.clone();
    wrong.entries = 99;
    assert!(wrong.verify_texts(&texts).is_err(), "总条数应核对");
    let mut wrong = manifest.clone();
    wrong.head = 0;
    assert!(wrong.verify_texts(&texts).is_err(), "链头应核对");
}

#[test]
fn dropped_or_reordered_shard_is_detected() {
    let (log, _) = sample_log();
    let texts = log.into_shards(2).expect("split").to_texts();
    let error = ShardedLog::from_texts(&[texts[1].clone()]).expect_err("缺首片");
    assert!(error.to_string().contains("分片"), "{error}");
    let error =
        ShardedLog::from_texts(&[texts[1].clone(), texts[0].clone()]).expect_err("分片乱序");
    assert!(error.to_string().contains("分片"), "{error}");
}
