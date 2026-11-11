//! SQLite 三值语义对拍：本 crate 的 `CHECK` / `NOT NULL` / `UNIQUE` 判定必须与真 SQLite 逐条一致。
//!
//! 用 `rusqlite` 的 bundled SQLite（三平台同一版本），不是自己实现的“参考”。

use rusqlite::Connection;
use trit_clean::{CmpOp, Column, Rule, Table, Ty, Value, eval_row_rule, eval_unique, evaluate};
use trit_core::value::Ternary;

fn sqlite_state(conn: &Connection, expr: &str) -> Ternary {
    let value: Option<i64> = conn
        .query_row(&format!("SELECT ({expr}) FROM t"), [], |row| row.get(0))
        .unwrap_or_else(|e| panic!("query {expr}: {e}"));
    match value {
        None => Ternary::Unknown,
        Some(0) => Ternary::False,
        Some(1) => Ternary::True,
        Some(other) => panic!("SQLite 三值结果不允许是 {other}"),
    }
}

fn int_table(rows: &[Option<i64>]) -> Table {
    let mut table = Table::new(vec![Column::new("a", Ty::Int)]);
    for value in rows {
        table
            .push_row(vec![value.map_or(Value::Null, Value::Int)])
            .unwrap();
    }
    table
}

#[test]
fn check_three_valued_matches_sqlite_for_every_operator() {
    let cases: [(&str, CmpOp); 6] = [
        ("=", CmpOp::Eq),
        ("<>", CmpOp::Ne),
        ("<", CmpOp::Lt),
        ("<=", CmpOp::Le),
        (">", CmpOp::Gt),
        (">=", CmpOp::Ge),
    ];
    for (sql_op, op) in cases {
        for a in [None, Some(-1), Some(0), Some(1), Some(7)] {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch("CREATE TABLE t(a INTEGER);").unwrap();
            conn.execute("INSERT INTO t(a) VALUES (?1)", [a]).unwrap();
            let expected = sqlite_state(&conn, &format!("a {sql_op} 0"));

            let table = int_table(&[a]);
            let rule = Rule::Check {
                col: 0,
                op,
                lit: Value::Int(0),
            };
            let got = eval_row_rule(&rule, &table.rows[0]);
            assert_eq!(
                got, expected,
                "a={a:?} sql_op={sql_op}: our={got:?} sqlite={expected:?}"
            );
        }
    }
}

#[test]
fn text_comparison_matches_sqlite_binary_collation() {
    for a in [None, Some("a"), Some("m"), Some("z")] {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t(s TEXT);").unwrap();
        conn.execute("INSERT INTO t(s) VALUES (?1)", [a]).unwrap();
        let expected = sqlite_state(&conn, "s < 'm'");

        let mut table = Table::new(vec![Column::new("s", Ty::Text)]);
        table
            .push_row(vec![a.map_or(Value::Null, |s| Value::Text(s.to_string()))])
            .unwrap();
        let rule = Rule::Check {
            col: 0,
            op: CmpOp::Lt,
            lit: Value::Text("m".into()),
        };
        assert_eq!(eval_row_rule(&rule, &table.rows[0]), expected, "s={a:?}");
    }
}

#[test]
fn not_null_matches_sqlite() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE t(a INTEGER NOT NULL);")
        .unwrap();
    let rejected = conn.execute("INSERT INTO t(a) VALUES (NULL)", []).is_err();
    assert!(rejected, "SQLite 应拒绝 NULL");

    let rule = Rule::NotNull { col: 0 };
    let table = int_table(&[None]);
    assert_eq!(eval_row_rule(&rule, &table.rows[0]), Ternary::False);
    assert_eq!(
        eval_row_rule(&rule, &[Value::Int(1)]),
        Ternary::True,
        "非空值不应被判 False"
    );
}

#[test]
fn check_unknown_passes_in_sqlite_and_ours() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE t(a INTEGER CHECK (a >= 0));")
        .unwrap();
    assert!(
        conn.execute("INSERT INTO t(a) VALUES (NULL)", []).is_ok(),
        "SQLite 的 CHECK 对 UNKNOWN 放行"
    );
    assert!(
        conn.execute("INSERT INTO t(a) VALUES (-1)", []).is_err(),
        "SQLite 的 CHECK 对 False 拒绝"
    );

    let table = int_table(&[None, Some(-1)]);
    let rule = Rule::Check {
        col: 0,
        op: CmpOp::Ge,
        lit: Value::Int(0),
    };
    let report = evaluate(&table, std::slice::from_ref(&rule));
    let no: Vec<usize> = report
        .rows
        .iter()
        .filter(|n| n.state == Ternary::False)
        .map(|n| n.row)
        .collect();
    assert_eq!(no, vec![1], "只有 -1 这一行算违反，NULL 是 UNKNOWN");
}

#[test]
fn unique_null_distinctness_matches_sqlite() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE t(x INTEGER UNIQUE);")
        .unwrap();
    assert!(
        conn.execute("INSERT INTO t(x) VALUES (NULL)", []).is_ok(),
        "第一条 NULL 应允许"
    );
    assert!(
        conn.execute("INSERT INTO t(x) VALUES (NULL)", []).is_ok(),
        "第二条 NULL 也应允许（NULL 互不冲突）"
    );
    assert!(
        conn.execute("INSERT INTO t(x) VALUES (1)", []).is_ok(),
        "第一条 1 应允许"
    );
    assert!(
        conn.execute("INSERT INTO t(x) VALUES (1)", []).is_err(),
        "重复的 1 应被拒绝"
    );

    let nulls = int_table(&[None, None]);
    assert_eq!(eval_unique(&nulls, 0), Ternary::True);
    let dup = int_table(&[Some(1), Some(1)]);
    assert_eq!(eval_unique(&dup, 0), Ternary::False);
}
