//! 最小正则引擎（Pike VM）——YARA 正则子集的**官方语义**实现。
//!
//! 语义全部来自官方探针实测（yara-python 4.5.4，2026-11-12）：
//! ①扫描起点 = `0..len`（EOF 处的空匹配/词边界永不触发——/a*/ 在
//!   "xaax" 报 0..3 四个起点、无起点 4）；
//! ②每起点**至多一命中**：回溯优先序的首完成（贪婪=长优先、交替=
//!   分支序、懒惰=短优先）——/a|ab/ 在 "ab" 报 "a" 不是 POSIX 最长；
//! ③空命中如实上报（/a*/ 在 "xaax" 报 (0,"") 与 (3,"")）；
//! ④锚点 `^`/`$` = 文件首/尾断言（非行）；`\b` = 词边界零宽断言
//!   （词字符 = \w 集）；`.` = 除 `\n` 外任意字节。
//! ⑤`(?:…)` 官方拒绝（语法错）——不支持；wide 正则语义发散（探针
//!   X/Y MISS）——v1 记名弃权。
//!
//! Pike VM（Russ Cox）：线程按优先序排列，首到达 Match 的线程终结
//! 本次起点匹配——线性时间保 DFS 优先序，无回溯爆炸面（SCAN_BUDGET
//! 兜底步进上限，耗尽 = 扫描不完整 = Unknown）。

use super::Pattern;
use super::Unscannable;

/// 单字节匹配器（字节集 + 否定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ByteSet {
    /// 除 `\n` 外任意字节（`.`）。
    AnyButNl,
    /// 显式字节集（类 / 转义类 / 单字节）。`negated` = 取补。
    Set {
        negated: bool,
        set: Box<[bool; 256]>,
    },
}

impl ByteSet {
    fn matches(&self, b: u8, nocase: bool) -> bool {
        let b = if nocase { b.to_ascii_lowercase() } else { b };
        match self {
            ByteSet::AnyButNl => b != b'\n',
            ByteSet::Set { negated, set } => set[b as usize] != *negated,
        }
    }
}

/// AST 节点。
enum Ast {
    Byte(ByteSet),
    /// 零宽断言：文件首 / 文件尾 / 词边界（`\b`）。
    Assert(AssertKind),
    Concat(Vec<Ast>),
    Alt(Vec<Ast>),
    /// 重复：min/max（max=None = 无上界）、greedy = 长优先。
    Repeat {
        node: Box<Ast>,
        min: u32,
        max: Option<u32>,
        greedy: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssertKind {
    FileStart,
    FileEnd,
    WordB,
}

/// 编译产物（进 `Pattern::Regex`，随 Pattern 派生 Clone/PartialEq/Eq）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexPat {
    pub(crate) prog: Vec<Instr>,
    pub(crate) nocase: bool,
}

/// VM 指令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Instr {
    Byte(ByteSet),
    Assert(AssertKind),
    Split(usize, usize), // 优先臂 first，次臂 second
    Jmp(usize),
    Match,
}

// ── 解析器（递归下降）：alt → concat → repeat → atom ─────────────────
mod depth; // 分组嵌套闸（递归下降的爆栈面）
/// NFA 指令数上限（{n,m} 展开与嵌套量词的爆炸闸）。
const MAX_PROG: usize = 8192;

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

/// 解析正则体（不含两端 `/`）为 AST；不支持的语法如实 `Err`。
fn parse(body: &str) -> Result<Ast, String> {
    let mut p = Parser {
        src: body.as_bytes(),
        pos: 0,
    };
    let ast = p.parse_alt()?;
    if p.pos != p.src.len() {
        return Err(format!("正则语法不支持（残留 @ {}）", p.pos));
    }
    Ok(ast)
}

impl<'a> Parser<'a> {
    fn eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    /// `|` 分隔的交替；空交替分支（`(a|)`）= 合法空分支。
    fn parse_alt(&mut self) -> Result<Ast, String> {
        let mut branches = vec![self.parse_concat()?];
        while self.peek() == Some(b'|') {
            self.pos += 1;
            branches.push(self.parse_concat()?);
        }
        Ok(match branches.len() {
            1 => branches.pop().expect("len>0"),
            _ => Ast::Alt(branches),
        })
    }

    /// 串接（空串 = 空 Concat——合法，匹配空串）；在 `|` 或组闭合 `)`
    /// 处停（组内容不得吞掉闭合括号——单元测试当场抓的坑）。
    fn parse_concat(&mut self) -> Result<Ast, String> {
        let mut items = Vec::new();
        while !self.eof() && self.peek() != Some(b'|') && self.peek() != Some(b')') {
            items.push(self.parse_repeat()?);
        }
        Ok(match items.len() {
            0 => Ast::Concat(Vec::new()),
            1 => items.pop().expect("len>0"),
            _ => Ast::Concat(items),
        })
    }

    /// 重复后缀：`*` `+` `?` `{n}` `{n,}` `{n,m}`（懒惰后缀 `?`）。
    fn parse_repeat(&mut self) -> Result<Ast, String> {
        let mut node = self.parse_atom()?;
        loop {
            let (min, max) = match self.peek() {
                Some(b'*') => {
                    self.pos += 1;
                    (0, None)
                }
                Some(b'+') => {
                    self.pos += 1;
                    (1, None)
                }
                Some(b'?') => {
                    self.pos += 1;
                    (0, Some(1))
                }
                Some(b'{') => {
                    self.pos += 1;
                    let (min, max) = self.parse_bound()?;
                    (min, max)
                }
                _ => break,
            };
            let greedy = if self.peek() == Some(b'?') {
                self.pos += 1;
                false
            } else {
                true
            };
            node = Ast::Repeat {
                node: Box::new(node),
                min,
                max,
                greedy,
            };
        }
        Ok(node)
    }

    /// `{n}` / `{n,}` / `{n,m}`（已消费 `{`）。逗号缺失 = 字面 `{`？
    /// 官方 YARA 把裸 `{` 当语法错——如实报错。
    fn parse_bound(&mut self) -> Result<(u32, Option<u32>), String> {
        let min = self.parse_int()?;
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok((min, Some(min)));
        }
        if self.peek() != Some(b',') {
            return Err("量词 { 缺 }".into());
        }
        self.pos += 1;
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok((min, None));
        }
        let max = self.parse_int()?;
        if self.peek() != Some(b'}') {
            return Err("量词缺 }".into());
        }
        self.pos += 1;
        if max < min {
            return Err("量词上界小于下界".into());
        }
        Ok((min, Some(max)))
    }

    fn parse_int(&mut self) -> Result<u32, String> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.pos += 1;
        }
        if start == self.pos {
            return Err("量词缺数字".into());
        }
        std::str::from_utf8(&self.src[start..self.pos])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| "量词数字不合法".into())
    }

    /// 原子：字面字节 / 转义类 / `.` / 类 `[…]` / 分组 `()` / 断言。
    fn parse_atom(&mut self) -> Result<Ast, String> {
        let Some(c) = self.peek() else {
            return Err("正则意外结束".into());
        };
        self.pos += 1;
        let node = match c {
            b'.' => Ast::Byte(ByteSet::AnyButNl),
            b'^' => Ast::Assert(AssertKind::FileStart),
            b'$' => Ast::Assert(AssertKind::FileEnd),
            b'(' => {
                // 官方拒绝 (?:…)`——普通分组只有 (…)。捕获组语义 v1 不
                // 涉及子匹配（YARA 条件用不到子匹配），只做结构分组。
                let inner = self.parse_alt()?;
                if self.peek() != Some(b')') {
                    return Err("分组未闭合".into());
                }
                self.pos += 1;
                inner
            }
            b'[' => self.parse_class()?,
            b'\\' => self.parse_escape()?,
            b'*' | b'+' | b'?' => {
                return Err("量词缺原子".into());
            }
            b')' => return Err("分组未开启的 )".into()),
            _ => Ast::Byte(single(c)),
        };
        Ok(node)
    }

    /// 转义：预定义类 \d \D \w \W \s \S、字符转义 \n \t \r \xNN、
    /// 元字符自身 \. 等；`\b` = 词边界断言（探针 Z 实测）。
    fn parse_escape(&mut self) -> Result<Ast, String> {
        let Some(e) = self.peek() else {
            return Err("转义缺字符".into());
        };
        self.pos += 1;
        Ok(match e {
            b'd' | b'D' | b'w' | b'W' | b's' | b'S' => Ast::Byte(escape_class(e)),
            b'n' => Ast::Byte(single(b'\n')),
            b't' => Ast::Byte(single(b'\t')),
            b'r' => Ast::Byte(single(b'\r')),
            b'x' => {
                let h1 = self.hex_digit()? as u32;
                let h2 = self.hex_digit()? as u32;
                Ast::Byte(single((h1 * 16 + h2) as u8))
            }
            b'b' => Ast::Assert(AssertKind::WordB),
            // 元字符自身与其它可打印字节：转义即字面。控制字节转义如实拒绝
            c if c.is_ascii_alphanumeric() => {
                return Err(format!("转义 \\{} 不支持", c as char));
            }
            c => Ast::Byte(single(c)),
        })
    }

    fn hex_digit(&mut self) -> Result<u8, String> {
        match self.peek() {
            Some(c) if c.is_ascii_hexdigit() => {
                self.pos += 1;
                Ok((c as char).to_digit(16).expect("hexdigit") as u8)
            }
            _ => Err("\\x 缺两位十六进制".into()),
        }
    }

    /// 字符类 `[…]`：范围 a-z、转义类、首 `^` 否定。
    fn parse_class(&mut self) -> Result<Ast, String> {
        let negated = if self.peek() == Some(b'^') {
            self.pos += 1;
            true
        } else {
            false
        };
        let mut set = [false; 256];
        let mut first = true;
        while let Some(c) = self.peek() {
            self.pos += 1;
            match c {
                b']' if !first => {
                    return Ok(Ast::Byte(ByteSet::Set {
                        negated,
                        set: Box::new(set),
                    }));
                }
                _ => {}
            }
            first = false;
            let lo = if c == b'\\' {
                match self.class_escape(&mut set)? {
                    Some(b) => b,
                    None => continue,
                }
            } else {
                c
            };
            // 范围 a-z（`-` 在类内 = 范围连接，字面 `-` 放头尾或转义）
            if self.peek() == Some(b'-')
                && self.src.get(self.pos + 1).copied() != Some(b']')
                && !self.src.get(self.pos + 1).is_none()
            {
                self.pos += 1;
                let hi_c = self.peek().ok_or("类范围缺上界")?;
                self.pos += 1;
                let hi = if hi_c == b'\\' {
                    let Some(b) = self.class_escape(&mut set)? else {
                        return Err("类范围上界不支持该转义".into());
                    };
                    b
                } else {
                    hi_c
                };
                if hi < lo {
                    return Err("类范围上下界反序".into());
                }
                for b in lo..=hi {
                    set[b as usize] = true;
                }
            } else {
                set[lo as usize] = true;
            }
        }
        Err("类未闭合".into())
    }

    /// 类内转义：预定义类并入集合（返回 None = 已并入），字符转义返回字节。
    fn class_escape(&mut self, set: &mut [bool; 256]) -> Result<Option<u8>, String> {
        let Some(e) = self.peek() else {
            return Err("类内转义缺字符".into());
        };
        self.pos += 1;
        Ok(match e {
            b'd' | b'D' | b'w' | b'W' | b's' | b'S' => {
                merge_escape_class(e, set);
                None
            }
            b'n' => Some(b'\n'),
            b't' => Some(b'\t'),
            b'r' => Some(b'\r'),
            b'x' => {
                let h1 = self.hex_digit()? as u32;
                let h2 = self.hex_digit()? as u32;
                Some((h1 * 16 + h2) as u8)
            }
            c => Some(c),
        })
    }
}

// ── 字节集助手 ────────────────────────────────────────────────────────

/// 单字节集合。
fn single(b: u8) -> ByteSet {
    let mut set = Box::new([false; 256]);
    set[b as usize] = true;
    ByteSet::Set {
        negated: false,
        set,
    }
}

/// 预定义转义类（\d \w \s 与大写取补）。
fn escape_class(e: u8) -> ByteSet {
    let mut set = Box::new([false; 256]);
    merge_escape_class(e, &mut set);
    ByteSet::Set {
        negated: e.is_ascii_uppercase(),
        set,
    }
}

/// 把预定义类并入集合（\d = 数字、\w = [A-Za-z0-9_]、\s = 空白）。
fn merge_escape_class(e: u8, set: &mut [bool; 256]) {
    let mut add = |f: &dyn Fn(u8) -> bool| {
        for b in 0u8..=255 {
            if f(b) {
                set[b as usize] = true;
            }
        }
    };
    match e {
        b'd' | b'D' => add(&|b| b.is_ascii_digit()),
        b'w' | b'W' => add(&|b| b.is_ascii_alphanumeric() || b == b'_'),
        b's' | b'S' => add(&|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0b' | b'\x0c')),
        _ => {}
    }
}

// ── 编译器（Thompson 展开，指令数守 MAX_PROG）────────────────────────

/// 编译正则体为 VM 程序；不支持 / 超预算如实 `Err`（→ 串记名弃权）。
pub(super) fn compile(body: &str, nocase: bool) -> Result<RegexPat, String> {
    let ast = depth::check(body).and_then(|()| parse(body))?;
    let mut prog = Vec::new();
    emit(&ast, &mut prog)?;
    prog.push(Instr::Match);
    if prog.len() > MAX_PROG {
        return Err(format!("NFA 超预算（{MAX_PROG} 指令）"));
    }
    Ok(RegexPat { prog, nocase })
}

impl super::Pattern {
    /// 正则串入口：wide/xor/fullword 修饰未与官方对拍（wide 的 `.` 行为
    /// 发散——探针 X/Y）→ 记名弃权；编译不支持/超预算 → 记名弃权
    ///（规则仍解析，扫描判 Unknown——评估不了的串不能报"没命中"）。
    pub fn regex(body: &str, nocase: bool, wide: bool, xor: bool, fullword: bool) -> Self {
        if wide || xor || fullword {
            return Pattern::Unscannable(Unscannable {
                reason: "正则串带 wide/xor/fullword 修饰（未对拍）".into(),
            });
        }
        match compile(body, nocase) {
            Ok(pat) => Pattern::Regex(pat),
            Err(reason) => Pattern::Unscannable(Unscannable {
                reason: format!("正则串：/{body}/（{reason}）"),
            }),
        }
    }
}

/// Thompson 展开：emit 子表达式，跳转尚未回填（用占位 0，随后 patch）。
fn emit(ast: &Ast, prog: &mut Vec<Instr>) -> Result<(), String> {
    if prog.len() > MAX_PROG {
        return Err(format!("NFA 超预算（{MAX_PROG} 指令）"));
    }
    match ast {
        Ast::Byte(bs) => prog.push(Instr::Byte(bs.clone())),
        Ast::Assert(k) => prog.push(Instr::Assert(*k)),
        Ast::Concat(items) => {
            for it in items {
                emit(it, prog)?;
            }
        }
        Ast::Alt(branches) => {
            // 链式 Split：branch1 → Jmp(end) → branch2 → …（分支序 = 优先序）
            let mut jumps = Vec::new();
            for (i, b) in branches.iter().enumerate() {
                if i + 1 < branches.len() {
                    let split_at = prog.len();
                    prog.push(Instr::Split(0, 0));
                    let branch_start = prog.len();
                    emit(b, prog)?;
                    jumps.push(prog.len());
                    prog.push(Instr::Jmp(0));
                    let next = prog.len();
                    prog[split_at] = Instr::Split(branch_start, next);
                } else {
                    emit(b, prog)?;
                }
            }
            let end = prog.len();
            for j in jumps {
                prog[j] = Instr::Jmp(end);
            }
        }
        Ast::Repeat {
            node,
            min,
            max,
            greedy,
        } => emit_repeat(node, *min, *max, *greedy, prog)?,
    }
    Ok(())
}

/// 重复展开：无上界 = Split 循环；有上界 = min 副本 + (max-min) 个可选。
fn emit_repeat(
    node: &Ast,
    min: u32,
    max: Option<u32>,
    greedy: bool,
    prog: &mut Vec<Instr>,
) -> Result<(), String> {
    if min > 255 || max.is_some_and(|m| m.saturating_sub(min) > 255) {
        // 展开爆炸闸：副本数上界 255（超过如实不支持，不静默截断）
        return Err("量词副本数超上限（255）".into());
    }
    match max {
        None => {
            // min 个必选副本 + 无界星环
            for _ in 0..min {
                emit(node, prog)?;
            }
            let l1 = prog.len();
            prog.push(Instr::Split(0, 0));
            let body = prog.len();
            emit(node, prog)?;
            prog.push(Instr::Jmp(l1));
            let exit = prog.len();
            prog[l1] = if greedy {
                Instr::Split(body, exit)
            } else {
                Instr::Split(exit, body)
            };
        }
        Some(m) => {
            for _ in 0..min {
                emit(node, prog)?;
            }
            let mut splits = Vec::new();
            for _ in min..m {
                let split_at = prog.len();
                prog.push(Instr::Split(0, 0));
                splits.push(split_at);
                let body = prog.len();
                emit(node, prog)?;
                prog[split_at] = if greedy {
                    Instr::Split(body, 0)
                } else {
                    Instr::Split(0, body)
                };
            }
            let end = prog.len();
            for s in splits {
                prog[s] = match &prog[s] {
                    Instr::Split(body, 0) if greedy => Instr::Split(*body, end),
                    Instr::Split(0, body) => Instr::Split(end, *body),
                    other => other.clone(),
                };
            }
        }
    }
    Ok(())
}

// ── 运行器（Pike VM，线程优先序 = 回溯 DFS 首完成语义）────────────────

/// 在数据上找全部命中**起点**（每起点至多一命中——获胜者 = 线程优先序
/// 最高的完成，后到的更高优先完成会替换早记录——DFS 首完成语义）。
/// 返回 (起点偏移列表, 扫描是否完整)；预算耗尽 = 不完整（调用方折
/// Unknown——"未命中只在完整扫描后给出"）。
pub(super) fn find_all(data: &[u8], re: &RegexPat, budget: u64) -> (Vec<u64>, bool) {
    let mut hits = Vec::new();
    let mut steps = 0u64;
    let mut visited = vec![0u64; re.prog.len()];
    for p in 0..data.len() {
        match run_at(data, re, p, &mut visited, &mut steps, budget) {
            Run::Hit => hits.push(p as u64),
            Run::Miss => {}
            Run::Exhausted => return (hits, false),
        }
    }
    (hits, true)
}

pub(super) enum Run {
    Hit,
    Miss,
    Exhausted,
}

/// 单起点运行：线程表按优先序逐字节推进。
/// 终止 = 获胜 rank 归零（不可能再被超越）或线程全灭。
pub(super) fn run_at(
    data: &[u8],
    re: &RegexPat,
    p: usize,
    visited: &mut [u64],
    steps: &mut u64,
    budget: u64,
) -> Run {
    let mut clist: Vec<usize> = Vec::new();
    let mut best: Option<u32> = None;
    // 起点闭包（rank 0；空命中在此即成——探针 H 的 (0,"") 形态）
    add_thread(
        data, re, 0, p, p, 0, &mut clist, &mut best, visited, steps, budget,
    );
    if *steps > budget {
        return Run::Exhausted;
    }
    if best == Some(0) {
        return Run::Hit;
    }
    for e in p..data.len() {
        let mut nlist: Vec<usize> = Vec::new();
        for (i, &pc) in clist.iter().enumerate() {
            if let Instr::Byte(bs) = &re.prog[pc] {
                *steps += 1;
                if bs.matches(data[e], re.nocase) {
                    add_thread(
                        data,
                        re,
                        pc + 1,
                        e + 1,
                        p,
                        i as u32,
                        &mut nlist,
                        &mut best,
                        visited,
                        steps,
                        budget,
                    );
                }
                if *steps > budget {
                    return Run::Exhausted;
                }
            }
        }
        if nlist.is_empty() {
            break;
        }
        clist = nlist;
    }
    match best {
        Some(_) => Run::Hit,
        None => Run::Miss,
    }
}

/// ε-闭包：断言在此求值；Match 记候选（rank 更小者替换——优先序即
/// 回溯 DFS 的探索序）。`rank` = 派生线程的父线程在当前线程表的序。
#[allow(clippy::too_many_arguments)]
fn add_thread(
    data: &[u8],
    re: &RegexPat,
    pc: usize,
    pos: usize,
    start: usize,
    rank: u32,
    list: &mut Vec<usize>,
    best: &mut Option<u32>,
    visited: &mut [u64],
    steps: &mut u64,
    budget: u64,
) {
    *steps += 1;
    if *steps > budget {
        return;
    }
    let stamp = ((start as u64) << 32) | (pos as u64 + 1);
    if visited[pc] == stamp {
        return; // 同 (起点,位置) 已见该 pc：首见者优先级更高
    }
    visited[pc] = stamp;
    match &re.prog[pc] {
        Instr::Match => {
            if best.is_none_or(|r| rank < r) {
                *best = Some(rank);
            }
        }
        Instr::Assert(k) => {
            let ok = match k {
                AssertKind::FileStart => pos == 0,
                AssertKind::FileEnd => pos == data.len(),
                AssertKind::WordB => {
                    let w = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
                    let before = pos > 0 && w(data[pos - 1]);
                    let after = pos < data.len() && w(data[pos]);
                    before != after
                }
            };
            if ok {
                add_thread(
                    data,
                    re,
                    pc + 1,
                    pos,
                    start,
                    rank,
                    list,
                    best,
                    visited,
                    steps,
                    budget,
                );
            }
        }
        Instr::Split(a, b) => {
            add_thread(
                data, re, *a, pos, start, rank, list, best, visited, steps, budget,
            );
            add_thread(
                data, re, *b, pos, start, rank, list, best, visited, steps, budget,
            );
        }
        Instr::Jmp(t) => {
            add_thread(
                data, re, *t, pos, start, rank, list, best, visited, steps, budget,
            );
        }
        Instr::Byte(_) => list.push(pc),
    }
}

#[cfg(test)]
mod tests {
    use super::compile;

    /// 单正则冒烟：编译 + 全起点命中列表。
    fn hits(body: &str, data: &[u8]) -> Vec<u64> {
        let re = compile(body, false).expect(body);
        super::find_all(data, &re, 1_000_000).0
    }

    #[test]
    fn compile_errors_are_honest() {
        for bad in [
            "(?:ab)",   // 官方拒绝的语法
            "a{3,2}",   // 上界小于下界
            "[z-a]",    // 类范围反序
            "a{2,999}", // 副本数爆炸闸
            "[a",       // 类未闭合
            "(ab",      // 分组未闭合
        ] {
            assert!(compile(bad, false).is_err(), "{bad} 应编译失败");
        }
        // (?: 官方语法错（探针实测）→ 我们同样编译失败（报错文案不必
        // 与官方逐字一致——差分口径是四态分类，不是报错串对拍）
        assert!(compile("(?:ab)", false).is_err());
    }

    #[test]
    fn probe_semantics_replay() {
        // 全部来自官方探针实测读数（2026-11-12，yara-python 4.5.4）
        assert_eq!(hits("a*", b"xaax"), vec![0, 1, 2, 3]);
        assert_eq!(hits("a+", b"xaax"), vec![1, 2]);
        assert_eq!(hits("a{2,3}", b"aaa"), vec![0, 1]);
        assert_eq!(hits("a|ab", b"ab"), vec![0]); // 回溯优先序：首分支胜
        assert_eq!(hits("a+?b", b"aab"), vec![0, 1]);
        assert_eq!(hits(r"\d+\w+\x41", b"12abA!"), vec![0, 1]);
        assert_eq!(hits(r"\b", b"ab\x08c"), vec![0, 2, 3]); // \b = 词边界断言
        assert_eq!(hits("[^a]+", b"aabc"), vec![2, 3]);
        assert_eq!(hits("a{2,}$", b"xaa"), vec![1]);
        assert_eq!(hits("(ab)+", b"zabab!"), vec![1, 3]);
        assert_eq!(hits("[a-f0-9]{4}", b"zz9f3c"), vec![2]);
        assert_eq!(hits("(a|)", b"bab"), vec![0, 1, 2]); // 空分支 = 空命中
        assert_eq!(hits("^abc", b"abc\ndef"), vec![0]);
        assert_eq!(hits("def$", b"abc\ndef"), vec![4]);
        assert_eq!(hits("abc$", b"abc\ndef"), Vec::<u64>::new()); // $ = 文件尾断言
        assert_eq!(hits("a.c", b"a\nc"), Vec::<u64>::new()); // . 不含 \n
        assert_eq!(hits("a.c", b"axc"), vec![0]);
        assert_eq!(hits("aa", b"aaaa"), vec![0, 1, 2]); // 重叠全报
    }

    #[test]
    fn nocase_folds_bytes() {
        let re = compile("abc", true).expect("编译");
        assert_eq!(super::find_all(b"ABC", &re, 1_000_000).0, vec![0]);
        // 否定类 + nocase：折叠后 [^a] 排除 a/A 两者
        let re = compile("[^a]+", true).expect("编译");
        assert_eq!(super::find_all(b"aA", &re, 1_000_000).0, Vec::<u64>::new());
    }
}
