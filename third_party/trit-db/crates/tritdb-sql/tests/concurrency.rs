//! Concurrency: unique constraints and counters must hold under parallel writers.
//!
//! The wire server shares one `SqlEngine` behind a mutex; these tests exercise
//! the same shape directly so the check-then-write window is covered without
//! a socket.

use std::sync::{Arc, Mutex};

use tritdb_sql::SqlEngine;
use tritdb_testkit::TempDir;

#[test]
fn concurrent_inserts_keep_unique_constraints() {
    let dir = TempDir::new("sql-race-unique");
    let db = dir.join("sql.db");
    let engine = Arc::new(Mutex::new(SqlEngine::open(&db).expect("open")));
    engine
        .lock()
        .expect("lock")
        .execute_script("CREATE TABLE t (id INT NOT NULL UNIQUE, name TEXT);")
        .expect("create");

    let mut handles = Vec::new();
    for thread in 0..8u32 {
        let engine = Arc::clone(&engine);
        handles.push(std::thread::spawn(move || {
            let mut guard = engine.lock().expect("lock");
            guard
                .execute_script(&format!("INSERT INTO t VALUES (1, 't{thread}');"))
                .is_ok()
        }));
    }
    let successes = handles
        .into_iter()
        .map(|handle| handle.join().expect("join"))
        .filter(|ok| *ok)
        .count();
    assert_eq!(
        successes, 1,
        "exactly one concurrent insert may win the unique key"
    );

    let mut guard = engine.lock().expect("lock");
    let results = guard
        .execute_script("SELECT COUNT(*) FROM t;")
        .expect("count");
    let rows = match &results[0] {
        tritdb_sql::QueryResult::Rows { rows, .. } => rows,
        other => panic!("expected rows, got {other:?}"),
    };
    assert_eq!(
        rows[0][0],
        tritdb_exec::Value::Int(1),
        "the table must hold exactly one row"
    );
}

#[test]
fn concurrent_updates_do_not_lose_increments() {
    let dir = TempDir::new("sql-race-update");
    let db = dir.join("sql.db");
    let engine = Arc::new(Mutex::new(SqlEngine::open(&db).expect("open")));
    engine
        .lock()
        .expect("lock")
        .execute_script(
            "CREATE TABLE counters (id INT NOT NULL UNIQUE, n INT NOT NULL); \
             INSERT INTO counters VALUES (1, 0);",
        )
        .expect("seed");

    let mut handles = Vec::new();
    for _ in 0..8 {
        let engine = Arc::clone(&engine);
        handles.push(std::thread::spawn(move || {
            let mut guard = engine.lock().expect("lock");
            guard
                .execute_script("UPDATE counters SET n = n + 1 WHERE id = 1;")
                .expect("update");
        }));
    }
    for handle in handles {
        handle.join().expect("join");
    }

    let mut guard = engine.lock().expect("lock");
    let results = guard
        .execute_script("SELECT n FROM counters WHERE id = 1;")
        .expect("read");
    let rows = match &results[0] {
        tritdb_sql::QueryResult::Rows { rows, .. } => rows,
        other => panic!("expected rows, got {other:?}"),
    };
    assert_eq!(
        rows[0][0],
        tritdb_exec::Value::Int(8),
        "every update must be applied exactly once"
    );
}

#[test]
fn concurrent_reads_during_writes_see_consistent_tables() {
    let dir = TempDir::new("sql-race-mixed");
    let db = dir.join("sql.db");
    let engine = Arc::new(Mutex::new(SqlEngine::open(&db).expect("open")));
    engine
        .lock()
        .expect("lock")
        .execute_script("CREATE TABLE mixed (id INT NOT NULL UNIQUE);")
        .expect("create");

    let writers: Vec<_> = (0..4u32)
        .map(|writer| {
            let engine = Arc::clone(&engine);
            std::thread::spawn(move || {
                for row in 0..25u32 {
                    let id = writer * 25 + row;
                    let mut guard = engine.lock().expect("lock");
                    guard
                        .execute_script(&format!("INSERT INTO mixed VALUES ({id});"))
                        .expect("insert");
                }
            })
        })
        .collect();
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let engine = Arc::clone(&engine);
            std::thread::spawn(move || {
                for _ in 0..25 {
                    let mut guard = engine.lock().expect("lock");
                    let results = guard
                        .execute_script("SELECT COUNT(*) FROM mixed;")
                        .expect("count");
                    let rows = match &results[0] {
                        tritdb_sql::QueryResult::Rows { rows, .. } => rows,
                        other => panic!("expected rows, got {other:?}"),
                    };
                    let count = match rows[0][0] {
                        tritdb_exec::Value::Int(count) => count,
                        ref other => panic!("expected integer count, got {other:?}"),
                    };
                    assert!(
                        (0..=100).contains(&count),
                        "count {count} must stay within the inserted range"
                    );
                }
            })
        })
        .collect();
    for handle in writers {
        handle.join().expect("writer join");
    }
    for handle in readers {
        handle.join().expect("reader join");
    }

    let mut guard = engine.lock().expect("lock");
    let results = guard
        .execute_script("SELECT COUNT(*) FROM mixed;")
        .expect("count");
    let rows = match &results[0] {
        tritdb_sql::QueryResult::Rows { rows, .. } => rows,
        other => panic!("expected rows, got {other:?}"),
    };
    assert_eq!(
        rows[0][0],
        tritdb_exec::Value::Int(100),
        "every insert must be visible exactly once"
    );
}

#[test]
fn racing_create_table_has_one_winner() {
    let dir = TempDir::new("sql-race-create");
    let db = dir.join("sql.db");
    let engine = Arc::new(Mutex::new(SqlEngine::open(&db).expect("open")));

    let mut handles = Vec::new();
    for _ in 0..8 {
        let engine = Arc::clone(&engine);
        handles.push(std::thread::spawn(move || {
            let mut guard = engine.lock().expect("lock");
            guard
                .execute_script("CREATE TABLE duplex (id INT NOT NULL UNIQUE);")
                .is_ok()
        }));
    }
    let successes = handles
        .into_iter()
        .map(|handle| handle.join().expect("join"))
        .filter(|ok| *ok)
        .count();
    assert_eq!(
        successes, 1,
        "exactly one concurrent CREATE TABLE may win the name"
    );
}
