use super::*;
pub fn route(argv: &[String]) {
    match argv.first().map(String::as_str) {
        Some("gate") => gate::run(&argv[1..]),
        Some("rewrite") => rewrite::run(&argv[1..]),
        Some("label") => label::run(&argv[1..]),
        Some("compare") => compare::run(&argv[1..]),
        Some("decontaminate") => decontaminate::run(&argv[1..]),
        Some("mix") => mix::run(&argv[1..]),
        Some("project") => project::run(&argv[1..]),
        Some("lang") => lang::run(&argv[1..]),
        Some("repo") => repo::run(&argv[1..]),
        Some("pipeline") => pipeline::run(&argv[1..]),
        _ => usage(),
    }
}
