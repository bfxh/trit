use tritdb_sql::parse_script;
use tritdb_testkit::Rng;

#[test]
fn parser_survives_hostile_input() {
    let corpus = [
        "",
        ";;;",
        "SELECT (((((1)))));",
        "SELECT 'unterminated",
        "SELECT \"unterminated",
        "SELECT /* unterminated",
        "SELECT 999999999999999999999999999;",
        "SELECT a FROM t WHERE id IN (1, 2, 3,);",
        "SELECT a FROM t WHERE id BETWEEN 1 AND;",
        "SELECT CASE WHEN THEN 1 END FROM t;",
        "INSERT INTO t VALUES (1,);",
        "SELECT a FROM t JOIN t ON;",
    ];
    for sql in corpus {
        let _ = parse_script(sql);
    }

    let deep = format!("SELECT {}1{};", "(".repeat(512), ")".repeat(512));
    let error = parse_script(&deep).expect_err("deep nesting must be rejected");
    assert!(format!("{error:?}").contains("too deep"), "{error:?}");

    let alphabet: &[char] = &[
        'S', 'E', 'L', 'C', 'T', 'F', 'R', 'O', 'M', 'W', 'H', 'E', 'R', 'I', 'N', 'J', 'O', 'A',
        'N', 'D', 'B', 'Y', 'G', 'P', 'U', 's', 'e', 'l', 'c', 't', 'a', 'b', 'c', '0', '1', '2',
        '9', ' ', '\n', '\t', '(', ')', ',', '.', ';', '\'', '"', '=', '<', '>', '+', '-', '*',
        '/', '_',
    ];
    let mut rng = Rng::new(0x5eed_5eed_5eed_5eed);
    for _ in 0..2000 {
        let len = rng.below(96) as usize;
        let mut sql = String::with_capacity(len);
        for _ in 0..len {
            sql.push(alphabet[rng.below(alphabet.len() as u32) as usize]);
        }
        let _ = parse_script(&sql);
    }
}
