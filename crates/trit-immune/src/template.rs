//! 默认项目模板：高维护性脚手架——**无上帝对象从第一天开始，且机器可查**。
//!
//! 两件事：
//!
//! 1. [`scaffold`]：生成新项目骨架（纯函数——返回 `(相对路径, 内容)` 列表，
//!    落盘由调用方决定）。模板文件本身示范纪律：小模块、单一职责、
//!    每文件一句话定位。
//! 2. [`audit_discipline`]：**反上帝对象机械检查**——单文件行数上限、
//!    单文件公开项上限。这不是风格建议，是可执行的判据（与
//!    `dag::design_smells` 同思想：机械判据替人眼挡噪音）。
//!    承诺：`scaffold` 生成物**必须**通过 [`audit_discipline`]——测试钉死。
//!
//! 违规是 **0 态候选**（设计层待审），不是 -1：超限只是"值得看一眼"。

use std::fmt;

/// 维护性纪律参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Discipline {
    /// 单文件最大行数（超过 = 上帝文件候选）。
    pub max_file_lines: usize,
    /// 单文件最大公开项（`pub fn`/`pub struct`/`pub enum`/`pub trait` 总数；
    /// 超过 = 上帝模块候选）。
    pub max_pub_items: usize,
}

impl Default for Discipline {
    fn default() -> Self {
        Self {
            max_file_lines: 400,
            max_pub_items: 15,
        }
    }
}

/// 一条纪律违规（0 态候选）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisciplineViolation {
    pub path: String,
    pub kind: ViolationKind,
    /// 实测值 / 上限。
    pub actual: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationKind {
    GodFile,
    GodModule,
}

impl fmt::Display for ViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ViolationKind::GodFile => write!(f, "上帝文件（行数超限）"),
            ViolationKind::GodModule => write!(f, "上帝模块（公开项超限）"),
        }
    }
}

/// 反上帝对象机械检查：对 `(相对路径, 源码文本)` 逐文件数行数与公开项。
/// 只认 `.rs` 文件；其它文件（MD/TOML）只查行数。
pub fn audit_discipline(files: &[(String, String)], d: &Discipline) -> Vec<DisciplineViolation> {
    let mut out = Vec::new();
    for (path, content) in files {
        let lines = content.lines().count();
        if lines > d.max_file_lines {
            out.push(DisciplineViolation {
                path: path.clone(),
                kind: ViolationKind::GodFile,
                actual: lines,
                limit: d.max_file_lines,
            });
        }
        if path.ends_with(".rs") && lines > 0 {
            let pub_items = content
                .lines()
                .filter(|l| {
                    let t = l.trim_start();
                    t.starts_with("pub fn ")
                        || t.starts_with("pub struct ")
                        || t.starts_with("pub enum ")
                        || t.starts_with("pub trait ")
                })
                .count();
            if pub_items > d.max_pub_items {
                out.push(DisciplineViolation {
                    path: path.clone(),
                    kind: ViolationKind::GodModule,
                    actual: pub_items,
                    limit: d.max_pub_items,
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// 生成新项目骨架：`(相对路径, 内容)` 列表（纯函数，不落盘）。
/// 模板自带的目标（测试钉死）：每个文件单一职责、体量小、通过自身纪律检查。
pub fn scaffold(name: &str) -> Vec<(String, String)> {
    assert!(!name.is_empty(), "项目名不能为空");
    let n = name.to_string();
    let mut files: Vec<(String, String)> = Vec::new();

    files.push((
        "Cargo.toml".into(),
        format!(
            r#"[package]
name = "{n}"
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"

[lib]
path = "src/lib.rs"
"#
        ),
    ));

    files.push((
        "README.md".into(),
        format!(
            r#"# {n}

一句话定位：（这里写一句话，写不出来说明还没想清楚，先别建仓。）

## 状态

- [ ] 核心（`src/lib.rs` 只做装配，不堆实现）
- [ ] 测试先行（`tests/` 与 `src/` 同长）
"#
        ),
    ));

    files.push((
        "docs/GOAL.md".into(),
        "# 目标\n\n病灶一句话 + 三药方一句话 + 度量（只许单向改善）。\n".into(),
    ));

    files.push((
        "docs/ROADMAP.md".into(),
        "# 路线图\n\n| # | 事项 | 状态 | 落点 |\n|---|------|------|------|\n| 1 | （第一件小事） | 🔜 | `src/` |\n".into(),
    ));

    files.push((
        "docs/UPSTREAM.md".into(),
        // 上游收录规则骨架：与 trit-immune 的 docs/UPSTREAM.md 同一纪律
        "# 上游收录规则\n\n> 定位一句话：（你的项目一句话——写不出来说明还没想清楚）\n>\n> 内核 = 给上游生态的；工具集 = 给本工具用户的。`--no-default-features`\n> 编译通过 = 上游收录形态（feature 门控钉死边界，不靠人记）。\n\n## 收录三分类\n\n| 层 | 模块 | 立场 |\n|---|---|---|\n| 内核（必收） | （删掉它项目就不完整的模块） | ✅ |\n| 工具（不收） | （人体工程/集成面：CLI、server、桥接器……） | ❌ |\n| 特例 | （本来就可选的 feature） | 按上下文 |\n\n## 选型哲学：性价比中性\n\n技术选型看性价比，不看出身：哪个进制/编码/算法好用用哪个——\n5-trit、4-trit、二进制位平面都是参数。检验问题：**这个设计换成\n二进制还成立吗？** 成立且更省 → 用更省的；不成立（语义必需）→\n这才是它该在的地方。\n\n## 检验清单\n\n- [ ] 新模块落在哪层？（删掉它内核还完整吗）\n- [ ] 内核形态编译过吗？（--no-default-features）\n- [ ] 新依赖零依赖吗？\n- [ ] 性价比检验过吗？\n".into(),
    ));

    files.push((
        "src/lib.rs".into(),
        format!(
            r#"//! {n} —— 一句话定位。
//!
//! `lib.rs` 只做模块装配与再导出；实现进各自小模块（单一职责，
//! 无上帝对象：文件小、公开项少、依赖单向）。

pub mod core_logic;
pub mod storage;

pub use core_logic::Verdict;
pub use storage::Store;
"#
        ),
    ));

    files.push((
        "src/core_logic.rs".into(),
        r#"//! 核心逻辑：一个小而完整的领域单元（示范：三值裁决）。
//!
//! 纪律：文件短、公开项少、不 import 兄弟模块（依赖单向：lib.rs 往下）。

/// 三值裁决：-1 坏消息 / 0 未决 / +1 通过。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Bad,
    Pending,
    Good,
}

impl Verdict {
    /// K3 AND：坏消息支配。
    pub fn and(self, other: Verdict) -> Verdict {
        use Verdict::*;
        match (self, other) {
            (Bad, _) | (_, Bad) => Bad,
            (Pending, _) | (_, Pending) => Pending,
            (Good, Good) => Good,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_dominates() {
        assert_eq!(Verdict::Good.and(Verdict::Bad), Verdict::Bad);
        assert_eq!(Verdict::Good.and(Verdict::Good), Verdict::Good);
    }
}
"#
        .into(),
    ));

    files.push((
        "src/storage.rs".into(),
        r#"//! 存储：追加式日志的最小骨架（示范：只增不改 + 重放）。
//!
//! 与 `core_logic` 无横向依赖——依赖单向从 lib.rs 装配。

/// 追加式存储（占位：接你的真实持久层）。
#[derive(Debug, Default)]
pub struct Store {
    lines: Vec<String>,
}

impl Store {
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一行（只增不改）。
    pub fn append(&mut self, line: String) {
        self.lines.push(line);
    }

    /// 重放全部（顺序即状态）。
    pub fn replay(&self) -> &[String] {
        &self.lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_replay_roundtrip() {
        let mut s = Store::new();
        s.append("a".into());
        s.append("b".into());
        assert_eq!(s.replay(), &["a".to_string(), "b".to_string()]);
    }
}
"#
        .into(),
    ));

    files.push((
        "tests/smoke.rs".into(),
        format!(
            r#"//! 冒烟测试：骨架可用性（新文件先过这里再长功能）。

#[test]
fn scaffold_compiles_and_core_works() {{
    use {n}::Verdict;
    assert_eq!(Verdict::Pending.and(Verdict::Good), Verdict::Pending);
}}
"#
        ),
    ));

    files.push((
        ".gitignore".into(),
        "/target\n*.journal\n*.backup\n.mimosa/\n".into(),
    ));

    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_passes_own_discipline() {
        // 模板的无上帝对象承诺是可测的：生成物必须过自身检查
        let files = scaffold("demo-app");
        let violations = audit_discipline(&files, &Discipline::default());
        assert!(violations.is_empty(), "模板自身违规：{violations:?}");
    }

    #[test]
    fn discipline_catches_god_file_and_god_module() {
        let big = (0..500)
            .map(|i| format!("fn filler{i}() {{}}"))
            .collect::<Vec<_>>()
            .join("\n");
        let god_mod = (0..20)
            .map(|i| format!("pub fn f{i}() {{}}"))
            .collect::<Vec<_>>()
            .join("\n");
        let files = vec![
            ("src/big.rs".to_string(), big),
            ("src/god.rs".to_string(), god_mod),
        ];
        let v = audit_discipline(&files, &Discipline::default());
        assert_eq!(v.len(), 2);
        assert!(v
            .iter()
            .any(|x| x.kind == ViolationKind::GodFile && x.path == "src/big.rs"));
        assert!(v
            .iter()
            .any(|x| x.kind == ViolationKind::GodModule && x.actual == 20));
    }

    #[test]
    fn scaffold_shape_is_modular() {
        let files = scaffold("demo");
        let paths: Vec<&String> = files.iter().map(|(p, _)| p).collect();
        // 装配层 + 两个小模块 + 测试 + 文档（含上游收录规则骨架）
        assert!(paths.contains(&&"src/lib.rs".to_string()));
        assert!(paths.contains(&&"src/core_logic.rs".to_string()));
        assert!(paths.contains(&&"src/storage.rs".to_string()));
        assert!(paths.contains(&&"tests/smoke.rs".to_string()));
        assert!(paths.contains(&&"docs/GOAL.md".to_string()));
        assert!(
            paths.contains(&&"docs/UPSTREAM.md".to_string()),
            "收录规则随模板走"
        );
    }
}
