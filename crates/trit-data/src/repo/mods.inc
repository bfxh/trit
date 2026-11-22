pub mod context;
pub mod deps;
mod deps_report;
pub mod dup;
mod imports;
mod imports_lang;
mod imports_rust;
mod kind;
mod lang_map;
mod license;
mod marks;
pub mod paths;
mod paths_targets;

#[cfg(test)]
mod context_tests;
#[cfg(test)]
mod deps_tests;
#[cfg(test)]
mod dup_tests;
#[cfg(test)]
mod graph_tests;
#[cfg(test)]
mod imports_tests;
#[cfg(test)]
mod paths_tests;
#[cfg(test)]
#[path = "../repo_tests.rs"]
mod tests;
