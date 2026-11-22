//! 测试目标匹配单测：JUnit 的 `*Test` / `*IT` 与 `src/test` → `src/main` 映射。

use std::collections::BTreeSet;

use super::deps_tests::unit;
use super::paths_targets::targets;

fn paths(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn junit_tests_map_to_main_sources() {
    let set = paths(&[
        "src/main/java/com/acme/Foo.java",
        "src/main/java/com/acme/Bar.java",
        "src/main/kotlin/com/acme/Baz.kt",
        "src/test/java/com/acme/FooTest.java",
        "src/test/java/com/acme/BarIT.java",
        "src/test/kotlin/com/acme/BazTest.kt",
    ]);
    assert_eq!(
        targets(&unit("src/test/java/com/acme/FooTest.java", "java", &[], true), &set),
        vec!["src/main/java/com/acme/Foo.java".to_string()]
    );
    assert_eq!(
        targets(&unit("src/test/java/com/acme/BarIT.java", "java", &[], true), &set),
        vec!["src/main/java/com/acme/Bar.java".to_string()]
    );
    assert_eq!(
        targets(&unit("src/test/kotlin/com/acme/BazTest.kt", "kotlin", &[], true), &set),
        vec!["src/main/kotlin/com/acme/Baz.kt".to_string()]
    );
}

#[test]
fn junit_spec_and_itcase_suffixes_are_stripped() {
    let set = paths(&[
        "src/main/java/com/acme/Quux.java",
        "src/test/java/com/acme/QuuxSpec.java",
        "src/test/java/com/acme/QuuxITCase.java",
    ]);
    let spec = unit("src/test/java/com/acme/QuuxSpec.java", "java", &[], true);
    let itcase = unit("src/test/java/com/acme/QuuxITCase.java", "java", &[], true);
    assert_eq!(targets(&spec, &set), vec!["src/main/java/com/acme/Quux.java".to_string()]);
    assert_eq!(targets(&itcase, &set), vec!["src/main/java/com/acme/Quux.java".to_string()]);
}
