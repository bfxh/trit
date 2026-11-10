//! 匹配族的**分配口径**探针：每节点多少次 `alloc`、多少字节。
//!
//! 用途是给「watched literals / 扁平化子句表」这一刀定量：
//! `dpll::residue` 每节点重建一次 CSR 出现表，并为每条残基子句单独 `Vec`，
//! 如果 µs/节点随规模上升主要由分配贡献，这一档就该由 arena / 复用缓冲来解决，
//! 而不是由监视器（监视器省的是扫描次数）。
//!
//! 只读不写产品代码：全局分配计数器装在**这个 example 里**，因此壁钟会被放大，
//! **不看时间、只看次数与字节**。节点数取自 `CountStats`，与 `matching_family` 逐位一致。

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::parser::Parser;
use trit_core::value::Ternary;
use trit_count::{cnf, dpll};

/// 记数的分配器：`alloc` / `dealloc` 各计一次，字节按 `layout.size()` 记（不含 realloc 复用）。
struct Counting;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static DEALLOC: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(u64::try_from(layout.size()).unwrap_or(0), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        DEALLOC.fetch_add(1, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn snapshot() -> (u64, u64, u64) {
    (
        ALLOCS.load(Ordering::Relaxed),
        DEALLOC.load(Ordering::Relaxed),
        BYTES.load(Ordering::Relaxed),
    )
}

fn family(pairs: usize) -> (Expr, Vec<String>, HashSet<String>) {
    let xs: Vec<String> = (0..pairs).map(|index| format!("x{index}")).collect();
    let ys: Vec<String> = (0..pairs).map(|index| format!("y{index}")).collect();
    let mut source = String::new();
    for (x, y) in xs.iter().zip(&ys) {
        let term = format!("({x} & {y})");
        source = if source.is_empty() {
            term
        } else {
            format!("({source} | {term})")
        };
    }
    let declared: Vec<String> = xs.iter().chain(ys.iter()).cloned().collect();
    let binary: HashSet<String> = declared.iter().cloned().collect();
    (
        Parser::parse(&source).expect("matching family parses"),
        declared,
        binary,
    )
}

fn main() {
    // 自标定：在当前（带计数的）分配器下量一次 alloc/free 的单价，用来把
    // 「allocs/节点」乘成「µs/节点」——注意这个单价**含计数器自身的开销**，
    // 所以它是分配成本的上界，用来判"分配能不能解释上升线"够用。
    let calib = std::hint::black_box(1_000_000usize);
    let start = std::time::Instant::now();
    let mut keep = 0usize;
    for i in 0..calib {
        let v: Vec<u8> = vec![i as u8; 64];
        keep += std::hint::black_box(&v)[0] as usize;
    }
    let ns_per_alloc = start.elapsed().as_nanos() as f64 / calib as f64;
    println!(
        "标定：64 B 的 vec! 一次 alloc+free = {ns_per_alloc:.1} ns（含计数开销，上界口径），keep={keep}"
    );

    println!("pairs clauses nodes allocs/节点 deallocs/节点 bytes/节点 分配折算µs/节点");
    for pairs in [8usize, 12, 16, 20, 24, 28, 32, 40] {
        let (expr, declared, binary) = family(pairs);
        let encoded =
            cnf::Encoder::new(Algebra::Kleene, &declared, &binary).encode(&expr, Ternary::True);
        let clauses = encoded.clauses.len();

        let (a0, d0, b0) = snapshot();
        let result = dpll::count_cnf(&encoded);
        let (a1, d1, b1) = snapshot();

        let nodes = result.stats.nodes;
        assert!(
            nodes > 0,
            "pairs={pairs} 节点数为 0 ⇒ 这一档根本没跑，计数不可解释"
        );
        assert!(a1 >= a0 && b1 >= b0, "分配计数倒退：{a0}→{a1}");
        let allocs_per_node = (a1 - a0) as f64 / nodes as f64;
        println!(
            "{pairs:>5} {clauses:>7} {nodes:>6} {:>11.1} {:>13.1} {:>10.0} {:>9.1}",
            allocs_per_node,
            (d1 - d0) as f64 / nodes as f64,
            (b1 - b0) as f64 / nodes as f64,
            allocs_per_node * ns_per_alloc / 1e3,
        );
    }
}
