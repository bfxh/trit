//! Data-driven SQL semantics matrix: arithmetic, three-valued logic,
//! predicates, aggregates, joins, subqueries, CTEs, windows, constraints,
//! schema migrations and persistence.

use tritdb_exec::Value;
use tritdb_sql::{QueryResult, SqlEngine};
use tritdb_testkit::TempDir;

fn engine(name: &str) -> (TempDir, SqlEngine) {
    let dir = TempDir::new(name);
    let db = dir.join("matrix.db");
    let engine = SqlEngine::open(&db).expect("open engine");
    (dir, engine)
}

fn one(engine: &mut SqlEngine, sql: &str) -> QueryResult {
    let mut results = engine.execute_script(sql).expect("execute sql");
    assert_eq!(results.len(), 1, "expected a single statement: {sql}");
    results.remove(0)
}

fn rows_of(engine: &mut SqlEngine, sql: &str) -> Vec<Vec<Value>> {
    match one(engine, sql) {
        QueryResult::Rows { rows, .. } => rows,
        QueryResult::Message(message) => panic!("expected rows, got message: {message}"),
    }
}

fn message_of(engine: &mut SqlEngine, sql: &str) -> String {
    match one(engine, sql) {
        QueryResult::Message(message) => message,
        QueryResult::Rows { .. } => panic!("expected a message result"),
    }
}

fn int(value: i64) -> Value {
    Value::Int(value)
}

#[test]
fn arithmetic_matrix_and_null_propagation() {
    let (_dir, mut engine) = engine("matrix-arith");
    assert_eq!(
        rows_of(&mut engine, "SELECT 1 + 2, 7 - 3, 3 * 4, 8 / 2;"),
        vec![vec![int(3), int(4), int(12), int(4)]]
    );
    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT 1 + NULL, NULL * 2, NULL - 1, NULL / 2;"
        ),
        vec![vec![Value::Null, Value::Null, Value::Null, Value::Null]]
    );
    assert_eq!(
        rows_of(&mut engine, "SELECT (1 + 2) * 3, 10 / 3;"),
        vec![vec![int(9), int(3)]]
    );
}

#[test]
fn three_valued_logic_truth_table() {
    let (_dir, mut engine) = engine("matrix-3vl");
    let cases: &[(&str, Value)] = &[
        ("TRUE AND TRUE", Value::Bool(true)),
        ("TRUE AND FALSE", Value::Bool(false)),
        ("FALSE AND FALSE", Value::Bool(false)),
        ("TRUE AND NULL", Value::Null),
        ("FALSE AND NULL", Value::Bool(false)),
        ("TRUE OR FALSE", Value::Bool(true)),
        ("FALSE OR FALSE", Value::Bool(false)),
        ("TRUE OR NULL", Value::Bool(true)),
        ("FALSE OR NULL", Value::Null),
        ("NOT TRUE", Value::Bool(false)),
        ("NOT FALSE", Value::Bool(true)),
        ("NOT NULL", Value::Null),
        ("NULL = NULL", Value::Null),
        ("1 = 1", Value::Bool(true)),
        ("1 = 2", Value::Bool(false)),
        ("1 < NULL", Value::Null),
    ];
    for (expression, expected) in cases {
        let rows = rows_of(&mut engine, &format!("SELECT {expression};"));
        assert_eq!(rows, vec![vec![expected.clone()]], "expr: {expression}");
    }
}

#[test]
fn predicate_matrix_in_between_like_is_null() {
    let (_dir, mut engine) = engine("matrix-predicates");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, name TEXT, score INT); \
             INSERT INTO t VALUES (1, 'alpha', 10), (2, 'beta', 20), \
                                  (3, 'gamma', 30), (4, NULL, NULL);",
        )
        .expect("seed");

    assert_eq!(
        rows_of(&mut engine, "SELECT COUNT(*) FROM t WHERE id IN (1, 3, 9);"),
        vec![vec![int(2)]]
    );
    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT COUNT(*) FROM t WHERE id NOT IN (1, 3);"
        ),
        vec![vec![int(2)]]
    );
    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT COUNT(*) FROM t WHERE score BETWEEN 15 AND 30;"
        ),
        vec![vec![int(2)]]
    );
    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT COUNT(*) FROM t WHERE score NOT BETWEEN 15 AND 30;"
        ),
        vec![vec![int(1)]]
    );
    assert_eq!(
        rows_of(&mut engine, "SELECT COUNT(*) FROM t WHERE name LIKE 'a%';"),
        vec![vec![int(1)]]
    );
    assert_eq!(
        rows_of(&mut engine, "SELECT COUNT(*) FROM t WHERE name LIKE '%a';"),
        vec![vec![int(3)]]
    );
    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT COUNT(*) FROM t WHERE name LIKE 'b_ta';"
        ),
        vec![vec![int(1)]]
    );
    assert_eq!(
        rows_of(&mut engine, "SELECT COUNT(*) FROM t WHERE name IS NULL;"),
        vec![vec![int(1)]]
    );
    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT COUNT(*) FROM t WHERE score IS NOT NULL;"
        ),
        vec![vec![int(3)]]
    );
    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT COUNT(*) FROM t WHERE id = 1 OR id = 4;"
        ),
        vec![vec![int(2)]]
    );
}

#[test]
fn case_matrix_searched_simple_and_null() {
    let (_dir, mut engine) = engine("matrix-case");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, score INT); \
             INSERT INTO t VALUES (1, 90), (2, 70), (3, 40), (4, NULL);",
        )
        .expect("seed");

    let rows = rows_of(
        &mut engine,
        "SELECT id, \
             CASE WHEN score >= 80 THEN 'A' WHEN score >= 60 THEN 'B' ELSE 'C' END AS grade \
         FROM t ORDER BY id;",
    );
    assert_eq!(
        rows,
        vec![
            vec![int(1), Value::Text("A".into())],
            vec![int(2), Value::Text("B".into())],
            vec![int(3), Value::Text("C".into())],
            vec![int(4), Value::Text("C".into())],
        ]
    );

    // A CASE without ELSE yields NULL on the unmatched branch.
    let rows = rows_of(
        &mut engine,
        "SELECT id, CASE WHEN score >= 80 THEN 1 END FROM t ORDER BY id;",
    );
    assert_eq!(
        rows,
        vec![
            vec![int(1), int(1)],
            vec![int(2), Value::Null],
            vec![int(3), Value::Null],
            vec![int(4), Value::Null],
        ]
    );
}

#[test]
fn aggregate_matrix_including_empty_input() {
    let (_dir, mut engine) = engine("matrix-aggregates");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, score INT); \
             INSERT INTO t VALUES (1, 10), (2, NULL), (3, 30);",
        )
        .expect("seed");

    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT COUNT(*), COUNT(score), SUM(score), AVG(score), MIN(score), MAX(score) FROM t;"
        ),
        vec![vec![int(3), int(2), int(40), int(20), int(10), int(30)]]
    );

    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT COUNT(*), COUNT(score), SUM(score), MIN(score), MAX(score) \
             FROM t WHERE FALSE;"
        ),
        vec![vec![int(0), int(0), Value::Null, Value::Null, Value::Null]]
    );
}

#[test]
fn group_by_multi_column_and_having() {
    let (_dir, mut engine) = engine("matrix-group");
    engine
        .execute_script(
            "CREATE TABLE s (region TEXT, team TEXT, score INT); \
             INSERT INTO s VALUES ('eu', 'a', 10), ('eu', 'a', 5), ('eu', 'b', 20), \
                                  ('us', 'a', 7), ('us', 'a', 1);",
        )
        .expect("seed");

    let rows = rows_of(
        &mut engine,
        "SELECT region, team, COUNT(*), SUM(score) FROM s \
         GROUP BY region, team ORDER BY region, team;",
    );
    assert_eq!(
        rows,
        vec![
            vec![
                Value::Text("eu".into()),
                Value::Text("a".into()),
                int(2),
                int(15)
            ],
            vec![
                Value::Text("eu".into()),
                Value::Text("b".into()),
                int(1),
                int(20)
            ],
            vec![
                Value::Text("us".into()),
                Value::Text("a".into()),
                int(2),
                int(8)
            ],
        ]
    );

    let rows = rows_of(
        &mut engine,
        "SELECT region, SUM(score) AS total FROM s GROUP BY region \
         HAVING SUM(score) > 10 ORDER BY region;",
    );
    assert_eq!(rows, vec![vec![Value::Text("eu".into()), int(35)]]);

    let rows = rows_of(
        &mut engine,
        "SELECT region, SUM(score) AS total FROM s GROUP BY region \
         HAVING SUM(score) > 20 ORDER BY region;",
    );
    assert_eq!(rows, vec![vec![Value::Text("eu".into()), int(35)]]);
}

#[test]
fn order_by_nulls_and_limit_boundaries() {
    let (_dir, mut engine) = engine("matrix-order");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, score INT); \
             INSERT INTO t VALUES (1, 20), (2, NULL), (3, 10);",
        )
        .expect("seed");

    // NULLs sort first ascending and last descending.
    assert_eq!(
        rows_of(&mut engine, "SELECT id FROM t ORDER BY score;"),
        vec![vec![int(2)], vec![int(3)], vec![int(1)]]
    );
    assert_eq!(
        rows_of(&mut engine, "SELECT id FROM t ORDER BY score DESC;"),
        vec![vec![int(1)], vec![int(3)], vec![int(2)]]
    );
    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT id FROM t ORDER BY score DESC, id ASC LIMIT 2;"
        ),
        vec![vec![int(1)], vec![int(3)]]
    );
    assert_eq!(
        rows_of(&mut engine, "SELECT id FROM t ORDER BY id LIMIT 0;"),
        Vec::<Vec<Value>>::new()
    );
    // LIMIT beyond the row count returns everything.
    assert_eq!(
        rows_of(&mut engine, "SELECT id FROM t ORDER BY id LIMIT 100;"),
        vec![vec![int(1)], vec![int(2)], vec![int(3)]]
    );
}

fn seed_join_pair(engine: &mut SqlEngine) {
    engine
        .execute_script(
            "CREATE TABLE a (id INT NOT NULL, payload INT); \
             CREATE TABLE b (id INT NOT NULL, payload INT); \
             INSERT INTO a VALUES (1, 10), (2, 20), (3, 30); \
             INSERT INTO b VALUES (2, 200), (3, 300), (4, 400);",
        )
        .expect("seed join pair");
}

#[test]
fn join_matrix_inner_left_right_full_cross_natural_using() {
    let (_dir, mut engine) = engine("matrix-joins");
    seed_join_pair(&mut engine);

    let ids = |engine: &mut SqlEngine, sql: &str| -> Vec<i64> {
        rows_of(engine, sql)
            .into_iter()
            .map(|row| match row[0] {
                Value::Int(value) => value,
                _ => panic!("expected INT id"),
            })
            .collect()
    };

    assert_eq!(
        ids(
            &mut engine,
            "SELECT a.id FROM a JOIN b ON a.id = b.id ORDER BY a.id;"
        ),
        vec![2, 3]
    );
    assert_eq!(
        ids(
            &mut engine,
            "SELECT a.id FROM a LEFT JOIN b ON a.id = b.id ORDER BY a.id;"
        ),
        vec![1, 2, 3]
    );
    assert_eq!(
        ids(
            &mut engine,
            "SELECT b.id FROM a RIGHT JOIN b ON a.id = b.id ORDER BY b.id;"
        ),
        vec![2, 3, 4]
    );
    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT a.id, b.id FROM a FULL JOIN b ON a.id = b.id ORDER BY a.id, b.id;"
        ),
        vec![
            // The unmatched b row has a NULL a.id and sorts first.
            vec![Value::Null, int(4)],
            vec![int(1), Value::Null],
            vec![int(2), int(2)],
            vec![int(3), int(3)],
        ]
    );
    assert_eq!(
        rows_of(&mut engine, "SELECT COUNT(*) FROM a CROSS JOIN b;"),
        vec![vec![int(9)]]
    );
    // a and b share (id, payload), so NATURAL JOIN matches on both columns and
    // finds nothing; USING only merges the named column.
    assert_eq!(
        rows_of(&mut engine, "SELECT COUNT(*) FROM a NATURAL JOIN b;"),
        vec![vec![int(0)]]
    );
    assert_eq!(
        rows_of(&mut engine, "SELECT COUNT(*) FROM a JOIN b USING (id);"),
        vec![vec![int(2)]]
    );
    assert_eq!(
        rows_of(&mut engine, "SELECT COUNT(*) FROM a NATURAL LEFT JOIN b;"),
        vec![vec![int(3)]]
    );
    // With a single shared column NATURAL JOIN behaves like USING (id).
    engine
        .execute_script(
            "CREATE TABLE c (id INT NOT NULL, x INT); \
             CREATE TABLE d (id INT NOT NULL, y INT); \
             INSERT INTO c VALUES (1, 10), (2, 20); \
             INSERT INTO d VALUES (2, 200), (3, 300);",
        )
        .expect("seed natural join pair");
    assert_eq!(
        rows_of(&mut engine, "SELECT id, x, y FROM c NATURAL JOIN d;"),
        vec![vec![int(2), int(20), int(200)]]
    );
    // Unmatched LEFT JOIN rows keep NULL on the right side.
    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT b.payload FROM a LEFT JOIN b ON a.id = b.id WHERE a.id = 1;"
        ),
        vec![vec![Value::Null]]
    );
}

#[test]
fn subquery_matrix_in_exists_scalar_correlated() {
    let (_dir, mut engine) = engine("matrix-subqueries");
    seed_join_pair(&mut engine);

    let ids = |engine: &mut SqlEngine, sql: &str| -> Vec<i64> {
        rows_of(engine, sql)
            .into_iter()
            .map(|row| match row[0] {
                Value::Int(value) => value,
                _ => panic!("expected INT id"),
            })
            .collect()
    };

    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM a WHERE id IN (SELECT id FROM b) ORDER BY id;"
        ),
        vec![2, 3]
    );
    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM a WHERE id NOT IN (SELECT id FROM b) ORDER BY id;"
        ),
        vec![1]
    );
    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM a WHERE EXISTS (SELECT 1 FROM b WHERE b.id = a.id) ORDER BY id;"
        ),
        vec![2, 3]
    );
    // b holds 200/300/400, so every a payload (10..30) is below MAX and
    // below MIN.
    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM a WHERE payload < (SELECT MAX(payload) FROM b) ORDER BY id;"
        ),
        vec![1, 2, 3]
    );
    assert_eq!(
        ids(
            &mut engine,
            "SELECT id FROM a WHERE payload > (SELECT MIN(payload) FROM b) ORDER BY id;"
        ),
        Vec::<i64>::new()
    );
}

#[test]
fn cte_and_recursive_matrix() {
    let (_dir, mut engine) = engine("matrix-cte");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, payload INT); \
             INSERT INTO t VALUES (1, 10), (2, 20), (3, 30);",
        )
        .expect("seed");

    assert_eq!(
        rows_of(
            &mut engine,
            "WITH big AS (SELECT id FROM t WHERE payload > 10) \
             SELECT id FROM big ORDER BY id;"
        ),
        vec![vec![int(2)], vec![int(3)]]
    );

    assert_eq!(
        rows_of(
            &mut engine,
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 5) \
             SELECT n FROM seq ORDER BY n;"
        ),
        vec![
            vec![int(1)],
            vec![int(2)],
            vec![int(3)],
            vec![int(4)],
            vec![int(5)]
        ]
    );

    // The recursive arm only sees the previous iteration's rows.
    assert_eq!(
        rows_of(
            &mut engine,
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 3) \
             SELECT COUNT(*) FROM seq;"
        ),
        vec![vec![int(3)]]
    );
}

#[test]
fn window_function_matrix() {
    let (_dir, mut engine) = engine("matrix-window");
    engine
        .execute_script(
            "CREATE TABLE w (id INT NOT NULL, v INT); \
             INSERT INTO w VALUES (1, 10), (2, 40), (3, 20), (4, 30);",
        )
        .expect("seed");

    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT id, ROW_NUMBER() OVER (ORDER BY v) AS rn FROM w ORDER BY id;"
        ),
        vec![
            vec![int(1), int(1)],
            vec![int(2), int(4)],
            vec![int(3), int(2)],
            vec![int(4), int(3)],
        ]
    );
    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT id, RANK() OVER (ORDER BY v) AS r FROM w ORDER BY id;"
        ),
        vec![
            vec![int(1), int(1)],
            vec![int(2), int(4)],
            vec![int(3), int(2)],
            vec![int(4), int(3)],
        ]
    );
    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT id, SUM(v) OVER (ORDER BY id) AS running FROM w ORDER BY id;"
        ),
        vec![
            vec![int(1), int(10)],
            vec![int(2), int(50)],
            vec![int(3), int(70)],
            vec![int(4), int(100)],
        ]
    );
    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT id, LAG(id) OVER (ORDER BY id) AS prev FROM w ORDER BY id;"
        ),
        vec![
            vec![int(1), Value::Null],
            vec![int(2), int(1)],
            vec![int(3), int(2)],
            vec![int(4), int(3)],
        ]
    );
    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT id, LEAD(id) OVER (ORDER BY id) AS next FROM w ORDER BY id;"
        ),
        vec![
            vec![int(1), int(2)],
            vec![int(2), int(3)],
            vec![int(3), int(4)],
            vec![int(4), Value::Null],
        ]
    );
}

#[test]
fn constraint_matrix_not_null_unique_check() {
    let (_dir, mut engine) = engine("matrix-constraints");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, email TEXT UNIQUE, score INT CHECK (score >= 0));",
        )
        .expect("create constrained table");

    engine
        .execute_script("INSERT INTO t VALUES (1, 'a', 10), (2, 'b', NULL);")
        .expect("valid rows");

    assert!(
        engine
            .execute_script("INSERT INTO t VALUES (NULL, 'c', 1);")
            .is_err(),
        "NOT NULL must reject NULL"
    );
    assert!(
        engine
            .execute_script("INSERT INTO t VALUES (3, 'a', 1);")
            .is_err(),
        "UNIQUE must reject duplicates"
    );
    assert!(
        engine
            .execute_script("INSERT INTO t VALUES (3, 'c', -1);")
            .is_err(),
        "CHECK must reject FALSE"
    );
    // UNKNOWN passes CHECK.
    engine
        .execute_script("INSERT INTO t VALUES (3, 'c', NULL);")
        .expect("UNKNOWN passes CHECK");

    assert!(
        engine
            .execute_script("UPDATE t SET email = 'a' WHERE id = 2;")
            .is_err(),
        "UPDATE must respect UNIQUE"
    );
}

#[test]
fn schema_migration_matrix() {
    let (_dir, mut engine) = engine("matrix-migration");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, name TEXT); \
             INSERT INTO t VALUES (1, 'x');",
        )
        .expect("seed");

    assert!(message_of(&mut engine, "ALTER TABLE t ADD COLUMN extra INT;").contains("extra"));
    assert_eq!(
        rows_of(&mut engine, "SELECT id, extra FROM t;"),
        vec![vec![int(1), Value::Null]]
    );

    assert!(message_of(&mut engine, "ALTER TABLE t DROP COLUMN extra;").contains("extra"));
    assert_eq!(
        rows_of(&mut engine, "SELECT COUNT(*) FROM t;"),
        vec![vec![int(1)]]
    );

    assert!(
        message_of(&mut engine, "ALTER TABLE t RENAME COLUMN name TO label;").contains("label")
    );
    assert_eq!(
        rows_of(&mut engine, "SELECT label FROM t;"),
        vec![vec![Value::Text("x".into())]]
    );

    assert!(message_of(&mut engine, "ALTER TABLE t ALTER COLUMN id TYPE TEXT;").contains("TEXT"));
    assert_eq!(
        rows_of(&mut engine, "SELECT id FROM t;"),
        vec![vec![Value::Text("1".into())]]
    );

    assert!(message_of(&mut engine, "ALTER TABLE t RENAME TO renamed;").contains("renamed"));
    assert_eq!(
        rows_of(&mut engine, "SELECT COUNT(*) FROM renamed;"),
        vec![vec![int(1)]]
    );

    assert!(
        engine
            .execute_script("ALTER TABLE renamed ALTER COLUMN label TYPE INT;")
            .is_err(),
        "text that is not a number cannot become INT"
    );
}

#[test]
fn dml_matrix_insert_select_update_delete() {
    let (_dir, mut engine) = engine("matrix-dml");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, v INT); \
             CREATE TABLE src (id INT NOT NULL, v INT); \
             INSERT INTO t VALUES (1, 10), (2, 20); \
             INSERT INTO src VALUES (3, 30), (4, 40);",
        )
        .expect("seed");

    engine
        .execute_script("INSERT INTO t SELECT id, v FROM src;")
        .expect("insert select");
    assert_eq!(
        rows_of(&mut engine, "SELECT id FROM t ORDER BY id;"),
        vec![vec![int(1)], vec![int(2)], vec![int(3)], vec![int(4)]]
    );

    // Assignments read the old row, so `v = v + 1` is not chained.
    engine
        .execute_script("UPDATE t SET v = v + 1 WHERE id < 3;")
        .expect("update");
    assert_eq!(
        rows_of(&mut engine, "SELECT id, v FROM t ORDER BY id;"),
        vec![
            vec![int(1), int(11)],
            vec![int(2), int(21)],
            vec![int(3), int(30)],
            vec![int(4), int(40)],
        ]
    );

    engine
        .execute_script("DELETE FROM t WHERE id = 1;")
        .expect("delete");
    assert_eq!(
        rows_of(&mut engine, "SELECT id FROM t ORDER BY id;"),
        vec![vec![int(2)], vec![int(3)], vec![int(4)]]
    );

    engine.execute_script("DROP TABLE src;").expect("drop");
    assert!(
        engine.execute_script("SELECT * FROM src;").is_err(),
        "dropped table must be gone"
    );
}

#[test]
fn persistence_matrix_reopen_keeps_tables_rows_and_constraints() {
    let dir = TempDir::new("matrix-persist");
    let db = dir.join("matrix.db");
    {
        let mut engine = SqlEngine::open(&db).expect("open");
        engine
            .execute_script(
                "CREATE TABLE t (id INT NOT NULL, email TEXT UNIQUE); \
                 INSERT INTO t VALUES (1, 'a'), (2, 'b');",
            )
            .expect("seed");
    }
    {
        let mut engine = SqlEngine::open(&db).expect("reopen");
        assert_eq!(
            rows_of(&mut engine, "SELECT id, email FROM t ORDER BY id;"),
            vec![
                vec![int(1), Value::Text("a".into())],
                vec![int(2), Value::Text("b".into())],
            ]
        );
        assert!(
            engine
                .execute_script("INSERT INTO t VALUES (3, 'a');")
                .is_err(),
            "UNIQUE must survive reopen"
        );
    }
}

#[test]
fn error_matrix_unknown_table_column_and_type() {
    let (_dir, mut engine) = engine("matrix-errors");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, name TEXT); INSERT INTO t VALUES (1, 'x');",
        )
        .expect("seed");

    assert!(
        engine.execute_script("SELECT * FROM missing;").is_err(),
        "unknown table must fail"
    );
    assert!(
        engine.execute_script("SELECT nope FROM t;").is_err(),
        "unknown column must fail"
    );
    assert!(
        engine
            .execute_script("INSERT INTO t (id) VALUES ('not a number');")
            .is_err(),
        "INT column must reject text"
    );
    assert!(
        engine.execute_script("CREATE TABLE t (id INT);").is_err(),
        "duplicate table must fail"
    );
    assert!(
        engine
            .execute_script("SELECT id FROM t JOIN t ON t.id = t.id;")
            .is_err(),
        "duplicate table alias must fail"
    );
}

#[test]
fn text_and_boolean_value_matrix() {
    let (_dir, mut engine) = engine("matrix-values");
    engine
        .execute_script(
            "CREATE TABLE t (id INT NOT NULL, name TEXT, flag BOOL); \
             INSERT INTO t VALUES (1, 'a', TRUE), (2, '', FALSE), (3, NULL, NULL);",
        )
        .expect("seed");

    assert_eq!(
        rows_of(&mut engine, "SELECT id, name, flag FROM t ORDER BY id;"),
        vec![
            vec![int(1), Value::Text("a".into()), Value::Bool(true)],
            vec![int(2), Value::Text("".into()), Value::Bool(false)],
            vec![int(3), Value::Null, Value::Null],
        ]
    );
    // Column aliases are usable in ORDER BY and projection.
    assert_eq!(
        rows_of(
            &mut engine,
            "SELECT id AS k, name AS n FROM t WHERE id = 1;"
        ),
        vec![vec![int(1), Value::Text("a".into())]]
    );
    assert_eq!(
        rows_of(&mut engine, "SELECT COUNT(*) FROM t WHERE flag;"),
        vec![vec![int(1)]]
    );
}
