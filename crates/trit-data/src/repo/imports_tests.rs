//! import 解析单测：静态声明与常量字符串的动态导入。

use super::imports::parse;

#[test]
fn parses_static_and_dynamic_imports() {
    let p = parse("pub mod a;
mod b;
use crate::c::{D, E};
mod tests {", "rust");
    assert!(p.known);
    assert_eq!(p.imports, vec!["a", "b", "crate::c::D", "crate::c::E"]);
    let q = parse("import os
x = importlib.import_module(\"pkg.dyn\")", "python");
    assert_eq!(q.imports, vec!["os"]);
    assert_eq!(q.dynamic, vec!["pkg.dyn"]);
    let r = parse("const m = await import('./lazy.js')", "javascript");
    assert_eq!(r.dynamic, vec!["./lazy.js"]);
}
