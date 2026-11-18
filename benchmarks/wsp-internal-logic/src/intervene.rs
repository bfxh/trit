//! 穷尽干预（M0e）：对**任意层**的指定单元做「清零 / 取反」，统计输出翻转。
//!
//! 这是设计文档 L-D 的第二半：光有「某层决定输出」（信息瓶颈）不够，还要知道
//! **每一步是否承重**——不承重的单元是装饰，不该算进「思考链」。
//!
//! 做法是穷尽的：域内每个串 × 每个单元 × 两种替代值（`Zero`、取反），不抽样。
//! 翻转率即该单元的承重读数。
//!
//! 通用化后的签名允许指定 `layer_idx` 与 `units`：
//! 手写两层网络用 `(0, 全部单元)`；编译出来的多层网络用 `(0, 状态位数)`。

use trit_wspace::{Net, Trit};

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

/// 在声明域上对第 `layer_idx` 层输出的前 `units` 个单元做穷尽干预。
///
/// 做法：取该层激活，改一个单元，再用**尾部网络**（`layer_idx + 1` 之后）重算输出。
/// 这样对两层网络与多层编译网络是同一个口径。
#[must_use]
pub fn sweep(
    net: &Net,
    max_len: usize,
    layer_idx: usize,
    units: usize,
    encode: fn(&[Trit], usize) -> Vec<Trit>,
) -> Matrix {
    let layers = net.layers();
    assert!(
        layer_idx + 1 < layers.len(),
        "layer_idx 之后必须还有层（否则没有可观测的输出）"
    );
    let tail = Net::new(layers[layer_idx + 1..].to_vec()).expect("尾部网络维度连续");
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
        let input = encode(w, max_len);
        let pre = net.pre_activations(&input).expect("编码后维度一致");
        let hidden: Vec<Trit> = pre[layer_idx].iter().map(|v| Trit::sign_of(*v)).collect();
        let base = tail.forward(&hidden).expect("尾部输入维度一致")[0];
        for (unit, impact) in out.iter_mut().enumerate() {
            let mut h = hidden.clone();
            h[unit] = Trit::Zero;
            if tail.forward(&h).expect("同宽度")[0] != base {
                impact.flips_zero += 1;
            }
            h[unit] = hidden[unit].neg();
            if tail.forward(&h).expect("同宽度")[0] != base {
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
