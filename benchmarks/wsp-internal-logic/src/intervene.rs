//! 穷尽干预（M0e）：对层 1 的每个单元做「清零 / 取反」，统计输出翻转。
//!
//! 这是设计文档 L-D 的第二半：光有「层 1 决定输出」（信息瓶颈）不够，还要知道
//! **每一步是否承重**——不承重的单元是装饰，不该算进「思考链」。
//!
//! 做法是穷尽的：域内每个串 × 每个单元 × 两种替代值（`Zero`、取反），
//! 不抽样。翻转率即该单元的承重读数。

use trit_wspace::{Layer, Net, Trit};

use crate::trace;
use crate::word;

/// 一个单元的干预读数。
#[derive(Clone, Copy, Debug)]
pub struct UnitImpact {
    pub unit: usize,
    /// 清零后输出翻转的串数。
    pub flips_zero: usize,
    /// 取反后输出翻转的串数。
    pub flips_neg: usize,
}

/// 全部单元的干预读数。
#[derive(Clone, Debug)]
pub struct Matrix {
    pub units: Vec<UnitImpact>,
    pub words: usize,
}

impl Matrix {
    /// 有多少个单元至少在一个串上承重。
    #[must_use]
    pub fn live_units(&self) -> usize {
        self.units
            .iter()
            .filter(|u| u.flips_zero > 0 || u.flips_neg > 0)
            .count()
    }
}

/// 用层 2 权重把（干预后的）层 1 激活算成输出。
fn out_of(layer: &Layer, h: &[Trit]) -> Trit {
    let mut acc = 0i32;
    for r in 0..layer.rows() {
        let mut sum = i32::from(layer.bias()[r].to_i8());
        let row = layer.row(r).expect("行号在范围内");
        for (c, &w) in row.iter().enumerate() {
            sum += i32::from(w.to_i8()) * i32::from(h[c].to_i8());
        }
        acc = sum;
    }
    Trit::sign_of(acc)
}

/// 在声明域上做穷尽干预。
#[must_use]
pub fn sweep(net: &Net, max_len: usize) -> Matrix {
    let layer2 = net.layer(1).expect("本案例是两层网络");
    let units = net.layer(0).expect("有层 1").rows();
    let words = word::all_words(max_len);
    let mut out = vec![
        UnitImpact {
            unit: 0,
            flips_zero: 0,
            flips_neg: 0,
        };
        units
    ];
    for w in &words {
        let t = trace::trace(net, w, max_len);
        let base = out_of(layer2, &t.hidden);
        for (unit, impact) in out.iter_mut().enumerate() {
            let mut h = t.hidden.clone();
            h[unit] = Trit::Zero;
            if out_of(layer2, &h) != base {
                impact.flips_zero += 1;
            }
            h[unit] = t.hidden[unit].neg();
            if out_of(layer2, &h) != base {
                impact.flips_neg += 1;
            }
        }
    }
    for (i, impact) in out.iter_mut().enumerate() {
        impact.unit = i;
    }
    Matrix {
        units: out,
        words: words.len(),
    }
}
