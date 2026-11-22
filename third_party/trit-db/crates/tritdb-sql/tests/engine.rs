use tritdb_exec::Value;
use tritdb_sql::{JoinStrategy, NameKind, QueryResult, SqlEngine, SqlError};
use tritdb_testkit::TempDir;

fn as_rows(result: &QueryResult) -> (&[String], &[Vec<Value>]) {
    match result {
        QueryResult::Rows { columns, rows } => (columns, rows),
        QueryResult::Message(message) => panic!("expected rows, got message: {message}"),
    }
}

fn message(result: &QueryResult) -> &str {
    match result {
        QueryResult::Message(message) => message,
        QueryResult::Rows { .. } => panic!("expected message, got rows"),
    }
}

#[test]
fn create_insert_select_and_reopen() {
    let dir = TempDir::new("sql-persist");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    let results = engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, name TEXT, flag BOOL); \
             INSERT INTO t VALUES (1, 'a', TRUE), (2, NULL, UNKNOWN), (3, 'c', FALSE); \
             SELECT id, name, flag FROM t WHERE flag IS NULL OR id >= 2 ORDER BY id DESC LIMIT 2;",
        )
        .expect("execute");
    assert_eq!(message(&results[0]), "created table t");
    assert_eq!(message(&results[1]), "inserted 3 row(s) into t");
    let (columns, rows) = as_rows(&results[2]);
    assert_eq!(columns, ["id", "name", "flag"]);
    assert_eq!(
        rows,
        [
            vec![
                Value::Int(3),
                Value::Text("c".to_string()),
                Value::Bool(false)
            ],
            vec![Value::Int(2), Value::Null, Value::Null],
        ]
    );
    drop(engine);

    let mut reopened = SqlEngine::open(&db).expect("reopen");
    let results = reopened
        .execute_script("SELECT id, name, flag FROM t ORDER BY id;")
        .expect("select");
    let (columns, rows) = as_rows(&results[0]);
    assert_eq!(columns, ["id", "name", "flag"]);
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[0],
        vec![
            Value::Int(1),
            Value::Text("a".to_string()),
            Value::Bool(true)
        ]
    );
}

#[test]
fn alter_table_add_column_migrates_old_rows_and_invalidates_stats() {
    let dir = TempDir::new("sql-alter-add-column");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    let seeded = engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL); \
             INSERT INTO t VALUES (1), (2), (3); \
             ANALYZE; \
             EXPLAIN SELECT id FROM t;",
        )
        .expect("seed");
    let plan_text = |result: &QueryResult| {
        let (_, rows) = as_rows(result);
        rows.iter()
            .map(|row| match &row[0] {
                Value::Text(text) => text.clone(),
                other => panic!("unexpected plan value: {other:?}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(
        plan_text(&seeded[3]).contains("est rows"),
        "expected stats-backed estimates before migration"
    );

    let altered = engine
        .execute_script("ALTER TABLE t ADD COLUMN note TEXT;")
        .expect("alter");
    assert_eq!(message(&altered[0]), "added column note to table t");
    assert!(
        !plan_text(
            &engine
                .execute_script("EXPLAIN SELECT id FROM t;")
                .expect("explain")[0]
        )
        .contains("est rows"),
        "migration must invalidate stored stats"
    );

    let migrated = engine
        .execute_script("SELECT * FROM t ORDER BY id;")
        .expect("select migrated");
    let (columns, rows) = as_rows(&migrated[0]);
    assert_eq!(columns, ["id", "note"]);
    assert_eq!(
        rows,
        [
            vec![Value::Int(1), Value::Null],
            vec![Value::Int(2), Value::Null],
            vec![Value::Int(3), Value::Null],
        ]
    );

    let inserted = engine
        .execute_script("INSERT INTO t (id, note) VALUES (4, 'd');")
        .expect("insert after alter");
    assert_eq!(message(&inserted[0]), "inserted 1 row(s) into t");

    let duplicate = engine
        .execute_script("ALTER TABLE t ADD COLUMN IF NOT EXISTS note TEXT;")
        .expect("idempotent alter");
    assert_eq!(
        message(&duplicate[0]),
        "column note already exists in table t"
    );
    let duplicate_error = engine
        .execute_script("ALTER TABLE t ADD COLUMN note TEXT;")
        .expect_err("duplicate without IF NOT EXISTS");
    assert!(duplicate_error
        .to_string()
        .contains("duplicate column: note"));
    let not_null = engine
        .execute_script("ALTER TABLE t ADD COLUMN required INT NOT NULL;")
        .expect_err("NOT NULL must be rejected");
    assert!(not_null.to_string().contains("NOT NULL"));
    let unknown = engine
        .execute_script("ALTER TABLE missing ADD COLUMN note TEXT;")
        .expect_err("unknown table");
    assert!(unknown.to_string().contains("unknown table: missing"));

    let after_failures = engine
        .execute_script("SELECT * FROM t ORDER BY id;")
        .expect("select after rejected migrations");
    let (columns, rows) = as_rows(&after_failures[0]);
    assert_eq!(columns, ["id", "note"]);
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[3], vec![Value::Int(4), Value::Text("d".to_string())]);

    let empty = engine
        .execute_script(
            "CREATE TABLE empty (id INT); \
             ALTER TABLE empty ADD COLUMN note TEXT; \
             SELECT * FROM empty;",
        )
        .expect("migrate empty table");
    assert_eq!(message(&empty[1]), "added column note to table empty");
    let (columns, rows) = as_rows(&empty[2]);
    assert_eq!(columns, ["id", "note"]);
    assert!(rows.is_empty());

    let empty_not_null = engine
        .execute_script(
            "CREATE TABLE empty_required (id INT); \
             ALTER TABLE empty_required ADD COLUMN required INT NOT NULL;",
        )
        .expect_err("NOT NULL must be rejected even on an empty table");
    assert!(empty_not_null.to_string().contains("NOT NULL"));

    drop(engine);
    let mut reopened = SqlEngine::open(&db).expect("reopen");
    assert!(
        !plan_text(
            &reopened
                .execute_script("EXPLAIN SELECT id FROM t;")
                .expect("explain")[0]
        )
        .contains("est rows"),
        "migration must leave no stale stats after reopen"
    );
    let persisted = reopened
        .execute_script("SELECT * FROM t ORDER BY id;")
        .expect("select after reopen");
    let (columns, rows) = as_rows(&persisted[0]);
    assert_eq!(columns, ["id", "note"]);
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0], vec![Value::Int(1), Value::Null]);
    assert_eq!(rows[3], vec![Value::Int(4), Value::Text("d".to_string())]);
}

#[test]
fn three_valued_where_only_keeps_true() {
    let dir = TempDir::new("sql-3vl");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT, flag BOOL); \
             INSERT INTO t VALUES (1, TRUE), (2, FALSE), (3, NULL);",
        )
        .expect("seed");

    let true_rows = engine
        .execute_script("SELECT id FROM t WHERE flag ORDER BY id;")
        .expect("where true");
    assert_eq!(as_rows(&true_rows[0]).1, [vec![Value::Int(1)]]);

    let false_rows = engine
        .execute_script("SELECT id FROM t WHERE NOT flag ORDER BY id;")
        .expect("where not");
    assert_eq!(as_rows(&false_rows[0]).1, [vec![Value::Int(2)]]);

    let null_rows = engine
        .execute_script("SELECT id FROM t WHERE flag IS NULL ORDER BY id;")
        .expect("where null");
    assert_eq!(as_rows(&null_rows[0]).1, [vec![Value::Int(3)]]);

    let unknown_rows = engine
        .execute_script("SELECT id FROM t WHERE flag IS UNKNOWN ORDER BY id;")
        .expect("where unknown");
    assert_eq!(as_rows(&unknown_rows[0]).1, [vec![Value::Int(3)]]);

    let and_rows = engine
        .execute_script("SELECT id FROM t WHERE flag AND flag IS NULL ORDER BY id;")
        .expect("where and");
    assert!(as_rows(&and_rows[0]).1.is_empty());
}

#[test]
fn aggregates_ignore_null_and_count_star_counts_rows() {
    let dir = TempDir::new("sql-agg");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, score INT); \
             INSERT INTO t VALUES (1, 10), (2, NULL), (3, 20), (4, NULL);",
        )
        .expect("seed");
    let results = engine
        .execute_script("SELECT COUNT(*), COUNT(score), SUM(score), MIN(score), MAX(score) FROM t;")
        .expect("aggregate");
    let (columns, rows) = as_rows(&results[0]);
    assert_eq!(
        columns,
        [
            "COUNT(*)",
            "COUNT(score)",
            "SUM(score)",
            "MIN(score)",
            "MAX(score)"
        ]
    );
    assert_eq!(
        rows[0],
        [
            Value::Int(4),
            Value::Int(2),
            Value::Int(30),
            Value::Int(10),
            Value::Int(20),
        ]
    );
}

#[test]
fn nulls_sort_first_ascending_and_last_descending() {
    let dir = TempDir::new("sql-order-null");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT, score INT); \
             INSERT INTO t VALUES (1, 2), (2, NULL), (3, 1);",
        )
        .expect("seed");
    let asc = engine
        .execute_script("SELECT id, score FROM t ORDER BY score ASC;")
        .expect("asc");
    assert_eq!(
        as_rows(&asc[0]).1,
        [
            vec![Value::Int(2), Value::Null],
            vec![Value::Int(3), Value::Int(1)],
            vec![Value::Int(1), Value::Int(2)],
        ]
    );
    let desc = engine
        .execute_script("SELECT id, score FROM t ORDER BY score DESC;")
        .expect("desc");
    assert_eq!(
        as_rows(&desc[0]).1,
        [
            vec![Value::Int(1), Value::Int(2)],
            vec![Value::Int(3), Value::Int(1)],
            vec![Value::Int(2), Value::Null],
        ]
    );
}

#[test]
fn delete_and_drop_persist() {
    let dir = TempDir::new("sql-delete");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL); \
             INSERT INTO t VALUES (1), (2), (3); \
             DELETE FROM t WHERE id >= 2;",
        )
        .expect("delete");
    let after_delete = engine.execute_script("SELECT id FROM t;").expect("select");
    assert_eq!(as_rows(&after_delete[0]).1, [vec![Value::Int(1)]]);
    drop(engine);

    let mut reopened = SqlEngine::open(&db).expect("reopen");
    let after_reopen = reopened
        .execute_script("SELECT id FROM t;")
        .expect("select");
    assert_eq!(as_rows(&after_reopen[0]).1, [vec![Value::Int(1)]]);
    let dropped = reopened.execute_script("DROP TABLE t;").expect("drop");
    assert_eq!(message(&dropped[0]), "dropped table t");
    assert!(reopened.execute_script("SELECT id FROM t;").is_err());
}

#[test]
fn not_null_and_type_checks_are_enforced() {
    let dir = TempDir::new("sql-constraints");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    let create = engine
        .execute_script("CREATE TABLE t (id INT NOT NULL, name TEXT);")
        .expect("create");
    assert_eq!(message(&create[0]), "created table t");
    assert!(engine
        .execute_script("INSERT INTO t VALUES (NULL, 'x');")
        .is_err());
    assert!(engine
        .execute_script("INSERT INTO t VALUES ('x', 'y');")
        .is_err());
    assert!(engine
        .execute_script("INSERT INTO t (id) VALUES (1), (1);")
        .is_ok());
}

#[test]
fn group_by_counts_and_aggregates_per_key() {
    let dir = TempDir::new("sql-group");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT, score INT, name TEXT); \
             INSERT INTO t VALUES \
               (1, 10, 'a'), (2, 20, 'a'), (3, NULL, 'b'), \
               (4, 30, 'b'), (5, 10, NULL);",
        )
        .expect("seed");
    let results = engine
        .execute_script(
            "SELECT score, COUNT(*) AS n, SUM(id) AS total \
             FROM t GROUP BY score ORDER BY score;",
        )
        .expect("group");
    let (columns, rows) = as_rows(&results[0]);
    assert_eq!(columns, ["score", "n", "total"]);
    assert_eq!(
        rows,
        [
            vec![Value::Null, Value::Int(1), Value::Int(3)],
            vec![Value::Int(10), Value::Int(2), Value::Int(6)],
            vec![Value::Int(20), Value::Int(1), Value::Int(2)],
            vec![Value::Int(30), Value::Int(1), Value::Int(4)],
        ]
    );
}

#[test]
fn group_by_having_and_order_by_aggregate_alias() {
    let dir = TempDir::new("sql-group-having");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT, score INT, name TEXT); \
             INSERT INTO t VALUES \
               (1, 10, 'a'), (2, 20, 'a'), (3, NULL, 'b'), \
               (4, 30, 'b'), (5, 10, NULL);",
        )
        .expect("seed");
    let results = engine
        .execute_script(
            "SELECT name, COUNT(*) AS n, SUM(id) AS total \
             FROM t GROUP BY name HAVING COUNT(*) >= 2 \
             ORDER BY n DESC, name ASC;",
        )
        .expect("having");
    assert_eq!(
        as_rows(&results[0]).1,
        [
            vec![Value::Text("a".to_string()), Value::Int(2), Value::Int(3),],
            vec![Value::Text("b".to_string()), Value::Int(2), Value::Int(7),],
        ]
    );

    let scalar_having = engine
        .execute_script(
            "SELECT name, COUNT(*) FROM t GROUP BY name \
             HAVING name IS NOT NULL ORDER BY name;",
        )
        .expect("having column");
    assert_eq!(
        as_rows(&scalar_having[0]).1,
        [
            vec![Value::Text("a".to_string()), Value::Int(2)],
            vec![Value::Text("b".to_string()), Value::Int(2)],
        ]
    );
}

#[test]
fn group_by_without_aggregate_and_aggregate_order() {
    let dir = TempDir::new("sql-group-order");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT, score INT, name TEXT); \
             INSERT INTO t VALUES \
               (1, 10, 'a'), (2, 20, 'a'), (3, NULL, 'b'), \
               (4, 30, 'b'), (5, 10, NULL);",
        )
        .expect("seed");

    let groups = engine
        .execute_script("SELECT name FROM t GROUP BY name ORDER BY name;")
        .expect("group without aggregate");
    assert_eq!(
        as_rows(&groups[0]).1,
        [
            vec![Value::Null],
            vec![Value::Text("a".to_string())],
            vec![Value::Text("b".to_string())],
        ]
    );

    let ordered = engine
        .execute_script(
            "SELECT score AS s, COUNT(*) AS n FROM t \
             GROUP BY score ORDER BY n DESC, s ASC LIMIT 2;",
        )
        .expect("aggregate order");
    assert_eq!(
        as_rows(&ordered[0]).1,
        [
            vec![Value::Int(10), Value::Int(2)],
            vec![Value::Null, Value::Int(1)],
        ]
    );
}

#[test]
fn group_by_empty_input_and_global_aggregate() {
    let dir = TempDir::new("sql-group-empty");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script("CREATE TABLE t (x INT);")
        .expect("create");
    let empty = engine
        .execute_script("SELECT COUNT(*), SUM(x) FROM t;")
        .expect("global aggregate");
    assert_eq!(as_rows(&empty[0]).1, [vec![Value::Int(0), Value::Null]]);

    let no_groups = engine
        .execute_script("SELECT x, COUNT(*) FROM t GROUP BY x;")
        .expect("empty groups");
    assert!(as_rows(&no_groups[0]).1.is_empty());
}

#[test]
fn group_by_rejects_ungrouped_columns_and_bad_having() {
    let dir = TempDir::new("sql-group-errors");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script("CREATE TABLE t (id INT, score INT); INSERT INTO t VALUES (1, 10);")
        .expect("seed");

    let error = engine
        .execute_script("SELECT id, COUNT(*) FROM t GROUP BY score;")
        .expect_err("ungrouped column")
        .to_string();
    assert!(error.contains("must appear in GROUP BY"), "{error}");

    let error = engine
        .execute_script("SELECT id FROM t GROUP BY score + 1;")
        .expect_err("group expression")
        .to_string();
    assert!(
        error.contains("GROUP BY only supports column references"),
        "{error}"
    );

    let error = engine
        .execute_script("SELECT id FROM t HAVING id > 1;")
        .expect_err("having without group")
        .to_string();
    assert!(error.contains("HAVING requires GROUP BY"), "{error}");

    let error = engine
        .execute_script("SELECT * FROM t GROUP BY score;")
        .expect_err("wildcard with group")
        .to_string();
    assert!(error.contains("SELECT * with GROUP BY"), "{error}");
}

#[test]
fn inner_join_matches_rows_and_qualified_columns() {
    let dir = TempDir::new("sql-join");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE users (id INT, name TEXT, dept INT); \
             CREATE TABLE depts (id INT, name TEXT); \
             INSERT INTO users VALUES (1, 'a', 10), (2, 'b', 20), (3, 'c', NULL), (4, 'd', 10); \
             INSERT INTO depts VALUES (10, 'eng'), (20, 'sales'), (30, 'ops');",
        )
        .expect("seed");
    let results = engine
        .execute_script(
            "SELECT u.id, u.name AS user_name, d.name AS dept_name \
             FROM users AS u JOIN depts AS d ON u.dept = d.id \
             ORDER BY u.id;",
        )
        .expect("join");
    let (columns, rows) = as_rows(&results[0]);
    assert_eq!(columns, ["id", "user_name", "dept_name"]);
    assert_eq!(
        rows,
        [
            vec![
                Value::Int(1),
                Value::Text("a".to_string()),
                Value::Text("eng".to_string()),
            ],
            vec![
                Value::Int(2),
                Value::Text("b".to_string()),
                Value::Text("sales".to_string()),
            ],
            vec![
                Value::Int(4),
                Value::Text("d".to_string()),
                Value::Text("eng".to_string()),
            ],
        ]
    );
}

#[test]
fn chained_join_matches_three_tables() {
    let dir = TempDir::new("sql-join-chain");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE a (id INT NOT NULL, name TEXT); \
             CREATE TABLE b (id INT NOT NULL, aid INT, label TEXT); \
             CREATE TABLE c (id INT, bid INT, tag TEXT); \
             INSERT INTO a VALUES (1, 'one'), (2, 'two'); \
             INSERT INTO b VALUES (10, 1, 'x'), (20, 2, 'y'), (30, 2, 'z'); \
             INSERT INTO c VALUES (100, 10, 'p'), (200, 20, 'q');",
        )
        .expect("seed");

    let results = engine
        .execute_script(
            "SELECT x.name, y.label, z.tag FROM a AS x \
             JOIN b AS y ON x.id = y.aid \
             JOIN c AS z ON y.id = z.bid \
             ORDER BY x.name;",
        )
        .expect("chained join");
    let (columns, rows) = as_rows(&results[0]);
    assert_eq!(columns, ["name", "label", "tag"]);
    assert_eq!(
        rows,
        [
            vec![
                Value::Text("one".to_string()),
                Value::Text("x".to_string()),
                Value::Text("p".to_string()),
            ],
            vec![
                Value::Text("two".to_string()),
                Value::Text("y".to_string()),
                Value::Text("q".to_string()),
            ],
        ]
    );

    let filtered = engine
        .execute_script(
            "SELECT x.id FROM a AS x JOIN b AS y ON x.id = y.aid \
             JOIN c AS z ON y.id = z.bid AND x.id = 1;",
        )
        .expect("later on sees earlier tables");
    assert_eq!(as_rows(&filtered[0]).1, [vec![Value::Int(1)]]);
}

#[test]
fn left_join_preserves_unmatched_left_rows() {
    let dir = TempDir::new("sql-left-join");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE users (id INT, name TEXT, dept INT); \
             CREATE TABLE depts (id INT, name TEXT); \
             INSERT INTO users VALUES (1, 'a', 10), (2, 'b', 20), (3, 'c', NULL), (4, 'd', 10); \
             INSERT INTO depts VALUES (10, 'eng'), (20, 'sales'), (30, 'ops');",
        )
        .expect("seed");

    let results = engine
        .execute_script(
            "SELECT u.id, d.name AS dept_name FROM users AS u \
             LEFT JOIN depts AS d ON u.dept = d.id ORDER BY u.id;",
        )
        .expect("left join");
    let (columns, rows) = as_rows(&results[0]);
    assert_eq!(columns, ["id", "dept_name"]);
    assert_eq!(
        rows,
        [
            vec![Value::Int(1), Value::Text("eng".to_string())],
            vec![Value::Int(2), Value::Text("sales".to_string())],
            vec![Value::Int(3), Value::Null],
            vec![Value::Int(4), Value::Text("eng".to_string())],
        ]
    );

    let outer = engine
        .execute_script(
            "SELECT u.id, d.name FROM users AS u \
             LEFT OUTER JOIN depts AS d ON u.dept = d.id ORDER BY u.id;",
        )
        .expect("left outer join");
    assert_eq!(as_rows(&outer[0]).1, rows);

    let count = engine
        .execute_script(
            "SELECT COUNT(*) FROM users AS u \
             LEFT JOIN depts AS d ON u.dept = d.id;",
        )
        .expect("left join count");
    assert_eq!(as_rows(&count[0]).1, [vec![Value::Int(4)]]);
}

#[test]
fn left_join_where_group_and_empty_right() {
    let dir = TempDir::new("sql-left-join-group");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE users (id INT, name TEXT, dept INT); \
             CREATE TABLE depts (id INT, name TEXT); \
             CREATE TABLE empty (id INT, name TEXT); \
             INSERT INTO users VALUES (1, 'a', 10), (2, 'b', 20), (3, 'c', NULL), (4, 'd', 10); \
             INSERT INTO depts VALUES (10, 'eng'), (20, 'sales'), (30, 'ops');",
        )
        .expect("seed");

    let matched = engine
        .execute_script(
            "SELECT u.id, d.name FROM users AS u \
             LEFT JOIN depts AS d ON u.dept = d.id \
             WHERE d.name = 'eng' ORDER BY u.id;",
        )
        .expect("where matched");
    assert_eq!(
        as_rows(&matched[0]).1,
        [
            vec![Value::Int(1), Value::Text("eng".to_string())],
            vec![Value::Int(4), Value::Text("eng".to_string())],
        ]
    );

    let unmatched = engine
        .execute_script(
            "SELECT u.id FROM users AS u \
             LEFT JOIN depts AS d ON u.dept = d.id \
             WHERE d.name IS NULL ORDER BY u.id;",
        )
        .expect("where unmatched");
    assert_eq!(as_rows(&unmatched[0]).1, [vec![Value::Int(3)]]);

    let grouped = engine
        .execute_script(
            "SELECT d.name, COUNT(*) FROM users AS u \
             LEFT JOIN depts AS d ON u.dept = d.id \
             GROUP BY d.name ORDER BY d.name;",
        )
        .expect("group outer join");
    assert_eq!(
        as_rows(&grouped[0]).1,
        [
            vec![Value::Null, Value::Int(1)],
            vec![Value::Text("eng".to_string()), Value::Int(2)],
            vec![Value::Text("sales".to_string()), Value::Int(1)],
        ]
    );

    let empty = engine
        .execute_script(
            "SELECT COUNT(*), COUNT(e.id) FROM users AS u \
             LEFT JOIN empty AS e ON u.id = e.id;",
        )
        .expect("empty right");
    assert_eq!(as_rows(&empty[0]).1, [vec![Value::Int(4), Value::Int(0)]]);
}

#[test]
fn join_with_group_by_and_where() {
    let dir = TempDir::new("sql-join-group");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE users (id INT, name TEXT, dept INT); \
             CREATE TABLE depts (id INT, name TEXT); \
             INSERT INTO users VALUES (1, 'a', 10), (2, 'b', 20), (3, 'c', NULL), (4, 'd', 10); \
             INSERT INTO depts VALUES (10, 'eng'), (20, 'sales'), (30, 'ops');",
        )
        .expect("seed");
    let results = engine
        .execute_script(
            "SELECT d.name, COUNT(*) AS n FROM users AS u JOIN depts AS d \
             ON u.dept = d.id GROUP BY d.name ORDER BY d.name;",
        )
        .expect("group join");
    assert_eq!(
        as_rows(&results[0]).1,
        [
            vec![Value::Text("eng".to_string()), Value::Int(2)],
            vec![Value::Text("sales".to_string()), Value::Int(1)],
        ]
    );

    let filtered = engine
        .execute_script(
            "SELECT users.id, depts.name FROM users JOIN depts \
             ON users.dept = depts.id WHERE depts.name = 'eng' ORDER BY users.id;",
        )
        .expect("where join");
    assert_eq!(
        as_rows(&filtered[0]).1,
        [
            vec![Value::Int(1), Value::Text("eng".to_string())],
            vec![Value::Int(4), Value::Text("eng".to_string())],
        ]
    );
}

#[test]
fn self_join_supports_compound_on_and_rejects_duplicate_alias() {
    let dir = TempDir::new("sql-self-join");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE users (id INT, name TEXT, dept INT); \
             INSERT INTO users VALUES (1, 'a', 10), (2, 'b', 20), (3, 'c', NULL), (4, 'd', 10);",
        )
        .expect("seed");
    let results = engine
        .execute_script(
            "SELECT u1.id, u2.id FROM users AS u1 JOIN users AS u2 \
             ON u1.dept = u2.dept AND u1.id < u2.id ORDER BY u1.id, u2.id;",
        )
        .expect("self join");
    assert_eq!(as_rows(&results[0]).1, [vec![Value::Int(1), Value::Int(4)]]);

    let error = engine
        .execute_script("SELECT u.id FROM users AS u JOIN users AS u ON u.id = u.id;")
        .expect_err("duplicate alias")
        .to_string();
    assert!(error.contains("duplicate table name or alias"), "{error}");
}

#[test]
fn join_rejects_ambiguous_columns_and_unknown_tables() {
    let dir = TempDir::new("sql-join-errors");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE a (id INT); \
             CREATE TABLE b (id INT); \
             INSERT INTO a VALUES (1); \
             INSERT INTO b VALUES (1);",
        )
        .expect("seed");

    let qualified = engine
        .execute_script("SELECT a.id FROM a ORDER BY a.id;")
        .expect("qualified single table");
    assert_eq!(as_rows(&qualified[0]).1, [vec![Value::Int(1)]]);

    let error = engine
        .execute_script("SELECT id FROM a JOIN b ON a.id = b.id;")
        .expect_err("ambiguous")
        .to_string();
    assert!(error.contains("ambiguous column: id"), "{error}");

    let error = engine
        .execute_script("SELECT a.id FROM a JOIN missing ON a.id = missing.id;")
        .expect_err("unknown table")
        .to_string();
    assert!(error.contains("unknown table: missing"), "{error}");

    let error = engine
        .execute_script("SELECT a.id FROM a JOIN b ON id = 1;")
        .expect_err("unqualified ambiguous on")
        .to_string();
    assert!(error.contains("ambiguous column: id"), "{error}");
}

#[test]
fn aliases_work_in_select_and_order_by() {
    let dir = TempDir::new("sql-alias");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT, score INT); \
             INSERT INTO t VALUES (2, 20), (1, 10), (3, 30);",
        )
        .expect("seed");
    let results = engine
        .execute_script("SELECT id AS k, score + 1 AS s FROM t ORDER BY s DESC LIMIT 2;")
        .expect("alias");
    let (columns, rows) = as_rows(&results[0]);
    assert_eq!(columns, ["k", "s"]);
    assert_eq!(
        rows,
        [
            vec![Value::Int(3), Value::Int(31)],
            vec![Value::Int(2), Value::Int(21)],
        ]
    );
}

#[test]
fn in_and_between_filter_with_three_valued_logic() {
    let dir = TempDir::new("sql-predicates");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, score INT); \
             INSERT INTO t VALUES (1, 10), (2, NULL), (3, 20), (4, 30), (5, NULL), (6, 40);",
        )
        .expect("seed");

    let ids = |engine: &mut SqlEngine, sql: &str| -> Vec<i64> {
        let results = engine.execute_script(sql).expect("select");
        as_rows(&results[0])
            .1
            .iter()
            .map(|row| match row[0] {
                Value::Int(id) => id,
                ref other => panic!("unexpected {other:?}"),
            })
            .collect()
    };

    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM t WHERE id IN (1, 3, 5) ORDER BY id;"
        ),
        [1, 3, 5]
    );
    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM t WHERE id NOT IN (1, 3, 5) ORDER BY id;"
        ),
        [2, 4, 6]
    );
    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM t WHERE score IN (10, 20) ORDER BY id;"
        ),
        [1, 3]
    );
    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM t WHERE score NOT IN (10, 20) ORDER BY id;"
        ),
        [4, 6]
    );
    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM t WHERE score IN (10, NULL) ORDER BY id;"
        ),
        [1]
    );
    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM t WHERE score BETWEEN 10 AND 30 ORDER BY id;"
        ),
        [1, 3, 4]
    );
    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM t WHERE score NOT BETWEEN 10 AND 30 ORDER BY id;"
        ),
        [6]
    );
    assert!(ids(
        &mut engine,
        "SELECT id FROM t WHERE score BETWEEN NULL AND 20 ORDER BY id;"
    )
    .is_empty());
    assert!(ids(
        &mut engine,
        "SELECT id FROM t WHERE score BETWEEN 21 AND NULL ORDER BY id;"
    )
    .is_empty());
}

#[test]
fn like_matches_wildcards_case_and_text_only() {
    let dir = TempDir::new("sql-like");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, name TEXT); \
             INSERT INTO t VALUES (1, 'a'), (2, 'A'), (3, 'abc'), (4, ''), (5, NULL), (6, 'xbc');",
        )
        .expect("seed");

    let ids = |engine: &mut SqlEngine, sql: &str| -> Vec<i64> {
        let results = engine.execute_script(sql).expect("select");
        as_rows(&results[0])
            .1
            .iter()
            .map(|row| match row[0] {
                Value::Int(id) => id,
                ref other => panic!("unexpected {other:?}"),
            })
            .collect()
    };

    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM t WHERE name LIKE 'a%' ORDER BY id;"
        ),
        [1, 2, 3]
    );
    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM t WHERE name LIKE '_' ORDER BY id;"
        ),
        [1, 2]
    );
    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM t WHERE name LIKE 'a_c' ORDER BY id;"
        ),
        [3]
    );
    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM t WHERE name NOT LIKE '%b%' ORDER BY id;"
        ),
        [1, 2, 4]
    );
    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM t WHERE name LIKE '%' ORDER BY id;"
        ),
        [1, 2, 3, 4, 6]
    );
    assert!(engine
        .execute_script("SELECT id FROM t WHERE id LIKE '1';")
        .is_err());
}

#[test]
fn predicates_work_in_projection_having_and_update() {
    let dir = TempDir::new("sql-predicate-context");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, score INT); \
             INSERT INTO t VALUES (1, 10), (2, 20), (3, NULL), (4, 30);",
        )
        .expect("seed");

    let projected = engine
        .execute_script("SELECT id, id IN (1, 3) AS member FROM t ORDER BY id;")
        .expect("project predicate");
    assert_eq!(
        as_rows(&projected[0]).1,
        [
            vec![Value::Int(1), Value::Bool(true)],
            vec![Value::Int(2), Value::Bool(false)],
            vec![Value::Int(3), Value::Bool(true)],
            vec![Value::Int(4), Value::Bool(false)],
        ]
    );

    let grouped = engine
        .execute_script(
            "SELECT score, COUNT(*) FROM t GROUP BY score \
             HAVING COUNT(*) BETWEEN 1 AND 2 ORDER BY score;",
        )
        .expect("having predicate");
    assert_eq!(as_rows(&grouped[0]).1.len(), 4);

    let first = engine
        .execute_script("UPDATE t SET score = score + 10 WHERE id IN (1, 3);")
        .expect("update in");
    assert_eq!(message(&first[0]), "updated 2 row(s) in t");
    let second = engine
        .execute_script("UPDATE t SET score = 0 WHERE score BETWEEN 20 AND 25;")
        .expect("update between");
    assert_eq!(message(&second[0]), "updated 2 row(s) in t");

    let final_rows = engine
        .execute_script("SELECT score FROM t ORDER BY id;")
        .expect("select");
    assert_eq!(
        as_rows(&final_rows[0]).1,
        [
            vec![Value::Int(0)],
            vec![Value::Int(0)],
            vec![Value::Null],
            vec![Value::Int(30)],
        ]
    );
}

#[test]
fn update_sets_values_filters_and_persists() {
    let dir = TempDir::new("sql-update");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    let results = engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, score INT, name TEXT); \
             INSERT INTO t VALUES (1, 10, 'a'), (2, 20, 'b'), (3, 30, 'c'); \
             UPDATE t SET score = score + 5, name = 'x' WHERE id = 2;",
        )
        .expect("update");
    assert_eq!(message(&results[2]), "updated 1 row(s) in t");

    let selected = engine
        .execute_script("SELECT id, score, name FROM t ORDER BY id;")
        .expect("select");
    assert_eq!(
        as_rows(&selected[0]).1,
        [
            vec![Value::Int(1), Value::Int(10), Value::Text("a".to_string())],
            vec![Value::Int(2), Value::Int(25), Value::Text("x".to_string())],
            vec![Value::Int(3), Value::Int(30), Value::Text("c".to_string())],
        ]
    );
    drop(engine);

    let mut reopened = SqlEngine::open(&db).expect("reopen");
    let after = reopened
        .execute_script("SELECT score FROM t WHERE id = 2;")
        .expect("select");
    assert_eq!(as_rows(&after[0]).1, [vec![Value::Int(25)]]);
}

#[test]
fn update_evaluates_all_assignments_against_the_old_row() {
    let dir = TempDir::new("sql-update-old-row");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (a INT, b INT); \
             INSERT INTO t VALUES (1, 2); \
             UPDATE t SET a = b, b = a;",
        )
        .expect("swap");

    let swapped = engine
        .execute_script("SELECT a, b FROM t;")
        .expect("select swap");
    assert_eq!(as_rows(&swapped[0]).1, [vec![Value::Int(2), Value::Int(1)]]);

    engine
        .execute_script("UPDATE t SET a = a + 1, b = a;")
        .expect("old value in second assignment");
    let chained = engine
        .execute_script("SELECT a, b FROM t;")
        .expect("select chained");
    assert_eq!(as_rows(&chained[0]).1, [vec![Value::Int(3), Value::Int(2)]]);
}

#[test]
fn update_checks_null_type_and_target_column() {
    let dir = TempDir::new("sql-update-constraints");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, score INT, name TEXT); \
             INSERT INTO t VALUES (1, 10, 'a'), (2, 20, 'b');",
        )
        .expect("seed");

    engine
        .execute_script("UPDATE t SET name = NULL WHERE id = 1;")
        .expect("nullable assignment");

    let not_null = engine
        .execute_script("UPDATE t SET id = NULL WHERE id = 1;")
        .expect_err("not null")
        .to_string();
    assert!(not_null.contains("t.id is NOT NULL"), "{not_null}");

    assert!(engine
        .execute_script("UPDATE t SET score = 'x' WHERE id = 1;")
        .is_err());
    assert!(engine.execute_script("UPDATE t SET missing = 1;").is_err());
    let duplicate = engine
        .execute_script("UPDATE t SET score = 1, score = 2;")
        .expect_err("duplicate target")
        .to_string();
    assert!(duplicate.contains("duplicate column: score"), "{duplicate}");
    assert!(engine.execute_script("UPDATE t SET t.score = 1;").is_err());

    let unchanged = engine
        .execute_script("SELECT id, score, name FROM t WHERE id = 1;")
        .expect("select");
    assert_eq!(
        as_rows(&unchanged[0]).1,
        [vec![Value::Int(1), Value::Int(10), Value::Null]]
    );
}

#[test]
fn case_projects_filters_and_defaults_to_null() {
    let dir = TempDir::new("sql-case");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    let results = engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, score INT, name TEXT, flag BOOL); \
             INSERT INTO t VALUES \
               (1, 10, 'a', TRUE), (2, NULL, NULL, NULL), (3, 20, 'c', FALSE), \
               (4, 30, 'd', TRUE), (5, NULL, '', NULL); \
             SELECT id, \
               CASE WHEN score IS NULL THEN -1 ELSE score END AS s, \
               CASE WHEN flag THEN 'yes' WHEN flag IS NULL THEN 'unknown' ELSE 'no' END AS f, \
               CASE score WHEN 10 THEN 'ten' WHEN 20 THEN 'twenty' ELSE 'other' END AS label \
             FROM t ORDER BY id;",
        )
        .expect("case projection");
    let (columns, rows) = as_rows(&results[2]);
    assert_eq!(columns, ["id", "s", "f", "label"]);
    assert_eq!(
        rows,
        [
            vec![
                Value::Int(1),
                Value::Int(10),
                Value::Text("yes".to_string()),
                Value::Text("ten".to_string()),
            ],
            vec![
                Value::Int(2),
                Value::Int(-1),
                Value::Text("unknown".to_string()),
                Value::Text("other".to_string()),
            ],
            vec![
                Value::Int(3),
                Value::Int(20),
                Value::Text("no".to_string()),
                Value::Text("twenty".to_string()),
            ],
            vec![
                Value::Int(4),
                Value::Int(30),
                Value::Text("yes".to_string()),
                Value::Text("other".to_string()),
            ],
            vec![
                Value::Int(5),
                Value::Int(-1),
                Value::Text("unknown".to_string()),
                Value::Text("other".to_string()),
            ],
        ]
    );

    let no_else = engine
        .execute_script("SELECT id, CASE WHEN id = 1 THEN 'one' END AS c FROM t ORDER BY id;")
        .expect("no else");
    assert_eq!(
        as_rows(&no_else[0]).1,
        [
            vec![Value::Int(1), Value::Text("one".to_string())],
            vec![Value::Int(2), Value::Null],
            vec![Value::Int(3), Value::Null],
            vec![Value::Int(4), Value::Null],
            vec![Value::Int(5), Value::Null],
        ]
    );

    let filtered = engine
        .execute_script(
            "SELECT id FROM t \
             WHERE CASE WHEN score IS NULL THEN FALSE ELSE score > 10 END ORDER BY id;",
        )
        .expect("case filter");
    assert_eq!(
        as_rows(&filtered[0]).1,
        [vec![Value::Int(3)], vec![Value::Int(4)],]
    );

    let inserted = engine
        .execute_script(
            "INSERT INTO t (id, name, flag) VALUES \
             (6, CASE WHEN TRUE THEN 'x' ELSE 'y' END, CASE WHEN FALSE THEN TRUE END);",
        )
        .expect("case insert");
    assert_eq!(message(&inserted[0]), "inserted 1 row(s) into t");
    let appended = engine
        .execute_script("SELECT name, flag FROM t WHERE id = 6;")
        .expect("inserted row");
    assert_eq!(
        as_rows(&appended[0]).1,
        [vec![Value::Text("x".to_string()), Value::Null]]
    );
}

#[test]
fn case_short_circuits_and_rejects_bad_conditions() {
    let dir = TempDir::new("sql-case-eval");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT, score INT); \
             INSERT INTO t VALUES (1, 10), (2, NULL);",
        )
        .expect("seed");

    let results = engine
        .execute_script("SELECT CASE WHEN TRUE THEN 1 ELSE 1 / 0 END AS c FROM t WHERE id = 1;")
        .expect("untaken else");
    assert_eq!(as_rows(&results[0]).1, [vec![Value::Int(1)]]);

    let results = engine
        .execute_script("SELECT CASE WHEN FALSE THEN 1 / 0 ELSE 2 END AS c FROM t WHERE id = 1;")
        .expect("untaken then");
    assert_eq!(as_rows(&results[0]).1, [vec![Value::Int(2)]]);

    let results = engine
        .execute_script("SELECT CASE WHEN score IS NULL THEN 1 / 0 END AS c FROM t WHERE id = 1;")
        .expect("null condition");
    assert_eq!(as_rows(&results[0]).1, [vec![Value::Null]]);

    let division = engine
        .execute_script("SELECT CASE WHEN score IS NULL THEN 1 / 0 END AS c FROM t WHERE id = 2;")
        .expect_err("chosen branch runs")
        .to_string();
    assert!(division.contains("division by zero"), "{division}");

    let predicate = engine
        .execute_script("SELECT CASE WHEN score THEN 1 END AS c FROM t;")
        .expect_err("non-boolean condition")
        .to_string();
    assert!(
        predicate.contains("predicate must be BOOLEAN"),
        "{predicate}"
    );

    let mismatch = engine
        .execute_script("SELECT CASE score WHEN 'x' THEN 1 ELSE 0 END AS c FROM t;")
        .expect_err("mixed simple-case types")
        .to_string();
    assert!(mismatch.contains("type mismatch"), "{mismatch}");
}

#[test]
fn case_works_with_aggregates_joins_and_update() {
    let dir = TempDir::new("sql-case-query");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, score INT, name TEXT, flag BOOL); \
             INSERT INTO t VALUES \
               (1, 10, 'a', TRUE), (2, NULL, NULL, NULL), (3, 20, 'c', FALSE), \
               (4, 30, 'd', TRUE), (5, NULL, '', NULL); \
             CREATE TABLE u (id INT, dept INT, label TEXT); \
             INSERT INTO u VALUES (10, 1, 'eng'), (20, 2, 'sales'), (30, 4, 'ops'), \
               (40, 99, 'idle'), (50, NULL, 'nil');",
        )
        .expect("seed");

    let grouped = engine
        .execute_script(
            "SELECT CASE WHEN COUNT(*) > 1 THEN 'many' ELSE 'one' END AS k, COUNT(*) \
             FROM t GROUP BY score ORDER BY score;",
        )
        .expect("case aggregate");
    assert_eq!(
        as_rows(&grouped[0]).1,
        [
            vec![Value::Text("many".to_string()), Value::Int(2)],
            vec![Value::Text("one".to_string()), Value::Int(1)],
            vec![Value::Text("one".to_string()), Value::Int(1)],
            vec![Value::Text("one".to_string()), Value::Int(1)],
        ]
    );

    let having = engine
        .execute_script(
            "SELECT score, COUNT(*) FROM t GROUP BY score \
             HAVING CASE WHEN COUNT(*) > 1 THEN TRUE ELSE FALSE END ORDER BY score;",
        )
        .expect("case having");
    assert_eq!(as_rows(&having[0]).1, [vec![Value::Null, Value::Int(2)]]);

    let aggregate_branch = engine
        .execute_script(
            "SELECT CASE WHEN flag THEN COUNT(*) ELSE 0 END AS n \
             FROM t GROUP BY flag ORDER BY flag;",
        )
        .expect("aggregate in branch");
    assert_eq!(
        as_rows(&aggregate_branch[0]).1,
        [
            vec![Value::Int(0)],
            vec![Value::Int(0)],
            vec![Value::Int(2)],
        ]
    );

    let joined = engine
        .execute_script(
            "SELECT t.id, u.label FROM t JOIN u \
             ON CASE WHEN t.id = 1 THEN u.dept = 1 ELSE t.id = u.dept END \
             ORDER BY t.id;",
        )
        .expect("case join");
    assert_eq!(
        as_rows(&joined[0]).1,
        [
            vec![Value::Int(1), Value::Text("eng".to_string())],
            vec![Value::Int(2), Value::Text("sales".to_string())],
            vec![Value::Int(4), Value::Text("ops".to_string())],
        ]
    );

    let updated = engine
        .execute_script(
            "UPDATE t SET name = CASE WHEN score IS NULL THEN 'missing' ELSE name END \
             WHERE id <= 4;",
        )
        .expect("case update");
    assert_eq!(message(&updated[0]), "updated 4 row(s) in t");
    let names = engine
        .execute_script("SELECT id, name FROM t WHERE id <= 4 ORDER BY id;")
        .expect("names");
    assert_eq!(
        as_rows(&names[0]).1,
        [
            vec![Value::Int(1), Value::Text("a".to_string())],
            vec![Value::Int(2), Value::Text("missing".to_string())],
            vec![Value::Int(3), Value::Text("c".to_string())],
            vec![Value::Int(4), Value::Text("d".to_string())],
        ]
    );
}

#[test]
fn right_join_preserves_unmatched_right_rows() {
    let dir = TempDir::new("sql-right-join");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, score INT, name TEXT, flag BOOL); \
             INSERT INTO t VALUES \
               (1, 10, 'a', TRUE), (2, NULL, NULL, NULL), (3, 20, 'c', FALSE), \
               (4, 30, 'd', TRUE), (5, NULL, '', NULL); \
             CREATE TABLE u (id INT, dept INT, label TEXT); \
             INSERT INTO u VALUES (10, 1, 'eng'), (20, 2, 'sales'), (30, 4, 'ops'), \
               (40, 99, 'idle'), (50, NULL, 'nil');",
        )
        .expect("seed");

    let joined = engine
        .execute_script(
            "SELECT t.id, u.id, u.label FROM t RIGHT JOIN u ON t.id = u.dept \
             ORDER BY u.id;",
        )
        .expect("right join");
    assert_eq!(
        as_rows(&joined[0]).1,
        [
            vec![
                Value::Int(1),
                Value::Int(10),
                Value::Text("eng".to_string())
            ],
            vec![
                Value::Int(2),
                Value::Int(20),
                Value::Text("sales".to_string())
            ],
            vec![
                Value::Int(4),
                Value::Int(30),
                Value::Text("ops".to_string())
            ],
            vec![Value::Null, Value::Int(40), Value::Text("idle".to_string())],
            vec![Value::Null, Value::Int(50), Value::Text("nil".to_string())],
        ]
    );

    let reversed = engine
        .execute_script("SELECT u.id, t.id FROM u RIGHT JOIN t ON u.dept = t.id ORDER BY t.id;")
        .expect("reversed right join");
    assert_eq!(
        as_rows(&reversed[0]).1,
        [
            vec![Value::Int(10), Value::Int(1)],
            vec![Value::Int(20), Value::Int(2)],
            vec![Value::Null, Value::Int(3)],
            vec![Value::Int(30), Value::Int(4)],
            vec![Value::Null, Value::Int(5)],
        ]
    );

    engine
        .execute_script("CREATE TABLE e (id INT);")
        .expect("empty right");
    let empty = engine
        .execute_script("SELECT COUNT(*) FROM t RIGHT JOIN e ON t.id = e.id;")
        .expect("empty right count");
    assert_eq!(as_rows(&empty[0]).1, [vec![Value::Int(0)]]);
}

#[test]
fn full_join_preserves_unmatched_rows_from_both_sides() {
    let dir = TempDir::new("sql-full-join");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, score INT, name TEXT, flag BOOL); \
             INSERT INTO t VALUES \
               (1, 10, 'a', TRUE), (2, NULL, NULL, NULL), (3, 20, 'c', FALSE), \
               (4, 30, 'd', TRUE), (5, NULL, '', NULL); \
             CREATE TABLE u (id INT, dept INT, label TEXT); \
             INSERT INTO u VALUES (10, 1, 'eng'), (20, 2, 'sales'), (30, 4, 'ops'), \
               (40, 99, 'idle'), (50, NULL, 'nil');",
        )
        .expect("seed");

    let joined = engine
        .execute_script(
            "SELECT t.id AS tid, u.id AS uid FROM t FULL JOIN u ON t.id = u.dept \
             ORDER BY tid, uid;",
        )
        .expect("full join");
    assert_eq!(
        as_rows(&joined[0]).1,
        [
            vec![Value::Null, Value::Int(40)],
            vec![Value::Null, Value::Int(50)],
            vec![Value::Int(1), Value::Int(10)],
            vec![Value::Int(2), Value::Int(20)],
            vec![Value::Int(3), Value::Null],
            vec![Value::Int(4), Value::Int(30)],
            vec![Value::Int(5), Value::Null],
        ]
    );

    engine
        .execute_script("CREATE TABLE e (id INT);")
        .expect("empty table");
    let empty_right = engine
        .execute_script("SELECT t.id, e.id FROM t FULL JOIN e ON t.id = e.id ORDER BY t.id;")
        .expect("empty right");
    assert_eq!(
        as_rows(&empty_right[0]).1,
        [
            vec![Value::Int(1), Value::Null],
            vec![Value::Int(2), Value::Null],
            vec![Value::Int(3), Value::Null],
            vec![Value::Int(4), Value::Null],
            vec![Value::Int(5), Value::Null],
        ]
    );

    let empty_left = engine
        .execute_script("SELECT e.id, t.id FROM e FULL JOIN t ON e.id = t.id ORDER BY t.id;")
        .expect("empty left");
    assert_eq!(
        as_rows(&empty_left[0]).1,
        [
            vec![Value::Null, Value::Int(1)],
            vec![Value::Null, Value::Int(2)],
            vec![Value::Null, Value::Int(3)],
            vec![Value::Null, Value::Int(4)],
            vec![Value::Null, Value::Int(5)],
        ]
    );
}

#[test]
fn right_and_full_join_work_with_where_and_group_by() {
    let dir = TempDir::new("sql-outer-where");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, score INT, name TEXT, flag BOOL); \
             INSERT INTO t VALUES \
               (1, 10, 'a', TRUE), (2, NULL, NULL, NULL), (3, 20, 'c', FALSE), \
               (4, 30, 'd', TRUE), (5, NULL, '', NULL); \
             CREATE TABLE u (id INT, dept INT, label TEXT); \
             INSERT INTO u VALUES (10, 1, 'eng'), (20, 2, 'sales'), (30, 4, 'ops'), \
               (40, 99, 'idle'), (50, NULL, 'nil');",
        )
        .expect("seed");

    let unmatched_right = engine
        .execute_script(
            "SELECT u.id, u.label FROM t RIGHT JOIN u ON t.id = u.dept \
             WHERE t.id IS NULL ORDER BY u.id;",
        )
        .expect("right where");
    assert_eq!(
        as_rows(&unmatched_right[0]).1,
        [
            vec![Value::Int(40), Value::Text("idle".to_string())],
            vec![Value::Int(50), Value::Text("nil".to_string())],
        ]
    );

    let unmatched_either = engine
        .execute_script(
            "SELECT t.id AS tid, u.id AS uid FROM t FULL JOIN u ON t.id = u.dept \
             WHERE t.id IS NULL OR u.id IS NULL ORDER BY tid, uid;",
        )
        .expect("full where");
    assert_eq!(
        as_rows(&unmatched_either[0]).1,
        [
            vec![Value::Null, Value::Int(40)],
            vec![Value::Null, Value::Int(50)],
            vec![Value::Int(3), Value::Null],
            vec![Value::Int(5), Value::Null],
        ]
    );

    let grouped = engine
        .execute_script(
            "SELECT u.label, COUNT(*) FROM t FULL JOIN u ON t.id = u.dept \
             GROUP BY u.label ORDER BY u.label;",
        )
        .expect("full group");
    assert_eq!(
        as_rows(&grouped[0]).1,
        [
            vec![Value::Null, Value::Int(2)],
            vec![Value::Text("eng".to_string()), Value::Int(1)],
            vec![Value::Text("idle".to_string()), Value::Int(1)],
            vec![Value::Text("nil".to_string()), Value::Int(1)],
            vec![Value::Text("ops".to_string()), Value::Int(1)],
            vec![Value::Text("sales".to_string()), Value::Int(1)],
        ]
    );
}

#[test]
fn cross_join_and_comma_from_produce_cartesian_product() {
    let dir = TempDir::new("sql-cross-join");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE a (id INT NOT NULL, tag TEXT); \
             INSERT INTO a VALUES (1, 'x'), (2, 'y'); \
             CREATE TABLE b (id INT, label TEXT); \
             INSERT INTO b VALUES (10, 'p'), (20, 'q'), (30, 'r');",
        )
        .expect("seed");

    let crossed = engine
        .execute_script("SELECT a.id, b.id FROM a CROSS JOIN b ORDER BY a.id, b.id;")
        .expect("cross join");
    assert_eq!(
        as_rows(&crossed[0]).1,
        [
            vec![Value::Int(1), Value::Int(10)],
            vec![Value::Int(1), Value::Int(20)],
            vec![Value::Int(1), Value::Int(30)],
            vec![Value::Int(2), Value::Int(10)],
            vec![Value::Int(2), Value::Int(20)],
            vec![Value::Int(2), Value::Int(30)],
        ]
    );

    let comma = engine
        .execute_script("SELECT COUNT(*) FROM a, b;")
        .expect("comma from");
    assert_eq!(as_rows(&comma[0]).1, [vec![Value::Int(6)]]);

    let filtered = engine
        .execute_script(
            "SELECT x.id, y.label FROM a AS x CROSS JOIN b AS y \
             WHERE y.id = 20 ORDER BY x.id;",
        )
        .expect("aliased cross join");
    assert_eq!(
        as_rows(&filtered[0]).1,
        [
            vec![Value::Int(1), Value::Text("q".to_string())],
            vec![Value::Int(2), Value::Text("q".to_string())],
        ]
    );
}

#[test]
fn natural_and_using_joins_merge_columns_through_outer_joins() {
    let dir = TempDir::new("sql-natural-using");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE l (id INT NOT NULL, code INT, lv TEXT); \
             CREATE TABLE r (id INT, code INT, rv TEXT); \
             CREATE TABLE s (id INT, sv TEXT); \
             INSERT INTO l VALUES (1, 10, 'l1'), (2, 20, 'l2'), (3, 30, 'l3'); \
             INSERT INTO r VALUES (1, 10, 'r1'), (2, 20, 'r2'), (4, 40, 'r4'); \
             INSERT INTO s VALUES (1, 's1'), (5, 's5');",
        )
        .expect("seed");

    let natural = engine
        .execute_script("SELECT * FROM l NATURAL JOIN r ORDER BY id;")
        .expect("natural join");
    let (columns, rows) = as_rows(&natural[0]);
    assert_eq!(columns, ["id", "code", "lv", "rv"]);
    assert_eq!(
        rows,
        [
            vec![
                Value::Int(1),
                Value::Int(10),
                Value::Text("l1".to_string()),
                Value::Text("r1".to_string()),
            ],
            vec![
                Value::Int(2),
                Value::Int(20),
                Value::Text("l2".to_string()),
                Value::Text("r2".to_string()),
            ],
        ]
    );

    let using = engine
        .execute_script("SELECT * FROM l JOIN r USING (id) ORDER BY id;")
        .expect("using join");
    let (columns, rows) = as_rows(&using[0]);
    assert_eq!(columns, ["id", "code", "lv", "code", "rv"]);
    assert_eq!(
        rows,
        [
            vec![
                Value::Int(1),
                Value::Int(10),
                Value::Text("l1".to_string()),
                Value::Int(10),
                Value::Text("r1".to_string()),
            ],
            vec![
                Value::Int(2),
                Value::Int(20),
                Value::Text("l2".to_string()),
                Value::Int(20),
                Value::Text("r2".to_string()),
            ],
        ]
    );

    let qualified = engine
        .execute_script(
            "SELECT l.id AS lid, r.id AS rid \
             FROM l RIGHT JOIN r USING (id) ORDER BY rid;",
        )
        .expect("qualified merged columns");
    assert_eq!(
        as_rows(&qualified[0]).1,
        [
            vec![Value::Int(1), Value::Int(1)],
            vec![Value::Int(2), Value::Int(2)],
            vec![Value::Int(4), Value::Int(4)],
        ]
    );

    let left = engine
        .execute_script("SELECT * FROM l LEFT JOIN r USING (id) ORDER BY id;")
        .expect("left using");
    assert_eq!(
        as_rows(&left[0]).1,
        [
            vec![
                Value::Int(1),
                Value::Int(10),
                Value::Text("l1".to_string()),
                Value::Int(10),
                Value::Text("r1".to_string()),
            ],
            vec![
                Value::Int(2),
                Value::Int(20),
                Value::Text("l2".to_string()),
                Value::Int(20),
                Value::Text("r2".to_string()),
            ],
            vec![
                Value::Int(3),
                Value::Int(30),
                Value::Text("l3".to_string()),
                Value::Null,
                Value::Null,
            ],
        ]
    );

    let right = engine
        .execute_script("SELECT * FROM l RIGHT JOIN r USING (id) ORDER BY id;")
        .expect("right using");
    assert_eq!(
        as_rows(&right[0]).1,
        [
            vec![
                Value::Int(1),
                Value::Int(10),
                Value::Text("l1".to_string()),
                Value::Int(10),
                Value::Text("r1".to_string()),
            ],
            vec![
                Value::Int(2),
                Value::Int(20),
                Value::Text("l2".to_string()),
                Value::Int(20),
                Value::Text("r2".to_string()),
            ],
            vec![
                Value::Int(4),
                Value::Null,
                Value::Null,
                Value::Int(40),
                Value::Text("r4".to_string()),
            ],
        ]
    );

    let full = engine
        .execute_script("SELECT * FROM l FULL JOIN r USING (id) ORDER BY id;")
        .expect("full using");
    assert_eq!(
        as_rows(&full[0]).1,
        [
            vec![
                Value::Int(1),
                Value::Int(10),
                Value::Text("l1".to_string()),
                Value::Int(10),
                Value::Text("r1".to_string()),
            ],
            vec![
                Value::Int(2),
                Value::Int(20),
                Value::Text("l2".to_string()),
                Value::Int(20),
                Value::Text("r2".to_string()),
            ],
            vec![
                Value::Int(3),
                Value::Int(30),
                Value::Text("l3".to_string()),
                Value::Null,
                Value::Null,
            ],
            vec![
                Value::Int(4),
                Value::Null,
                Value::Null,
                Value::Int(40),
                Value::Text("r4".to_string()),
            ],
        ]
    );

    let chained = engine
        .execute_script("SELECT * FROM l NATURAL JOIN r NATURAL JOIN s ORDER BY id;")
        .expect("chained natural joins");
    let (columns, rows) = as_rows(&chained[0]);
    assert_eq!(columns, ["id", "code", "lv", "rv", "sv"]);
    assert_eq!(
        rows,
        [vec![
            Value::Int(1),
            Value::Int(10),
            Value::Text("l1".to_string()),
            Value::Text("r1".to_string()),
            Value::Text("s1".to_string()),
        ]]
    );

    let chained_qualified = engine
        .execute_script("SELECT l.id, r.id, s.id FROM l NATURAL JOIN r NATURAL JOIN s;")
        .expect("chained natural qualified columns");
    assert_eq!(
        as_rows(&chained_qualified[0]).1,
        [vec![Value::Int(1), Value::Int(1), Value::Int(1)]]
    );
}

#[test]
fn hash_join_handles_composite_null_and_residual_predicates() {
    let dir = TempDir::new("sql-hash-join");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE h1 (id INT, code INT, v TEXT); \
             CREATE TABLE h2 (id INT, code INT, w TEXT); \
             INSERT INTO h1 VALUES (1, 10, 'a1'), (1, 20, 'a2'), (NULL, 10, 'n1'), \
                 (2, NULL, 'n2'), (3, 30, 'a3'); \
             INSERT INTO h2 VALUES (1, 10, 'b1'), (1, 20, 'b2'), (2, 20, 'b3'), \
                 (NULL, 10, 'bn'), (4, 40, 'b4');",
        )
        .expect("seed");

    let matched = engine
        .execute_script(
            "SELECT h1.v, h2.w FROM h1 JOIN h2 \
             ON h1.id = h2.id AND h1.code = h2.code ORDER BY h1.v;",
        )
        .expect("composite hash join");
    assert_eq!(
        as_rows(&matched[0]).1,
        [
            vec![Value::Text("a1".to_string()), Value::Text("b1".to_string())],
            vec![Value::Text("a2".to_string()), Value::Text("b2".to_string())],
        ]
    );

    let residual = engine
        .execute_script(
            "SELECT h1.v, h2.w FROM h1 LEFT JOIN h2 \
             ON h1.id = h2.id AND h1.code = h2.code AND h2.w <> 'b2' ORDER BY h1.v;",
        )
        .expect("residual predicate");
    assert_eq!(
        as_rows(&residual[0]).1,
        [
            vec![Value::Text("a1".to_string()), Value::Text("b1".to_string())],
            vec![Value::Text("a2".to_string()), Value::Null],
            vec![Value::Text("a3".to_string()), Value::Null],
            vec![Value::Text("n1".to_string()), Value::Null],
            vec![Value::Text("n2".to_string()), Value::Null],
        ]
    );

    let fallback = engine
        .execute_script("SELECT COUNT(*) FROM h1 JOIN h2 ON h1.id < h2.id;")
        .expect("non-equi fallback");
    assert_eq!(as_rows(&fallback[0]).1, [vec![Value::Int(6)]]);

    let full = engine
        .execute_script("SELECT COUNT(*) FROM h1 FULL JOIN h2 ON h1.id = h2.id;")
        .expect("full hash join with null keys");
    assert_eq!(as_rows(&full[0]).1, [vec![Value::Int(9)]]);
}

#[test]
fn explain_sexpr_reports_structured_plan() {
    let dir = TempDir::new("sql-explain-sexpr");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script("CREATE TABLE a (id INT, v TEXT); CREATE TABLE b (id INT, w TEXT);")
        .expect("seed");

    let explained = engine
        .execute_script(
            "EXPLAIN SEXPR SELECT a.v FROM a JOIN b ON a.id = b.id \
             WHERE a.v <> 'skip' ORDER BY a.v LIMIT 1;",
        )
        .expect("explain sexpr");
    assert_eq!(as_rows(&explained[0]).0, ["plan"]);
    assert_eq!(as_rows(&explained[0]).1.len(), 1);
    let plan = match &as_rows(&explained[0]).1[0][0] {
        Value::Text(plan) => plan,
        other => panic!("expected plan text, got {other:?}"),
    };
    assert_eq!(
        plan,
        "(limit :limit 1\n  (sort :keys ((asc (col \"a.v\")))\n    (project ((\"v\" (col \"a.v\")))\n      \
         (join hash inner :keys ((\"a.id\" \"b.id\")) \
         :pred (= (col \"a.id\") (col \"b.id\"))\n        \
         (scan \"a\" :filtered #t :filter (<> (col \"a.v\") \"skip\"))\n        (scan \"b\" :cols (\"id\"))))))"
    );

    let both = engine
        .execute_script(
            "EXPLAIN SEXPR SELECT a.v FROM a JOIN b ON a.id = b.id \
             WHERE a.v <> 'skip' AND b.w <> 'keep';",
        )
        .expect("explain sexpr both scan filters");
    let both_plan = match &as_rows(&both[0]).1[0][0] {
        Value::Text(plan) => plan,
        other => panic!("expected plan text, got {other:?}"),
    };
    assert!(
        both_plan.contains("(scan \"a\" :filtered #t :filter (<> (col \"a.v\") \"skip\"))"),
        "unexpected plan: {both_plan}"
    );
    assert!(
        both_plan.contains("(scan \"b\" :filtered #t :filter (<> (col \"b.w\") \"keep\"))"),
        "unexpected plan: {both_plan}"
    );

    let aliased = engine
        .execute_script("EXPLAIN SEXPR SELECT x.v FROM a AS x;")
        .expect("explain sexpr aliased scan");
    let aliased_plan = match &as_rows(&aliased[0]).1[0][0] {
        Value::Text(plan) => plan,
        other => panic!("expected plan text, got {other:?}"),
    };
    assert!(
        aliased_plan.contains("(scan \"a\" :alias \"x\""),
        "unexpected plan: {aliased_plan}"
    );

    let aliased_text = engine
        .execute_script("EXPLAIN SELECT x.v FROM a AS x WHERE x.id > 1;")
        .expect("explain aliased scan");
    let aliased_text_plan = as_rows(&aliased_text[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan line, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        aliased_text_plan.contains("Scan a AS x"),
        "unexpected plan: {aliased_text_plan}"
    );

    let having = engine
        .execute_script(
            "EXPLAIN SEXPR SELECT a.v, COUNT(*) FROM a \
             GROUP BY a.v HAVING COUNT(*) > 1 AND a.v IS NOT NULL;",
        )
        .expect("explain sexpr having");
    let having_plan = match &as_rows(&having[0]).1[0][0] {
        Value::Text(plan) => plan,
        other => panic!("expected plan text, got {other:?}"),
    };
    assert!(
        having_plan.contains(
            "(having :pred (and (> (count :all) 1) \
             (is-not-null (col \"a.v\")))"
        ),
        "unexpected plan: {having_plan}"
    );
    assert!(
        having_plan.contains("(aggregate :keys ((col \"a.v\")) :aggregates ((count :all))"),
        "unexpected plan: {having_plan}"
    );
    assert!(!having_plan.contains(":est-groups"), "{having_plan}");
    assert!(!having_plan.contains(":actual-groups"), "{having_plan}");

    let summed = engine
        .execute_script(
            "EXPLAIN SEXPR SELECT a.v, SUM(a.id) FROM a \
             GROUP BY a.v HAVING SUM(a.id) > 2;",
        )
        .expect("explain sexpr having sum");
    let summed_plan = match &as_rows(&summed[0]).1[0][0] {
        Value::Text(plan) => plan,
        other => panic!("expected plan text, got {other:?}"),
    };
    assert!(
        summed_plan.contains("(having :pred (> (sum (col \"a.id\")) 2)"),
        "unexpected plan: {summed_plan}"
    );
    assert!(
        summed_plan.contains("(aggregate :keys ((col \"a.v\")) :aggregates ((sum (col \"a.id\")))"),
        "unexpected plan: {summed_plan}"
    );

    let global = engine
        .execute_script("EXPLAIN SEXPR SELECT COUNT(*) FROM a;")
        .expect("explain sexpr global aggregate");
    let global_plan = match &as_rows(&global[0]).1[0][0] {
        Value::Text(plan) => plan,
        other => panic!("expected plan text, got {other:?}"),
    };
    assert!(
        global_plan.contains("(aggregate :keys () :aggregates ((count :all))"),
        "unexpected plan: {global_plan}"
    );

    let grouped = engine
        .execute_script("EXPLAIN SEXPR SELECT a.v FROM a GROUP BY a.v;")
        .expect("explain sexpr group only");
    let grouped_plan = match &as_rows(&grouped[0]).1[0][0] {
        Value::Text(plan) => plan,
        other => panic!("expected plan text, got {other:?}"),
    };
    assert!(
        grouped_plan.contains("(aggregate :keys ((col \"a.v\")) :aggregates ()"),
        "unexpected plan: {grouped_plan}"
    );

    let sorted = engine
        .execute_script(
            "EXPLAIN SEXPR SELECT a.v, SUM(a.id) FROM a GROUP BY a.v \
             ORDER BY SUM(a.id) DESC, a.v ASC;",
        )
        .expect("explain sexpr sorted");
    let sorted_plan = match &as_rows(&sorted[0]).1[0][0] {
        Value::Text(plan) => plan,
        other => panic!("expected plan text, got {other:?}"),
    };
    assert!(
        sorted_plan.contains("(sort :keys ((desc (sum (col \"a.id\"))) (asc (col \"a.v\")))"),
        "unexpected plan: {sorted_plan}"
    );

    let sorted_source = engine
        .execute_script("EXPLAIN SEXPR SELECT a.v FROM a ORDER BY a.id DESC;")
        .expect("explain sexpr sorted source column");
    let sorted_source_plan = match &as_rows(&sorted_source[0]).1[0][0] {
        Value::Text(plan) => plan,
        other => panic!("expected plan text, got {other:?}"),
    };
    assert!(
        sorted_source_plan.contains("(sort :keys ((desc (col \"a.id\")))"),
        "unexpected plan: {sorted_source_plan}"
    );

    let projected = engine
        .execute_script("EXPLAIN SEXPR SELECT a.v, SUM(a.id) AS total FROM a GROUP BY a.v;")
        .expect("explain sexpr projected aggregate");
    let projected_plan = match &as_rows(&projected[0]).1[0][0] {
        Value::Text(plan) => plan,
        other => panic!("expected plan text, got {other:?}"),
    };
    assert!(
        projected_plan
            .contains("(project ((\"v\" (col \"a.v\")) (\"total\" (sum (col \"a.id\"))))"),
        "unexpected plan: {projected_plan}"
    );
}

#[test]
fn explain_analyze_sexpr_reports_actual_metrics() {
    let dir = TempDir::new("sql-explain-analyze-sexpr");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE a (id INT, v TEXT); \
             CREATE TABLE b (id INT, w TEXT); \
             INSERT INTO a VALUES (1, 'a1'), (2, 'a2'), (3, 'a3'); \
             INSERT INTO b VALUES (1, 'b1'), (2, 'b2'), (2, 'b2x'), (4, 'b4');",
        )
        .expect("seed");

    let analyzed = engine
        .execute_script(
            "EXPLAIN ANALYZE SEXPR SELECT a.v FROM a JOIN b \
             ON a.id = b.id AND b.w <> 'b2x' \
             WHERE a.id >= 1 OR b.w <> 'b2x' ORDER BY a.v LIMIT 1;",
        )
        .expect("explain analyze sexpr");
    let (columns, rows) = as_rows(&analyzed[0]);
    assert_eq!(columns, ["plan"]);
    assert_eq!(rows.len(), 1);
    let plan = match &rows[0][0] {
        Value::Text(plan) => plan,
        other => panic!("expected plan text, got {other:?}"),
    };
    assert!(
        plan.starts_with("(execution :rows 1 :time-ms "),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("(limit :limit 1 :actual-rows 1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("(sort :keys ((asc (col \"a.v\"))) :actual-rows 2 :time-ms "),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("(project ((\"v\" (col \"a.v\"))) :actual-rows 2 :time-ms "),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains(
            "(filter :pred (or (>= (col \"a.id\") 1) (<> (col \"b.w\") \"b2x\")) \
             :actual-rows 2 :time-ms "
        ),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains(
            "(join hash inner :keys ((\"a.id\" \"b.id\")) :residual #t \
             :residual-pred (<> (col \"b.w\") \"b2x\") \
             :pred (and (= (col \"a.id\") (col \"b.id\")) \
             (<> (col \"b.w\") \"b2x\")) :actual-rows 2 :time-ms "
        ),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("(scan \"a\" :actual-rows 3)"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("(scan \"b\" :actual-rows 4)"),
        "unexpected plan: {plan}"
    );
    assert!(!plan.contains(":est-rows"), "unexpected plan: {plan}");
    assert!(!plan.contains("Execution:"), "unexpected plan: {plan}");

    let pushed = engine
        .execute_script(
            "EXPLAIN ANALYZE SEXPR SELECT a.v FROM a JOIN b ON a.id = b.id \
             WHERE a.v <> 'skip' ORDER BY a.v LIMIT 1;",
        )
        .expect("explain analyze sexpr pushed filter");
    let pushed_plan = match &as_rows(&pushed[0]).1[0][0] {
        Value::Text(plan) => plan,
        other => panic!("expected plan text, got {other:?}"),
    };
    assert!(
        pushed_plan.contains(
            "(scan \"a\" :filtered #t :filter (<> (col \"a.v\") \"skip\") :actual-rows 3)"
        ),
        "unexpected plan: {pushed_plan}"
    );

    let grouped = engine
        .execute_script("EXPLAIN ANALYZE SEXPR SELECT a.v, COUNT(*) FROM a GROUP BY a.v;")
        .expect("explain analyze sexpr grouped");
    let grouped_plan = match &as_rows(&grouped[0]).1[0][0] {
        Value::Text(plan) => plan,
        other => panic!("expected plan text, got {other:?}"),
    };
    assert!(
        grouped_plan.contains(
            "(aggregate :keys ((col \"a.v\")) :aggregates ((count :all)) \
             :actual-groups 3 :time-ms "
        ),
        "unexpected plan: {grouped_plan}"
    );
    assert!(!grouped_plan.contains(":est-groups"), "{grouped_plan}");
}

#[test]
fn explain_sexpr_reports_structured_filter_predicate() {
    let dir = TempDir::new("sql-explain-sexpr-predicate");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE a (id INT, v TEXT); CREATE TABLE b (id INT, w TEXT); \
             INSERT INTO a VALUES (1, 'a1'), (2, 'skip'), (3, 'a3'); \
             INSERT INTO b VALUES (1, 'b1'), (2, 'skip'), (4, 'b4');",
        )
        .expect("seed");

    let explained = engine
        .execute_script(
            "EXPLAIN SEXPR SELECT a.v FROM a JOIN b ON a.id = b.id \
             WHERE a.v <> 'skip' OR b.w LIKE 'b%';",
        )
        .expect("explain sexpr predicate");
    let (_, rows) = as_rows(&explained[0]);
    let plan = match &rows[0][0] {
        Value::Text(plan) => plan,
        other => panic!("expected plan text, got {other:?}"),
    };
    assert_eq!(
        plan,
        "(project ((\"v\" (col \"a.v\")))\n  (filter :pred (or (<> (col \"a.v\") \"skip\") \
         (like (col \"b.w\") \"b%\"))\n    (join hash inner :keys ((\"a.id\" \"b.id\")) \
         :pred (= (col \"a.id\") (col \"b.id\"))\n      \
         (scan \"a\")\n      (scan \"b\"))))"
    );
}

#[test]
fn explain_sexpr_reports_structured_join_keys() {
    fn plan_text(engine: &mut SqlEngine, sql: &str) -> String {
        let result = engine.execute_script(sql).expect("explain sexpr");
        let (_, rows) = as_rows(&result[0]);
        match &rows[0][0] {
            Value::Text(plan) => plan.clone(),
            other => panic!("expected plan text, got {other:?}"),
        }
    }

    let dir = TempDir::new("sql-explain-sexpr-join-keys");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script("CREATE TABLE a (id INT, code INT); CREATE TABLE b (id INT, code INT);")
        .expect("seed");

    let using = plan_text(
        &mut engine,
        "EXPLAIN SEXPR SELECT * FROM a LEFT JOIN b USING (id);",
    );
    assert!(
        using.contains("(join hash left :keys (\"id\") :pred (= (col \"a.id\") (col \"b.id\"))"),
        "unexpected plan: {using}"
    );
    assert!(!using.contains(":details"), "unexpected plan: {using}");
    assert!(
        !using.contains(":residual-pred"),
        "USING join must not invent a residual predicate: {using}"
    );

    let residual = plan_text(
        &mut engine,
        "EXPLAIN SEXPR SELECT a.id FROM a JOIN b ON a.id = b.id AND b.code <> 0;",
    );
    assert!(
        residual.contains(
            ":keys ((\"a.id\" \"b.id\")) :residual #t \
             :residual-pred (<> (col \"b.code\") 0) \
             :pred (and (= (col \"a.id\") (col \"b.id\")) (<> (col \"b.code\") 0))"
        ),
        "unexpected plan: {residual}"
    );
    assert!(
        !residual.contains(":details"),
        "unexpected plan: {residual}"
    );

    let composite = plan_text(
        &mut engine,
        "EXPLAIN SEXPR SELECT a.id FROM a JOIN b \
         ON a.id = b.id AND a.code = b.code AND b.code <> 0;",
    );
    assert!(
        composite.contains(
            ":keys ((\"a.id\" \"b.id\") (\"a.code\" \"b.code\")) :residual #t \
             :residual-pred (<> (col \"b.code\") 0)"
        ),
        "unexpected plan: {composite}"
    );

    let or_residual = plan_text(
        &mut engine,
        "EXPLAIN SEXPR SELECT a.id FROM a JOIN b \
         ON a.id = b.id AND (b.code <> 0 OR a.code <> 1);",
    );
    assert!(
        or_residual.contains(
            ":residual #t :residual-pred (or (<> (col \"b.code\") 0) \
             (<> (col \"a.code\") 1))"
        ),
        "unexpected plan: {or_residual}"
    );

    let pure = plan_text(
        &mut engine,
        "EXPLAIN SEXPR SELECT a.id FROM a JOIN b \
         ON a.id = b.id AND a.code = b.code;",
    );
    assert!(
        !pure.contains(":residual-pred"),
        "pure equi join must not carry a residual predicate: {pure}"
    );

    let same_source = plan_text(
        &mut engine,
        "EXPLAIN SEXPR SELECT a.id FROM a JOIN b \
         ON a.id = b.id AND a.id = a.code;",
    );
    assert!(
        same_source.contains(
            ":keys ((\"a.id\" \"b.id\")) \
             :residual-pred (= (col \"a.id\") (col \"a.code\")) \
             :pred (and (= (col \"a.id\") (col \"b.id\")) \
             (= (col \"a.id\") (col \"a.code\")))"
        ),
        "same-source equality must still expose its non-key conjunct: {same_source}"
    );
    assert!(
        !same_source.contains(":residual #t"),
        "historical residual flag must stay unchanged: {same_source}"
    );

    let same_source_text = engine
        .execute_script("EXPLAIN SELECT a.id FROM a JOIN b ON a.id = b.id AND a.id = a.code;")
        .expect("explain same-source join");
    let same_source_text = as_rows(&same_source_text[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan line, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        same_source_text.contains("HashJoin (inner) keys=(a.id = b.id)"),
        "unexpected text plan: {same_source_text}"
    );
    assert!(
        !same_source_text.contains("residual=yes"),
        "text plan must not gain a residual marker: {same_source_text}"
    );

    engine.set_join_strategy(JoinStrategy::NestedLoop);
    let nested = plan_text(
        &mut engine,
        "EXPLAIN SEXPR SELECT a.id FROM a JOIN b ON a.id = b.id;",
    );
    assert!(
        nested.contains(
            "(join nested-loop inner :on-expr #t \
             :pred (= (col \"a.id\") (col \"b.id\"))"
        ),
        "unexpected plan: {nested}"
    );
    assert!(!nested.contains(":details"), "unexpected plan: {nested}");
    assert!(
        !nested.contains(":residual-pred"),
        "nested-loop join must not carry a residual predicate: {nested}"
    );
}

#[test]
fn explain_sexpr_includes_estimates_and_zero_columns() {
    let dir = TempDir::new("sql-explain-sexpr-est");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE a (id INT, spare TEXT); \
             INSERT INTO a VALUES (1, 'x'), (2, 'y'), (3, 'z'); \
             ANALYZE;",
        )
        .expect("seed and analyze");
    let explained = engine
        .execute_script("EXPLAIN SEXPR SELECT 1 AS one FROM a;")
        .expect("explain sexpr estimates");
    let (columns, rows) = as_rows(&explained[0]);
    assert_eq!(columns, ["plan"]);
    assert_eq!(rows.len(), 1);
    let plan = match &rows[0][0] {
        Value::Text(plan) => plan,
        other => panic!("expected plan text, got {other:?}"),
    };
    assert_eq!(
        plan,
        "(project ((\"one\" 1)) :est-rows 3\n  (scan \"a\" :est-rows 3 :cols ()))"
    );
}

#[test]
fn explain_reports_hash_and_nested_loop_join_plans() {
    let dir = TempDir::new("sql-explain");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE e1 (id INT, code INT, v TEXT); \
             CREATE TABLE e2 (id INT, code INT, w TEXT);",
        )
        .expect("seed");

    let plan_text = |result: &QueryResult| {
        let (_, rows) = as_rows(result);
        rows.iter()
            .map(|row| match &row[0] {
                Value::Text(line) => line.clone(),
                other => panic!("expected plan text, got {other:?}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let explained = engine
        .execute_script(
            "EXPLAIN SELECT e1.v FROM e1 JOIN e2 \
             ON e1.id = e2.id AND e1.code = e2.code AND e2.w <> 'x' \
             WHERE e1.v <> 'skip' ORDER BY e1.v LIMIT 2;",
        )
        .expect("explain hash join");
    let plan = plan_text(&explained[0]);
    assert_eq!(
        plan,
        "Limit 2\n  Sort (1 key(s))\n    Project [v]\n      \
         HashJoin (inner) keys=(e1.id = e2.id, e1.code = e2.code) residual=yes\n        \
         Scan e1 filtered\n        Scan e2"
    );
    assert_eq!(as_rows(&explained[0]).0, ["plan"]);

    engine.set_join_strategy(JoinStrategy::NestedLoop);
    let forced = engine
        .execute_script(
            "EXPLAIN SELECT e1.v FROM e1 JOIN e2 \
             ON e1.id = e2.id AND e1.code = e2.code;",
        )
        .expect("explain nested loop");
    let plan = plan_text(&forced[0]);
    assert!(
        plan.contains("NestedLoop (inner) on=expr"),
        "unexpected plan: {plan}"
    );
    assert!(!plan.contains("HashJoin"), "unexpected plan: {plan}");

    engine.set_join_strategy(JoinStrategy::Auto);
    let using = engine
        .execute_script("EXPLAIN SELECT * FROM e1 LEFT JOIN e2 USING (id);")
        .expect("explain using");
    let plan = plan_text(&using[0]);
    assert!(
        plan.contains("HashJoin (left) keys=(id)"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Project [id, code, v, code, w]"),
        "unexpected plan: {plan}"
    );
    assert!(!plan.contains("residual"), "unexpected plan: {plan}");
}

#[test]
fn explain_analyze_reports_actual_rows_and_times() {
    let dir = TempDir::new("sql-explain-analyze");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE a (id INT, v TEXT); \
             CREATE TABLE b (id INT, w TEXT); \
             INSERT INTO a VALUES (1, 'a1'), (2, 'a2'), (3, 'a3'); \
             INSERT INTO b VALUES (1, 'b1'), (2, 'b2'), (2, 'b2x'), (4, 'b4');",
        )
        .expect("seed");

    let plan_text = |result: &QueryResult| {
        let (_, rows) = as_rows(result);
        rows.iter()
            .map(|row| match &row[0] {
                Value::Text(line) => line.clone(),
                other => panic!("expected plan text, got {other:?}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let analyzed = engine
        .execute_script(
            "EXPLAIN ANALYZE SELECT a.v FROM a JOIN b \
             ON a.id = b.id AND b.w <> 'b2x' \
             WHERE a.id >= 1 OR b.w <> 'b2x' ORDER BY a.v LIMIT 1;",
        )
        .expect("explain analyze join");
    let plan = plan_text(&analyzed[0]);
    assert_eq!(as_rows(&analyzed[0]).0, ["plan"]);
    assert!(
        plan.contains("Limit 1 actual rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Sort (1 key(s)) actual rows=2 time="),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Project [v] actual rows=2 time="),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Filter actual rows=2 time="),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("HashJoin (inner) keys=(a.id = b.id) residual=yes actual rows=2 time="),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Scan a actual rows=3"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Scan b actual rows=4"),
        "unexpected plan: {plan}"
    );
    let execution = plan.lines().last().expect("execution line");
    assert!(
        execution.starts_with("Execution: rows=1 time=") && execution.ends_with("ms"),
        "unexpected execution line: {execution}"
    );

    let grouped = engine
        .execute_script(
            "EXPLAIN ANALYZE SELECT a.v, COUNT(*) AS n FROM a JOIN b ON a.id = b.id \
             GROUP BY a.v HAVING COUNT(*) >= 1 ORDER BY a.v LIMIT 2;",
        )
        .expect("explain analyze grouped join");
    let plan = plan_text(&grouped[0]);
    assert!(
        plan.contains("Aggregate actual groups=2 time="),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Having actual groups=2 time="),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Project [v, n] actual rows=2 time="),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("HashJoin (inner) keys=(a.id = b.id) actual rows=3 time="),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Execution: rows=2 time="),
        "unexpected plan: {plan}"
    );
}

#[test]
fn inner_join_pushes_single_source_where_filter_to_scan() {
    let dir = TempDir::new("sql-pushdown");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE a (id INT, v TEXT); \
             CREATE TABLE b (id INT, w TEXT); \
             INSERT INTO a VALUES (1, 'a1'), (2, 'a2'), (3, 'a3'); \
             INSERT INTO b VALUES (1, 'b1'), (2, 'b2'), (3, 'b3'); \
             ANALYZE;",
        )
        .expect("seed and analyze");

    let plan_text = |result: &QueryResult| {
        let (_, rows) = as_rows(result);
        rows.iter()
            .map(|row| match &row[0] {
                Value::Text(line) => line.clone(),
                other => panic!("expected plan text, got {other:?}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let explained = engine
        .execute_script("EXPLAIN SELECT a.v, b.w FROM a JOIN b ON a.id = b.id WHERE a.id = 2;")
        .expect("explain pushed filter");
    let plan = plan_text(&explained[0]);
    assert!(
        plan.contains("Scan a filtered est rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("NestedLoop (inner) on=expr est rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(!plan.contains("Filter"), "unexpected plan: {plan}");
    assert!(
        !plan.contains("Scan a est rows=3"),
        "unexpected plan: {plan}"
    );

    let analyzed = engine
        .execute_script(
            "EXPLAIN ANALYZE SELECT a.v, b.w FROM a JOIN b ON a.id = b.id WHERE a.id = 2;",
        )
        .expect("explain analyze pushed filter");
    let plan = plan_text(&analyzed[0]);
    assert!(
        plan.contains("Scan a filtered actual rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("NestedLoop (inner) on=expr actual rows=1 time="),
        "unexpected plan: {plan}"
    );
    assert!(!plan.contains("Filter"), "unexpected plan: {plan}");
    assert!(
        plan.contains("Scan b actual rows=3"),
        "unexpected plan: {plan}"
    );

    let selected = engine
        .execute_script("SELECT a.v, b.w FROM a JOIN b ON a.id = b.id WHERE a.id = 2;")
        .expect("select pushed filter");
    assert_eq!(
        as_rows(&selected[0]).1,
        [vec![
            Value::Text("a2".to_string()),
            Value::Text("b2".to_string())
        ]]
    );

    let right = engine
        .execute_script(
            "EXPLAIN ANALYZE SELECT a.v, b.w FROM a JOIN b ON a.id = b.id \
             WHERE b.w = 'b2';",
        )
        .expect("explain analyze right scan filter");
    let plan = plan_text(&right[0]);
    assert!(
        plan.contains("Scan b filtered actual rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Scan a actual rows=3"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("NestedLoop (inner) on=expr actual rows=1 time="),
        "unexpected plan: {plan}"
    );
    assert!(!plan.contains("Filter"), "unexpected plan: {plan}");

    let outer = engine
        .execute_script(
            "EXPLAIN SELECT a.v, b.w FROM a LEFT JOIN b ON a.id = b.id \
             WHERE a.id = 2 AND b.w = 'b2';",
        )
        .expect("outer join plan");
    let plan = plan_text(&outer[0]);
    assert!(
        plan.contains("Scan a est rows=3"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Filter est rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(!plan.contains("filtered"), "unexpected plan: {plan}");

    let mixed = engine
        .execute_script(
            "EXPLAIN SELECT a.v, b.w FROM a JOIN b ON a.id = b.id \
             WHERE a.id = b.id;",
        )
        .expect("multi-source predicate plan");
    let plan = plan_text(&mixed[0]);
    assert!(
        plan.contains("Scan a est rows=3"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Filter est rows=2"),
        "unexpected plan: {plan}"
    );
    assert!(!plan.contains("filtered"), "unexpected plan: {plan}");
}

#[test]
fn inner_join_splits_conjunctive_where_filters_across_scans() {
    let dir = TempDir::new("sql-conjunctive-pushdown");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE a (id INT, v TEXT); \
             CREATE TABLE b (id INT, w TEXT); \
             INSERT INTO a VALUES (1, 'a1'), (2, 'a2'), (3, 'a3'); \
             INSERT INTO b VALUES (1, 'b1'), (2, 'b2'), (3, 'b3'); \
             ANALYZE;",
        )
        .expect("seed and analyze");

    let plan_text = |result: &QueryResult| {
        let (_, rows) = as_rows(result);
        rows.iter()
            .map(|row| match &row[0] {
                Value::Text(line) => line.clone(),
                other => panic!("expected plan text, got {other:?}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let split = engine
        .execute_script(
            "EXPLAIN SELECT a.v, b.w FROM a JOIN b ON a.id = b.id \
             WHERE a.id = 2 AND b.w = 'b2';",
        )
        .expect("explain split conjunctive filter");
    let plan = plan_text(&split[0]);
    assert!(
        plan.contains("Scan a filtered est rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Scan b filtered est rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("NestedLoop (inner) on=expr est rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(!plan.contains("Filter"), "unexpected plan: {plan}");

    let split_actual = engine
        .execute_script(
            "EXPLAIN ANALYZE SELECT a.v, b.w FROM a JOIN b ON a.id = b.id \
             WHERE a.id = 2 AND b.w = 'b2';",
        )
        .expect("explain analyze split conjunctive filter");
    let plan = plan_text(&split_actual[0]);
    assert!(
        plan.contains("Scan a filtered actual rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Scan b filtered actual rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("NestedLoop (inner) on=expr actual rows=1 time="),
        "unexpected plan: {plan}"
    );
    assert!(!plan.contains("Filter"), "unexpected plan: {plan}");

    let selected = engine
        .execute_script(
            "SELECT a.v, b.w FROM a JOIN b ON a.id = b.id \
             WHERE a.id = 2 AND b.w = 'b2';",
        )
        .expect("select split conjunctive filter");
    assert_eq!(
        as_rows(&selected[0]).1,
        [vec![
            Value::Text("a2".to_string()),
            Value::Text("b2".to_string())
        ]]
    );

    let cross = engine
        .execute_script(
            "EXPLAIN SELECT a.v, b.w FROM a CROSS JOIN b \
             WHERE a.id = 2 AND b.w = 'b2';",
        )
        .expect("explain cross join conjunctive filter");
    let plan = plan_text(&cross[0]);
    assert!(
        plan.contains("Scan a filtered est rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Scan b filtered est rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("NestedLoop (cross) est rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(!plan.contains("Filter"), "unexpected plan: {plan}");

    let same_source = engine
        .execute_script(
            "EXPLAIN SELECT a.v, b.w FROM a JOIN b ON a.id = b.id \
             WHERE a.id = 2 AND a.v = 'a2' AND b.w = 'b2';",
        )
        .expect("explain same-source conjuncts");
    let plan = plan_text(&same_source[0]);
    assert!(
        plan.contains("Scan a filtered est rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Scan b filtered est rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(!plan.contains("Filter"), "unexpected plan: {plan}");

    let residual = engine
        .execute_script(
            "EXPLAIN SELECT a.v, b.w FROM a JOIN b ON a.id = b.id \
             WHERE a.id = 2 AND b.id = 2 AND a.id = b.id;",
        )
        .expect("explain residual conjunct");
    let plan = plan_text(&residual[0]);
    assert!(
        plan.contains("Scan a filtered est rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Scan b filtered est rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("NestedLoop (inner) on=expr est rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Filter est rows=1"),
        "unexpected plan: {plan}"
    );

    let residual_actual = engine
        .execute_script(
            "EXPLAIN ANALYZE SELECT a.v, b.w FROM a JOIN b ON a.id = b.id \
             WHERE a.id = 2 AND b.id = 2 AND a.id = b.id;",
        )
        .expect("explain analyze residual conjunct");
    let plan = plan_text(&residual_actual[0]);
    assert!(
        plan.contains("Scan a filtered actual rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Scan b filtered actual rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("NestedLoop (inner) on=expr actual rows=1 time="),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Filter actual rows=1 time="),
        "unexpected plan: {plan}"
    );

    let unsplittable = engine
        .execute_script(
            "EXPLAIN SELECT a.v, b.w FROM a JOIN b ON a.id = b.id \
             WHERE a.id = 2 OR b.id = 2;",
        )
        .expect("explain unsplittable OR filter");
    let plan = plan_text(&unsplittable[0]);
    assert!(
        plan.contains("Scan a est rows=3"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Scan b est rows=3"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Filter est rows=2"),
        "unexpected plan: {plan}"
    );
    assert!(!plan.contains("filtered"), "unexpected plan: {plan}");
}

#[test]
fn analyze_persists_and_explain_uses_estimates() {
    let dir = TempDir::new("sql-analyze-explain");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    let results = engine
        .execute_script(
            "CREATE TABLE a (id INT, v TEXT); \
             CREATE TABLE b (id INT, w TEXT); \
             INSERT INTO a VALUES (1, 'x'), (2, 'y'), (3, NULL); \
             INSERT INTO b VALUES (1, 'p'), (2, 'q'), (2, 'r'); \
             ANALYZE;",
        )
        .expect("seed and analyze");
    assert_eq!(message(&results[4]), "analyzed 2 table(s)");

    let explained = engine
        .execute_script(
            "EXPLAIN SELECT a.id, b.w FROM a JOIN b ON a.id = b.id \
             WHERE a.id >= 2 ORDER BY a.id LIMIT 2;",
        )
        .expect("explain with estimates");
    let (columns, rows) = as_rows(&explained[0]);
    assert_eq!(columns, ["plan"]);
    let plan = rows
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        plan,
        "Limit 2 est rows=1\n  Sort (1 key(s)) est rows=1\n    \
         Project [id, w] est rows=1\n      \
         NestedLoop (inner) on=expr est rows=1\n        \
         Scan a filtered est rows=1 cols=id\n        Scan b est rows=3"
    );
    let grouped = engine
        .execute_script(
            "EXPLAIN SELECT a.v, COUNT(*) AS n FROM a JOIN b ON a.id = b.id \
             GROUP BY a.v HAVING COUNT(*) >= 1 ORDER BY a.v;",
        )
        .expect("explain grouped estimates");
    let plan = as_rows(&grouped[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("Aggregate est groups=2"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Having est groups=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Project [v, n] est rows=1"),
        "unexpected plan: {plan}"
    );
    drop(engine);

    let mut reopened = SqlEngine::open(&db).expect("reopen");
    let explained = reopened
        .execute_script("EXPLAIN SELECT a.id FROM a JOIN b ON a.id = b.id;")
        .expect("explain after reopen");
    let plan = as_rows(&explained[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("Project [id] est rows=3"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("HashJoin (inner) keys=(a.id = b.id) est rows=3"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Scan a est rows=3"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Scan b est rows=3"),
        "unexpected plan: {plan}"
    );
}

#[test]
fn auto_join_uses_statistics_to_choose_nested_loop() {
    let dir = TempDir::new("sql-join-cost");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE a (id INT, v TEXT); \
             CREATE TABLE b (id INT, w TEXT); \
             INSERT INTO a VALUES (1, 'a1'), (2, 'a2'); \
             INSERT INTO b VALUES (1, 'b1'), (2, 'b2'); \
             ANALYZE;",
        )
        .expect("seed and analyze");

    let plan_text = |result: &QueryResult| {
        as_rows(result)
            .1
            .iter()
            .map(|row| match &row[0] {
                Value::Text(line) => line.clone(),
                other => panic!("expected plan text, got {other:?}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let explained = engine
        .execute_script("EXPLAIN SELECT a.v FROM a JOIN b ON a.id = b.id;")
        .expect("explain auto join");
    let plan = plan_text(&explained[0]);
    assert!(
        plan.contains("NestedLoop (inner) on=expr est rows=2"),
        "unexpected plan: {plan}"
    );
    assert!(!plan.contains("HashJoin"), "unexpected plan: {plan}");

    let selected = engine
        .execute_script("SELECT a.v FROM a JOIN b ON a.id = b.id ORDER BY a.v;")
        .expect("execute auto join");
    let (_, rows) = as_rows(&selected[0]);
    assert_eq!(rows.len(), 2);

    let analyzed = engine
        .execute_script("EXPLAIN ANALYZE SELECT a.v FROM a JOIN b ON a.id = b.id;")
        .expect("explain analyze auto join");
    let plan = plan_text(&analyzed[0]);
    assert!(
        plan.contains("NestedLoop (inner) on=expr actual rows=2 time="),
        "unexpected plan: {plan}"
    );

    engine.set_join_strategy(JoinStrategy::Hash);
    let forced = engine
        .execute_script("EXPLAIN SELECT a.v FROM a JOIN b ON a.id = b.id;")
        .expect("explain forced hash");
    let plan = plan_text(&forced[0]);
    assert!(
        plan.contains("HashJoin (inner) keys=(a.id = b.id) est rows=2"),
        "unexpected plan: {plan}"
    );
}

#[test]
fn dml_invalidates_persisted_statistics_until_next_analyze() {
    let dir = TempDir::new("sql-analyze-invalidate");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT, v TEXT); \
             INSERT INTO t VALUES (1, 'a'), (2, 'b'), (3, 'c'); \
             ANALYZE t;",
        )
        .expect("seed and analyze");
    let explained = engine
        .execute_script("EXPLAIN SELECT id FROM t WHERE id >= 2;")
        .expect("explain before dml");
    let plan = as_rows(&explained[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("Filter est rows=1"),
        "unexpected plan: {plan}"
    );

    engine
        .execute_script("INSERT INTO t VALUES (4, 'd');")
        .expect("insert");
    let explained = engine
        .execute_script("EXPLAIN SELECT id FROM t WHERE id >= 2;")
        .expect("explain after dml");
    let plan = as_rows(&explained[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!plan.contains("est rows"), "unexpected plan: {plan}");
    drop(engine);

    let mut reopened = SqlEngine::open(&db).expect("reopen");
    let explained = reopened
        .execute_script("EXPLAIN SELECT id FROM t WHERE id >= 2;")
        .expect("explain after reopen");
    let plan = as_rows(&explained[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!plan.contains("est rows"), "unexpected plan: {plan}");

    let analyzed = reopened
        .execute_script("ANALYZE t;")
        .expect("analyze again");
    assert_eq!(message(&analyzed[0]), "analyzed 1 table(s)");
    let explained = reopened
        .execute_script("EXPLAIN SELECT id FROM t WHERE id >= 2;")
        .expect("explain after re-analyze");
    let plan = as_rows(&explained[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("Scan t est rows=4"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Filter est rows=2"),
        "unexpected plan: {plan}"
    );
    assert!(reopened.execute_script("ANALYZE missing;").is_err());
}

#[test]
fn natural_join_without_common_columns_is_cross_join_and_validates_using() {
    let dir = TempDir::new("sql-natural-using-errors");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE l (id INT NOT NULL, code INT); \
             CREATE TABLE x (v TEXT); \
             CREATE TABLE s (id TEXT); \
             INSERT INTO l VALUES (1, 10), (2, 20); \
             INSERT INTO x VALUES ('a'), ('b'); \
             INSERT INTO s VALUES ('1');",
        )
        .expect("seed");

    let crossed = engine
        .execute_script("SELECT COUNT(*) FROM l NATURAL JOIN x;")
        .expect("natural cross");
    assert_eq!(as_rows(&crossed[0]).1, [vec![Value::Int(4)]]);

    let missing = engine
        .execute_script("SELECT * FROM l JOIN x USING (missing);")
        .expect_err("missing using column")
        .to_string();
    assert!(missing.contains("unknown column: missing"), "{missing}");

    let duplicate = engine
        .execute_script("SELECT * FROM l JOIN l AS l2 USING (id, id);")
        .expect_err("duplicate using column")
        .to_string();
    assert!(
        duplicate.contains("duplicate USING column: id"),
        "{duplicate}"
    );

    let mismatch = engine
        .execute_script("SELECT * FROM l JOIN s USING (id);")
        .expect_err("using type mismatch")
        .to_string();
    assert!(mismatch.contains("type mismatch"), "{mismatch}");
}

#[test]
fn cross_join_composes_with_other_joins_and_group_by() {
    let dir = TempDir::new("sql-cross-compose");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE a (id INT NOT NULL, tag TEXT); \
             INSERT INTO a VALUES (1, 'x'), (2, 'y'); \
             CREATE TABLE b (id INT, aid INT); \
             INSERT INTO b VALUES (10, 1), (20, 2), (30, 2); \
             CREATE TABLE c (id INT, bid INT, name TEXT); \
             INSERT INTO c VALUES (100, 10, 'p'), (200, 20, 'q'), (300, 20, 'r');",
        )
        .expect("seed");

    let mixed = engine
        .execute_script("SELECT a.id, c.name FROM a, b JOIN c ON b.id = c.bid ORDER BY a.id, c.id;")
        .expect("comma then inner join");
    assert_eq!(
        as_rows(&mixed[0]).1,
        [
            vec![Value::Int(1), Value::Text("p".to_string())],
            vec![Value::Int(1), Value::Text("q".to_string())],
            vec![Value::Int(1), Value::Text("r".to_string())],
            vec![Value::Int(2), Value::Text("p".to_string())],
            vec![Value::Int(2), Value::Text("q".to_string())],
            vec![Value::Int(2), Value::Text("r".to_string())],
        ]
    );

    let grouped = engine
        .execute_script(
            "SELECT a.tag, COUNT(*) FROM a CROSS JOIN b WHERE b.aid = a.id \
             GROUP BY a.tag ORDER BY a.tag;",
        )
        .expect("cross join group");
    assert_eq!(
        as_rows(&grouped[0]).1,
        [
            vec![Value::Text("x".to_string()), Value::Int(1)],
            vec![Value::Text("y".to_string()), Value::Int(2)],
        ]
    );
}

#[test]
fn cross_join_handles_empty_inputs_and_validation() {
    let dir = TempDir::new("sql-cross-empty");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE a (id INT); \
             INSERT INTO a VALUES (1), (2); \
             CREATE TABLE b (id INT); \
             INSERT INTO b VALUES (10), (20); \
             CREATE TABLE e (id INT);",
        )
        .expect("seed");

    let empty_right = engine
        .execute_script("SELECT COUNT(*) FROM a CROSS JOIN e;")
        .expect("empty right");
    assert_eq!(as_rows(&empty_right[0]).1, [vec![Value::Int(0)]]);
    let empty_left = engine
        .execute_script("SELECT COUNT(*) FROM e, a, b;")
        .expect("empty first source");
    assert_eq!(as_rows(&empty_left[0]).1, [vec![Value::Int(0)]]);

    let duplicate = engine
        .execute_script("SELECT * FROM a AS x, b AS x;")
        .expect_err("duplicate alias");
    assert!(
        matches!(&duplicate, SqlError::DuplicateTableAlias(name) if name == "x"),
        "unexpected {duplicate:?}"
    );
    let unknown = engine
        .execute_script("SELECT * FROM a, missing;")
        .expect_err("unknown table");
    assert!(
        matches!(
            &unknown,
            SqlError::UnknownName {
                kind: NameKind::Table,
                name,
                ..
            } if name == "missing"
        ),
        "unexpected {unknown:?}"
    );
}

#[test]
fn batch_where_matches_row_path_on_annotation_columns() {
    let dir = TempDir::new("sql-batch-where");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, a LOGIC K3, b LOGIC BELNAP);              INSERT INTO t VALUES              (1, TRUE, TRUE),              (2, FALSE, FALSE),              (3, NULL, BOTH),              (4, TRUE, NEITHER),              (5, TRUE, FALSE);",
        )
        .expect("seed");

    let ids = |engine: &mut SqlEngine, sql: &str| -> Vec<i64> {
        let results = engine.execute_script(sql).expect("query");
        as_rows(&results[0])
            .1
            .iter()
            .map(|row| match row[0] {
                Value::Int(id) => id,
                ref other => panic!("expected int id, got {other:?}"),
            })
            .collect()
    };

    // Pure K3 column: only TRUE survives (NULL is UNKNOWN).
    assert_eq!(ids(&mut engine, "SELECT id FROM t WHERE a;"), vec![1, 4, 5]);
    assert_eq!(ids(&mut engine, "SELECT id FROM t WHERE NOT a;"), vec![2]);
    // Belnap BOTH/NEITHER are UNKNOWN in WHERE.
    assert_eq!(ids(&mut engine, "SELECT id FROM t WHERE a AND b;"), vec![1]);
    assert_eq!(
        ids(&mut engine, "SELECT id FROM t WHERE a OR b;"),
        vec![1, 4, 5]
    );
    assert_eq!(
        ids(&mut engine, "SELECT id FROM t WHERE a AND (NOT b OR a);"),
        vec![1, 4, 5]
    );
    // A non-annotation comparison keeps the row-wise path.
    assert_eq!(
        ids(&mut engine, "SELECT id FROM t WHERE a AND id > 1;"),
        vec![4, 5]
    );
}

#[test]
fn batch_where_drives_update_and_delete() {
    let dir = TempDir::new("sql-batch-dml");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, flag LOGIC K3);              INSERT INTO t VALUES (1, TRUE), (2, FALSE), (3, NULL), (4, TRUE);",
        )
        .expect("seed");

    let ids = |engine: &mut SqlEngine| -> Vec<i64> {
        let results = engine
            .execute_script("SELECT id FROM t ORDER BY id;")
            .expect("query");
        as_rows(&results[0])
            .1
            .iter()
            .map(|row| match row[0] {
                Value::Int(id) => id,
                ref other => panic!("expected int id, got {other:?}"),
            })
            .collect()
    };

    // UPDATE only touches rows where the K3 predicate is TRUE.
    engine
        .execute_script("UPDATE t SET id = id + 100 WHERE flag;")
        .expect("update");
    assert_eq!(ids(&mut engine), vec![2, 3, 101, 104]);

    // DELETE drops exactly those rows (NULL is UNKNOWN, kept).
    engine
        .execute_script("DELETE FROM t WHERE flag;")
        .expect("delete");
    assert_eq!(ids(&mut engine), vec![2, 3]);

    // A mixed predicate falls back to the row-wise path.
    engine
        .execute_script(
            "CREATE TABLE u (id INT NOT NULL, flag LOGIC K3);              INSERT INTO u VALUES (1, TRUE), (2, TRUE), (3, FALSE);              DELETE FROM u WHERE flag AND id > 1;",
        )
        .expect("mixed predicate delete");
    let remaining = engine
        .execute_script("SELECT id FROM u ORDER BY id;")
        .expect("query");
    let values: Vec<i64> = as_rows(&remaining[0])
        .1
        .iter()
        .map(|row| match row[0] {
            Value::Int(id) => id,
            ref other => panic!("expected int id, got {other:?}"),
        })
        .collect();
    assert_eq!(values, vec![1, 3]);
}

#[test]
fn asof_join_picks_the_last_matching_row() {
    let dir = TempDir::new("sql-asof");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE trades (id INT NOT NULL, ts INT NOT NULL, price INT);              CREATE TABLE quotes (ts INT NOT NULL, bid INT);              INSERT INTO trades VALUES (1, 10, 100), (2, 25, 200), (3, 40, 300);              INSERT INTO quotes VALUES (5, 50), (20, 60), (30, 70), (20, 61);",
        )
        .expect("seed");

    // The right side is unsorted on purpose: the executor sorts it (stable),
    // so the last of the equal 20s (bid 61) wins for ts = 25.
    let results = engine
        .execute_script(
            "SELECT trades.id, quotes.bid FROM trades              ASOF JOIN quotes ON trades.ts >= quotes.ts ORDER BY trades.id;",
        )
        .expect("asof");
    assert_eq!(
        as_rows(&results[0]).1,
        [
            vec![Value::Int(1), Value::Int(50)],
            vec![Value::Int(2), Value::Int(61)],
            vec![Value::Int(3), Value::Int(70)],
        ]
    );
}

#[test]
fn asof_join_handles_null_and_missing_matches() {
    let dir = TempDir::new("sql-asof-null");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE trades (id INT NOT NULL, ts INT);              CREATE TABLE quotes (ts INT, bid INT);              INSERT INTO trades VALUES (1, 3), (2, NULL), (3, 10);              INSERT INTO quotes VALUES (5, 50), (NULL, 99), (10, 100);",
        )
        .expect("seed");
    let results = engine
        .execute_script(
            "SELECT trades.id, quotes.bid FROM trades              ASOF JOIN quotes ON trades.ts >= quotes.ts ORDER BY trades.id;",
        )
        .expect("asof");
    // ts=3 has no quote <= 3 (NULL time excluded); ts=NULL never matches;
    // ts=10 takes the equal quote (100).
    assert_eq!(
        as_rows(&results[0]).1,
        [
            vec![Value::Int(1), Value::Null],
            vec![Value::Int(2), Value::Null],
            vec![Value::Int(3), Value::Int(100)],
        ]
    );
}

#[test]
fn asof_join_rejects_unsupported_conditions() {
    let dir = TempDir::new("sql-asof-errors");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE a (ts INT NOT NULL, label TEXT);              CREATE TABLE b (ts INT NOT NULL, label TEXT);",
        )
        .expect("seed");

    let error = engine
        .execute_script("SELECT * FROM a ASOF JOIN b ON a.ts = b.ts;")
        .expect_err("equality is not an as-of condition");
    assert!(
        matches!(error, SqlError::Unsupported(_)),
        "unexpected {error:?}"
    );

    let error = engine
        .execute_script("SELECT * FROM a ASOF JOIN b ON a.label >= b.label;")
        .expect_err("time columns must be INT");
    assert!(
        matches!(
            error,
            SqlError::Exec(tritdb_exec::ExecError::TypeMismatch { .. })
        ),
        "unexpected {error:?}"
    );
}

#[test]
fn asof_join_groups_by_equality_keys() {
    let dir = TempDir::new("sql-asof-multikey");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE trades (id INT NOT NULL, sym TEXT, ts INT NOT NULL, px INT);              CREATE TABLE quotes (sym TEXT, ts INT NOT NULL, bid INT);              INSERT INTO trades VALUES (1, 'AAA', 10, 100), (2, 'BBB', 10, 200),              (3, 'AAA', 25, 300), (4, 'CCC', 10, 400), (5, NULL, 10, 500);              INSERT INTO quotes VALUES ('AAA', 5, 50), ('BBB', 5, 60), ('AAA', 20, 70),              ('AAA', 20, 71), ('BBB', 20, 80), (NULL, 5, 99);",
        )
        .expect("seed");
    let results = engine
        .execute_script(
            "SELECT trades.id, quotes.bid FROM trades              ASOF JOIN quotes ON trades.sym = quotes.sym AND trades.ts >= quotes.ts              ORDER BY trades.id;",
        )
        .expect("multi-key asof");
    assert_eq!(
        as_rows(&results[0]).1,
        [
            vec![Value::Int(1), Value::Int(50)],
            vec![Value::Int(2), Value::Int(60)],
            vec![Value::Int(3), Value::Int(71)],
            vec![Value::Int(4), Value::Null],
            vec![Value::Int(5), Value::Null],
        ]
    );

    // Without a time key the ON clause is rejected.
    let error = engine
        .execute_script("SELECT * FROM trades ASOF JOIN quotes ON trades.sym = quotes.sym;")
        .expect_err("missing time key");
    assert!(
        matches!(error, SqlError::Unsupported(_)),
        "unexpected {error:?}"
    );
}

#[test]
fn join_projection_prunes_unused_source_columns() {
    let dir = TempDir::new("sql-projection");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE a (id INT NOT NULL, v TEXT, spare TEXT); \
             CREATE TABLE b (id INT NOT NULL, w TEXT, spare TEXT); \
             INSERT INTO a VALUES (1, 'a1', 'sa1'), (2, 'a2', 'sa2'), (3, 'a3', 'sa3'); \
             INSERT INTO b VALUES (1, 'b1', 'sb1'), (2, 'b2', 'sb2'), (3, 'b3', 'sb3'); \
             ANALYZE;",
        )
        .expect("seed and analyze");

    let selected = engine
        .execute_script("SELECT a.v, b.w FROM a JOIN b ON a.id = b.id WHERE a.id = 2;")
        .expect("select with projection");
    assert_eq!(
        as_rows(&selected[0]).1,
        [vec![
            Value::Text("a2".to_string()),
            Value::Text("b2".to_string())
        ]]
    );

    let explained = engine
        .execute_script("EXPLAIN SELECT a.v, b.w FROM a JOIN b ON a.id = b.id WHERE a.id = 2;")
        .expect("explain projected join");
    let plan = as_rows(&explained[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("Scan a filtered est rows=1 cols=id,v"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Scan b est rows=3 cols=id,w"),
        "unexpected plan: {plan}"
    );
    assert!(!plan.contains("spare"), "unexpected plan: {plan}");

    let grouped = engine
        .execute_script(
            "SELECT a.v, COUNT(*) AS n FROM a JOIN b ON a.id = b.id \
             GROUP BY a.v ORDER BY a.v;",
        )
        .expect("grouped projection");
    assert_eq!(
        as_rows(&grouped[0]).1,
        [
            vec![Value::Text("a1".to_string()), Value::Int(1)],
            vec![Value::Text("a2".to_string()), Value::Int(1)],
            vec![Value::Text("a3".to_string()), Value::Int(1)],
        ]
    );
    let grouped_plan = engine
        .execute_script(
            "EXPLAIN SELECT a.v, COUNT(*) AS n FROM a JOIN b ON a.id = b.id \
             GROUP BY a.v ORDER BY a.v;",
        )
        .expect("grouped explain");
    let plan = as_rows(&grouped_plan[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("Scan b est rows=3 cols=id"),
        "unexpected plan: {plan}"
    );

    let analyzed = engine
        .execute_script(
            "EXPLAIN ANALYZE SELECT a.v, b.w FROM a JOIN b ON a.id = b.id WHERE a.id = 2;",
        )
        .expect("explain analyze projected join");
    let plan = as_rows(&analyzed[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("Scan a filtered actual rows=1 cols=id,v"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Scan b actual rows=3 cols=id,w"),
        "unexpected plan: {plan}"
    );

    let wildcard = engine
        .execute_script("EXPLAIN SELECT * FROM a JOIN b ON a.id = b.id;")
        .expect("wildcard explain");
    let plan = as_rows(&wildcard[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!plan.contains("cols="), "unexpected plan: {plan}");

    let unqualified = engine
        .execute_script("EXPLAIN SELECT a.v, b.w FROM a JOIN b ON a.id = b.id WHERE v = 'a2';")
        .expect("unqualified explain");
    let plan = as_rows(&unqualified[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("cols=id,v"),
        "unambiguous unqualified column should be pruned: {plan}"
    );
    assert!(!plan.contains("spare"), "unexpected plan: {plan}");
    assert!(plan.contains("Filter"), "unexpected plan: {plan}");

    let unqualified_result = engine
        .execute_script("SELECT a.v, b.w FROM a JOIN b ON a.id = b.id WHERE v = 'a2';")
        .expect("unqualified projected join");
    assert_eq!(
        as_rows(&unqualified_result[0]).1,
        [vec![
            Value::Text("a2".to_string()),
            Value::Text("b2".to_string())
        ]]
    );

    let single_unqualified = engine
        .execute_script("EXPLAIN SELECT v FROM a;")
        .expect("single unqualified explain");
    let plan = as_rows(&single_unqualified[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("cols=v"),
        "single-source unqualified column should be pruned: {plan}"
    );
    assert!(!plan.contains("spare"), "unexpected plan: {plan}");

    let using = engine
        .execute_script("EXPLAIN SELECT a.v, b.w FROM a JOIN b USING (id);")
        .expect("using explain");
    let plan = as_rows(&using[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!plan.contains("cols="), "unexpected plan: {plan}");

    let single = engine
        .execute_script("EXPLAIN SELECT a.v FROM a WHERE a.id = 2;")
        .expect("single table explain");
    let plan = as_rows(&single[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("Scan a est rows=3 cols=id,v"),
        "unexpected plan: {plan}"
    );
}

#[test]
fn zero_column_source_projection_preserves_row_multiplicity() {
    let dir = TempDir::new("sql-zero-column");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE a (id INT NOT NULL, v TEXT, spare TEXT); \
             CREATE TABLE b (id INT NOT NULL, w TEXT, spare TEXT); \
             INSERT INTO a VALUES (1, 'a1', 'sa1'), (2, 'a2', 'sa2'), (3, 'a3', 'sa3'); \
             INSERT INTO b VALUES (1, 'b1', 'sb1'), (2, 'b2', 'sb2'), (3, 'b3', 'sb3'); \
             ANALYZE;",
        )
        .expect("seed and analyze");

    let constants = engine
        .execute_script("SELECT 1 AS one FROM a;")
        .expect("constant projection");
    let (columns, rows) = as_rows(&constants[0]);
    assert_eq!(columns, ["one"]);
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|row| row == &[Value::Int(1)]));

    let counted = engine
        .execute_script("SELECT COUNT(*) AS n FROM a;")
        .expect("count over zero-column source");
    assert_eq!(as_rows(&counted[0]).1, [vec![Value::Int(3)]]);

    let crossed = engine
        .execute_script("SELECT 1 AS one FROM a CROSS JOIN b;")
        .expect("cross join over zero-column sources");
    assert_eq!(as_rows(&crossed[0]).1.len(), 9);

    let full = engine
        .execute_script("SELECT 1 AS one FROM a FULL JOIN b ON FALSE;")
        .expect("outer join over zero-column sources");
    assert_eq!(as_rows(&full[0]).1.len(), 6);

    let explained = engine
        .execute_script("EXPLAIN SELECT 1 AS one FROM a;")
        .expect("zero-column explain");
    let plan = as_rows(&explained[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("Scan a est rows=3 cols=()"),
        "zero-column scan should be visible: {plan}"
    );

    let analyzed = engine
        .execute_script("EXPLAIN ANALYZE SELECT COUNT(*) AS n FROM a;")
        .expect("zero-column analyze");
    let plan = as_rows(&analyzed[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("Scan a actual rows=3 cols=()"),
        "zero-column analyzed scan should be visible: {plan}"
    );
}

#[test]
fn constant_folding_removes_trivial_filters_and_keeps_errors() {
    let dir = TempDir::new("sql-constant-fold");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE a (id INT NOT NULL, v TEXT); \
             CREATE TABLE b (id INT NOT NULL, w TEXT); \
             INSERT INTO a VALUES (1, 'a1'), (2, 'a2'), (3, 'a3'); \
             INSERT INTO b VALUES (1, 'b1'), (2, 'b2'), (3, 'b3'); \
             ANALYZE;",
        )
        .expect("seed and analyze");

    let all = engine
        .execute_script("SELECT a.v FROM a ORDER BY a.id;")
        .expect("select all");
    assert_eq!(as_rows(&all[0]).1.len(), 3);

    let projected = engine
        .execute_script("SELECT 1 + 1, a.id FROM a ORDER BY a.id;")
        .expect("folded projection");
    let (columns, rows) = as_rows(&projected[0]);
    assert_eq!(columns, ["col1", "id"]);
    assert_eq!(
        rows,
        [
            vec![Value::Int(2), Value::Int(1)],
            vec![Value::Int(2), Value::Int(2)],
            vec![Value::Int(2), Value::Int(3)],
        ]
    );
    let aliased = engine
        .execute_script("SELECT 1 + 1 AS two, a.id FROM a ORDER BY a.id;")
        .expect("folded aliased projection");
    assert_eq!(as_rows(&aliased[0]).0, ["two", "id"]);

    let explained = engine
        .execute_script("EXPLAIN SELECT a.v FROM a WHERE 1 = 1;")
        .expect("explain true filter");
    let plan = as_rows(&explained[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!plan.contains("Filter"), "unexpected plan: {plan}");
    assert!(
        plan.contains("Scan a est rows=3 cols=v"),
        "unexpected plan: {plan}"
    );

    let explained = engine
        .execute_script("EXPLAIN SELECT a.v FROM a WHERE 1 = 0;")
        .expect("explain false filter");
    let plan = as_rows(&explained[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("Filter est rows=0"),
        "unexpected plan: {plan}"
    );
    let none = engine
        .execute_script("SELECT a.v FROM a WHERE 1 = 0;")
        .expect("false select");
    assert!(as_rows(&none[0]).1.is_empty());

    let explained = engine
        .execute_script(
            "EXPLAIN SELECT a.v FROM a JOIN b ON a.id = b.id \
             WHERE 1 = 1 AND a.id = 2;",
        )
        .expect("explain folded join filter");
    let plan = as_rows(&explained[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!plan.contains("Filter"), "unexpected plan: {plan}");
    assert!(
        plan.contains("Scan a filtered est rows=1"),
        "unexpected plan: {plan}"
    );
    assert!(
        plan.contains("Scan b est rows=3 cols=id"),
        "unexpected plan: {plan}"
    );

    for query in [
        "EXPLAIN SELECT a.v FROM a JOIN b ON a.id = b.id WHERE a.id = 2 AND 1 = 1;",
        "EXPLAIN SELECT a.v FROM a JOIN b ON a.id = b.id WHERE 1 = 0 OR a.id = 2;",
    ] {
        let explained = engine.execute_script(query).expect("explain identity");
        let plan = as_rows(&explained[0])
            .1
            .iter()
            .map(|row| match &row[0] {
                Value::Text(line) => line.clone(),
                other => panic!("expected plan text, got {other:?}"),
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !plan.contains("Filter"),
            "unexpected plan for {query}: {plan}"
        );
        assert!(
            plan.contains("Scan a filtered est rows=1"),
            "unexpected plan for {query}: {plan}"
        );
    }

    let joined = engine
        .execute_script("SELECT a.v FROM a JOIN b ON a.id = b.id WHERE 1 = 1 AND a.id = 2;")
        .expect("folded join select");
    assert_eq!(as_rows(&joined[0]).1, [vec![Value::Text("a2".to_string())]]);

    let explained = engine
        .execute_script(
            "EXPLAIN SELECT a.id, COUNT(*) AS n FROM a \
             GROUP BY a.id HAVING 1 = 1;",
        )
        .expect("explain true having");
    let plan = as_rows(&explained[0])
        .1
        .iter()
        .map(|row| match &row[0] {
            Value::Text(line) => line.clone(),
            other => panic!("expected plan text, got {other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!plan.contains("Having"), "unexpected plan: {plan}");
    let grouped = engine
        .execute_script(
            "SELECT a.id, COUNT(*) AS n FROM a \
             GROUP BY a.id HAVING 1 = 1;",
        )
        .expect("true having select");
    assert_eq!(as_rows(&grouped[0]).1.len(), 3);

    let error = engine
        .execute_script("SELECT 1 / 0 AS bad FROM a;")
        .expect_err("projection constant error must survive folding")
        .to_string();
    assert!(error.contains("division by zero"), "{error}");

    let error = engine
        .execute_script("SELECT a.v FROM a WHERE 1 / 0 = 2;")
        .expect_err("constant error must survive folding")
        .to_string();
    assert!(error.contains("division by zero"), "{error}");

    for query in [
        "SELECT a.v FROM a WHERE TRUE OR 1 / 0 = 2;",
        "SELECT a.v FROM a WHERE FALSE AND 1 / 0 = 2;",
    ] {
        let error = engine
            .execute_script(query)
            .expect_err("non-identity boolean terms must keep errors")
            .to_string();
        assert!(
            error.contains("division by zero"),
            "unexpected error for {query}: {error}"
        );
    }
}

#[test]
fn update_without_matches_and_empty_table_reports_zero() {
    let dir = TempDir::new("sql-update-empty");
    let db = dir.join("sql.db");
    let mut engine = SqlEngine::open(&db).expect("open");
    engine
        .execute_script(
            "CREATE TABLE t (id INT, score INT); \
             INSERT INTO t VALUES (1, 10); \
             UPDATE t SET score = 1 WHERE id = 99;",
        )
        .expect("no match");
    let results = engine
        .execute_script("UPDATE t SET score = 2 WHERE id = 99;")
        .expect("second no match");
    assert_eq!(message(&results[0]), "updated 0 row(s) in t");
    let untouched = engine
        .execute_script("SELECT score FROM t;")
        .expect("select");
    assert_eq!(as_rows(&untouched[0]).1, [vec![Value::Int(10)]]);

    engine
        .execute_script("CREATE TABLE e (id INT);")
        .expect("create empty");
    let empty = engine
        .execute_script("UPDATE e SET id = 1;")
        .expect("empty update");
    assert_eq!(message(&empty[0]), "updated 0 row(s) in e");
}
