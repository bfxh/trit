use super::{ensure_test_runner, parse_assertions};

fn argv(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

#[test]
fn cargo_summary_counts_every_test_binary() {
    let text = "\
running 3 tests
test a ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

running 2 tests
test b ... FAILED
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
";
    assert_eq!(parse_assertions(text), (4, 1));
}

#[test]
fn pytest_node_go_and_dotnet_summaries_are_recognised() {
    assert_eq!(parse_assertions("2 passed, 1 failed in 0.12s"), (2, 1));
    assert_eq!(parse_assertions("# pass 5\n# fail 2"), (5, 2));
    assert_eq!(parse_assertions("ℹ pass 2\nℹ fail 0"), (2, 0));
    assert_eq!(
        parse_assertions("Summary [ 0.12s] 5 tests run: 4 passed, 1 failed, 0 skipped"),
        (4, 1)
    );
    assert_eq!(
        parse_assertions("--- PASS: TestA\n--- PASS: TestB\n--- FAIL: TestC"),
        (2, 1)
    );
    assert_eq!(
        parse_assertions("Passed!  - Failed: 0, Passed: 7, Skipped: 0"),
        (7, 0)
    );
}

#[test]
fn freeform_text_does_not_become_evidence() {
    assert_eq!(parse_assertions("all good, 100 passed, honest"), (0, 0));
    assert_eq!(parse_assertions("not a test result: 42 passed"), (0, 0));
    assert_eq!(parse_assertions(""), (0, 0));
}

#[test]
fn only_test_runner_shapes_are_allowed() {
    assert!(ensure_test_runner("a", "cargo", &argv(&["test"])).is_ok());
    assert!(ensure_test_runner("a", "cargo", &argv(&["nextest", "run"])).is_ok());
    assert!(ensure_test_runner("a", "go", &argv(&["test", "./..."])).is_ok());
    assert!(ensure_test_runner("a", "node", &argv(&["--test"])).is_ok());
    assert!(ensure_test_runner("a", "python", &argv(&["-m", "pytest"])).is_ok());
    assert!(ensure_test_runner("a", "dotnet", &argv(&["test"])).is_ok());

    assert!(ensure_test_runner("a", "cargo", &argv(&["run"])).is_err());
    assert!(ensure_test_runner("a", "./node", &argv(&["--test"])).is_err());
    assert!(ensure_test_runner("a", "echo", &argv(&["test result: ok. 99 passed"])).is_err());
    assert!(ensure_test_runner("a", "sh", &argv(&["-c", "cargo test"])).is_err());
    assert!(ensure_test_runner("a", "python", &argv(&["-c", "print('1 passed')"])).is_err());
}
