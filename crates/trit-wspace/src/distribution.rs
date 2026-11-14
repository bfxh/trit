//! 训练数据分布：输入坐标频率、输出类别与唯一模式数。
//!
//! 只看 oracle 的样本集，不跑网络。用来回答「训练数据覆盖了哪些取值、有没有长尾」。

use std::collections::{BTreeMap, BTreeSet};

use crate::oracle::Oracle;
use crate::trit::Trit;

/// 一个输入坐标的取值频率。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoordStats {
    /// 坐标下标。
    pub index: usize,
    /// `Neg` 次数。
    pub neg: usize,
    /// `Zero` 次数。
    pub zero: usize,
    /// `Pos` 次数。
    pub pos: usize,
}

/// 一个输出类别的样本数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClassStats {
    /// 类别（一维输出取 trit 值；多维取最大 trit 的位置）。
    pub class: i32,
    /// 样本数。
    pub count: usize,
}

/// 样本分布。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Distribution {
    /// 样本数。
    pub samples: usize,
    /// 每个输入坐标的取值频率。
    pub inputs: Vec<CoordStats>,
    /// 输出类别分布。
    pub outputs: Vec<ClassStats>,
    /// 不同的输入模式数。
    pub unique_inputs: usize,
    /// 不同的输出模式数。
    pub unique_outputs: usize,
}

/// 输出类别：一维按 trit 值；多维按最大 trit 的位置。
#[must_use]
pub fn class_of(output: &[Trit]) -> i32 {
    if output.len() == 1 {
        i32::from(output[0].to_i8())
    } else {
        output
            .iter()
            .enumerate()
            .max_by_key(|(_, value)| value.to_i8())
            .map_or(0, |(index, _)| index as i32)
    }
}

/// 统计样本分布。
#[must_use]
pub fn distribution(oracle: &Oracle) -> Distribution {
    let inputs = oracle.inputs();
    let outputs = oracle.outputs();
    let width = inputs.first().map_or(0, Vec::len);
    let mut coords: Vec<CoordStats> = (0..width)
        .map(|index| CoordStats {
            index,
            neg: 0,
            zero: 0,
            pos: 0,
        })
        .collect();
    let mut unique_inputs = BTreeSet::new();
    for row in inputs {
        unique_inputs.insert(row.clone());
        for (index, value) in row.iter().enumerate() {
            match value {
                Trit::Neg => coords[index].neg += 1,
                Trit::Zero => coords[index].zero += 1,
                Trit::Pos => coords[index].pos += 1,
            }
        }
    }
    let mut class_map: BTreeMap<i32, usize> = BTreeMap::new();
    for row in outputs {
        *class_map.entry(class_of(row)).or_default() += 1;
    }
    let classes = class_map
        .into_iter()
        .map(|(class, count)| ClassStats { class, count })
        .collect();
    let unique_outputs = outputs.iter().cloned().collect::<BTreeSet<_>>().len();
    Distribution {
        samples: inputs.len(),
        inputs: coords,
        outputs: classes,
        unique_inputs: unique_inputs.len(),
        unique_outputs,
    }
}
