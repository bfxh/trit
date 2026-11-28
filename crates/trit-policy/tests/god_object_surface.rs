//! 上帝对象的职责面棘轮（`docs/specs/FRONTIER.md` §1.3 的机器可判部分）。
//!
//! `scripts/module_budget.py` 只管**行数**；行数只是代理指标。这里补另一维：
//! 单类型的职责面——`pub struct/enum` 的 pub 字段数、`impl` 的 `pub fn` 数。
//!
//! 台账 `scripts/god_object_surface.json`：键 `<相对路径>::type|impl::<名字>`，值是该类型
//! 当前职责面。规则与行数棘轮同口径：**只减不增**；新类型首次登记不得高于 `limit`。
//!
//! 已知局限（如实）：花括号按字符计数，字符串/注释里的花括号会干扰嵌套深度；
//! 只统计块第一层的成员，宏生成的成员看不见。它拦的是"明显膨胀"，不是完备分析。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

const SURFACE_MANIFEST: &str = "scripts/god_object_surface.json";
const SURFACE_LIMIT: usize = 32;
const SKIP_DIRS: &[&str] = &["target", ".git", "node_modules", "vendor", "__pycache__"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<crate> 之上应有仓库根")
        .to_path_buf()
}

/// 关键字后必须是空白、`<` 或 `{`，否则 `implementation` 会被当成 `impl`。
fn keyword_rest<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(keyword)?;
    let boundary = rest.is_empty()
        || rest.starts_with(|ch: char| ch.is_whitespace() || ch == '<' || ch == '{');
    boundary.then_some(rest)
}

/// 从一行头部解析出 `(是否 impl, 类型名)`；不是类型/impl 头则返回 `None`。
fn parse_head(line: &str) -> Option<(bool, String)> {
    let text = line.trim_start();
    let (is_impl, rest) = if let Some(rest) = keyword_rest(text, "impl") {
        (true, rest)
    } else {
        let rest = match text.strip_prefix("pub") {
            Some(after) if after.starts_with('(') => {
                let close = after.find(')')?;
                &after[close + 1..]
            }
            Some(after) => after,
            None => text,
        };
        let rest = rest.trim_start();
        if let Some(rest) = keyword_rest(rest, "struct") {
            (false, rest)
        } else {
            let rest = keyword_rest(rest, "enum")?;
            (false, rest)
        }
    };
    let mut rest = rest.trim_start();
    // `impl<T> Foo`：跳过泛型参数。
    if rest.starts_with('<') {
        let close = rest.find('>')?;
        rest = rest[close + 1..].trim_start();
    }
    // `impl Display for Foo`：取 `for` 之后的类型名。
    if let Some(after) = rest.split_once(" for ").map(|(_, after)| after) {
        rest = after.trim_start();
    }
    let name: String = rest
        .chars()
        .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
        .collect();
    (!name.is_empty()).then_some((is_impl, name))
}

fn is_pub_field(text: &str) -> bool {
    let Some(rest) = text.trim_start().strip_prefix("pub") else {
        return false;
    };
    if rest.starts_with('(') {
        return false; // pub(crate) 字段不算对外职责面
    }
    rest.trim_start()
        .split_once(':')
        .is_some_and(|(name, _)| !name.trim().is_empty() && !name.contains(' '))
}

fn is_pub_fn(text: &str) -> bool {
    let Some(rest) = text.trim_start().strip_prefix("pub") else {
        return false;
    };
    let rest = rest.trim_start();
    let rest = rest.strip_prefix("async ").unwrap_or(rest).trim_start();
    rest.starts_with("fn ")
}

fn scan_file(rel: &str, source: &str, caps: &mut BTreeMap<String, usize>) {
    let lines: Vec<&str> = source.lines().collect();
    let mut index = 0usize;
    while index < lines.len() {
        let Some((is_impl, name)) = parse_head(lines[index]) else {
            index += 1;
            continue;
        };
        let mut depth = 0i32;
        let mut started = false;
        let mut count = 0usize;
        let mut cursor = index;
        while cursor < lines.len() {
            let line = lines[cursor];
            let opens = line.matches('{').count() as i32;
            let closes = line.matches('}').count() as i32;
            if !started && opens > 0 {
                started = true;
                depth = opens - closes;
                cursor += 1;
                continue;
            }
            if started {
                if depth == 1 {
                    if is_impl && is_pub_fn(line) {
                        count += 1;
                    }
                    if !is_impl && is_pub_field(line) {
                        count += 1;
                    }
                }
                depth += opens - closes;
                if depth <= 0 {
                    break;
                }
            }
            cursor += 1;
        }
        let key = format!("{rel}::{}::{name}", if is_impl { "impl" } else { "type" });
        let slot = caps.entry(key).or_insert(0);
        if count > *slot {
            *slot = count;
        }
        index = cursor + 1;
    }
}

fn scan_surface(root: &Path) -> BTreeMap<String, usize> {
    let mut caps = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(path);
                }
                continue;
            }
            if !name.ends_with(".rs") {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            scan_file(&rel, &source, &mut caps);
        }
    }
    caps
}

fn read_manifest(root: &Path) -> (usize, BTreeMap<String, usize>) {
    let path = root.join(SURFACE_MANIFEST);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读不到 {}: {error}", path.display()));
    let value: Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} 不是合法 JSON: {error}", path.display()));
    let limit = value["limit"].as_u64().unwrap_or(32) as usize;
    let mut caps = BTreeMap::new();
    let object = value["caps"].as_object().expect("caps 对象");
    for (key, count) in object {
        caps.insert(key.clone(), count.as_u64().unwrap_or(0) as usize);
    }
    (limit, caps)
}

#[test]
fn surface_never_grows_past_ratchet() {
    let root = repo_root();
    let (limit, caps) = read_manifest(&root);
    let actual = scan_surface(&root);
    let mut problems = Vec::new();
    for (key, count) in &actual {
        match caps.get(key) {
            Some(cap) => {
                if count > cap {
                    problems.push(format!(
                        "GREW: {key} {count} > 台账上限 {cap}（拆职责，别抬台账）"
                    ));
                }
            }
            None => {
                if *count > limit {
                    problems.push(format!(
                        "OVER: {key} {count} > 预算 {limit} 且未登记 —— 先拆"
                    ));
                } else {
                    problems.push(format!(
                        "UNCAPPED: {key} {count} 不在台账（跑 --update 登记）"
                    ));
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "职责面棘轮判红：{}",
        problems.join("; ")
    );
}

#[test]
fn scan_counts_only_first_level_pub_members() {
    // 反证：门禁的计数器必须真的数对，否则它只是文档。
    let source = "\
pub struct Demo {
    pub a: u8,
    pub b: u8,
    hidden: u8,
}

impl Demo {
    pub fn one(&self) {}
    fn private(&self) {}
    pub async fn two(&self) {}
    pub(crate) fn scoped(&self) {}
}
";
    let mut caps = BTreeMap::new();
    scan_file("demo.rs", source, &mut caps);
    assert_eq!(caps["demo.rs::type::Demo"], 2, "只数 pub 字段");
    assert_eq!(
        caps["demo.rs::impl::Demo"], 2,
        "只数 pub fn（pub(crate) 不算）"
    );
}

#[test]
fn scan_handles_generics_traits_and_neutral_names() {
    let source = "\
impl<T> Holder<T> {
    pub fn get(&self) {}
}
impl std::fmt::Display for Holder<u8> {
    fn fmt(&self) {}
}
pub enum Mode {
    A,
    B,
}
";
    let mut caps = BTreeMap::new();
    scan_file("demo.rs", source, &mut caps);
    assert_eq!(caps["demo.rs::impl::Holder"], 1, "泛型 impl 取类型名");
    assert_eq!(
        caps["demo.rs::impl::Holder"], 1,
        "trait impl 取 for 之后的类型名"
    );
    assert_eq!(caps["demo.rs::type::Mode"], 0, "enum 变体不是 pub 字段");
}

#[test]
fn nested_pub_items_do_not_count_toward_the_outer_type() {
    let source = "\
pub struct Outer {
    pub field: u8,
}
impl Outer {
    pub fn run(&self) {
        let inner = Inner { value: 1 };
        let _ = inner.value;
    }
}
struct Inner {
    value: u8,
}
";
    let mut caps = BTreeMap::new();
    scan_file("demo.rs", source, &mut caps);
    assert_eq!(caps["demo.rs::type::Outer"], 1);
    assert_eq!(caps["demo.rs::impl::Outer"], 1, "只数 impl 第一层的 pub fn");
}

/// 台账生成器（不参与常规测试）：
/// `cargo test -p trit-policy --test god_object_surface -- --ignored --exact dump_surface_json --nocapture`
#[test]
#[ignore = "台账生成器，人工运行"]
fn dump_surface_json() {
    let root = repo_root();
    let caps = scan_surface(&root);
    let mut object = serde_json::Map::new();
    for (key, count) in &caps {
        object.insert(key.clone(), serde_json::json!(count));
    }
    let payload = serde_json::json!({
        "schema": 1,
        "limit": SURFACE_LIMIT,
        "note": "单类型职责面棘轮：key = <相对路径>::type|impl::<名字>，value = pub 字段数（struct/enum）或 pub fn 数（impl）。只减不增；新类型首次登记不得高于 limit。",
        "caps": object,
    });
    println!("{}", serde_json::to_string_pretty(&payload).unwrap());
}
