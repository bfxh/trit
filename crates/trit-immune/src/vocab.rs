//! 概念台账：反"换词不换设计"污染。
//!
//! 病灶：论文/库/项目把**同一个设计换个词重发一遍**——理念层面的杂乱与污染。
//! 台账把每个设计钉死在 **规范名 + 别名 + 设计属性** 上：
//!
//! - **标签是解释用的**：查任何别名都返回规范名 + 定义（解释，不是分类）；
//! - **同构检测**：新概念的属性集合与已登记概念完全一致 → 判**重复**（换词不换设计，
//!   拒收并指认出处）；部分重叠 → **挂起**（人工裁决，引用出处）；全新 → **入账**；
//! - **撞名纪律**：规范名/别名与已有条目冲突 → 直接拒绝——外面换的词应当
//!   [`Vocabulary::add_alias`] 收编进已有概念，而不是立新词。
//!
//! 预置了内核自己的概念种子：其他仓库/论文的等价设计来登记时会被当场指认。

use std::collections::BTreeSet;

/// 一个设计概念：规范名 + 别名 + 定义 + 设计属性（同构判定只看属性）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Concept {
    /// 规范名（唯一）。
    pub name: String,
    /// 别名：他处换的词，收编进来供查询。
    pub aliases: Vec<String>,
    /// 领域（如 sync / logic / policy）。
    pub domain: String,
    /// 一句话定义（解释用）。
    pub definition: String,
    /// 设计属性（键值对）。空属性的条目不参与同构判定。
    pub attributes: Vec<(String, String)>,
}

impl Concept {
    pub fn new(
        name: &str,
        aliases: &[&str],
        domain: &str,
        definition: &str,
        attributes: &[(&str, &str)],
    ) -> Self {
        Self {
            name: name.to_string(),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
            domain: domain.to_string(),
            definition: definition.to_string(),
            attributes: attributes
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn normalized_attributes(&self) -> Vec<(String, String)> {
        let mut v = self.attributes.clone();
        v.sort();
        v
    }
}

/// 登记结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Registration {
    /// 全新设计，入账。
    Accepted,
    /// 换词不换设计：与已有概念属性同构，拒收（`of` 指认规范名）。
    Duplicate { of: String },
    /// 属性部分重叠：人工裁决（`with` 列出所有重叠概念）。
    Overlap { with: Vec<String> },
}

/// 折叠度量：词 → 概念的收敛率。
#[derive(Debug, Clone, PartialEq)]
pub struct Absorption {
    pub concepts: usize,
    pub aliases: usize,
    /// 别名 / (概念 + 别名)；空台账视为 1.0
    pub ratio: f64,
}

/// 概念台账。
#[derive(Debug, Clone, Default)]
pub struct Vocabulary {
    concepts: Vec<Concept>,
}

impl Vocabulary {
    pub fn new() -> Self {
        Self::default()
    }

    /// 内核自有概念的种子台账（其他仓库的换词等价物来登记时会被当场指认）。
    pub fn with_kernel_concepts() -> Self {
        let mut v = Self::new();
        let seeds = [
            Concept::new(
                "HLC",
                &["混合逻辑时钟", "Hybrid Logical Clock", "物理逻辑混合时钟"],
                "sync",
                "物理毫秒 + 逻辑计数 + 写者 id 的因果戳；同写者严格单调",
                &[("causal", "hlc"), ("ordering", "total"), ("clock", "hybrid")],
            ),
            Concept::new(
                "K3",
                &["Kleene 三值逻辑", "Kleene K3", "强三值逻辑"],
                "logic",
                "三值逻辑：未知在中间传染，证伪支配即否决",
                &[
                    ("values", "3"),
                    ("unknown", "propagates"),
                    ("dominance", "falsified"),
                ],
            ),
            Concept::new(
                "LWW",
                &["后到者胜", "Last-Write-Wins", "末写胜出"],
                "sync",
                "并发冲突按全序后到者胜；两条历史都保留（审计不丢）",
                &[("conflict", "lww"), ("history", "keep-all")],
            ),
            Concept::new(
                "anti-entropy",
                &["反熵", "双向反熵", "反熵同步"],
                "sync",
                "副本交换日志、按全序合并至收敛；确定性 ⇒ 逐字节一致",
                &[
                    ("model", "state-based"),
                    ("convergence", "eventual"),
                    ("direction", "bidirectional"),
                ],
            ),
            Concept::new(
                "合并门禁",
                &["merge gate", "MergeGate", "合并门"],
                "policy",
                "因果链 + 锚定攻击分支的 K3 AND 折叠 → 放行/冻结/阻断",
                &[("fold", "k3-and"), ("action", "allow-freeze-block")],
            ),
            Concept::new(
                "污染传播",
                &["contagion", "污点传播", "影响面传播"],
                "propagate",
                "(min,max) 半环：路径强度取最薄弱环节，目标取最优路径（攻击者视角）",
                &[("semiring", "min-max"), ("polarity", "attack-first")],
            ),
            Concept::new(
                "收割",
                &["harvest", "项目收割"],
                "platform",
                "清构建产物 + 语言普查贴标签 + 源码路径登记的吸收流程",
                &[
                    ("cleanup", "whitelist"),
                    ("tag", "explanatory"),
                    ("ledger", "append-only"),
                ],
            ),
            Concept::new(
                "预测合并",
                &["predictive merge", "合并预测"],
                "policy",
                "合并门禁 + 传播后有效裁决作为策略输入，纯函数决策",
                &[
                    ("predictor", "immune-dag"),
                    ("decision", "auto-hold-block"),
                ],
            ),
        ];
        for c in seeds {
            let _ = v.register(c);
        }
        v
    }

    /// 登记一个概念。返回 [`Registration`]；撞名直接拒绝。
    pub fn register(&mut self, concept: Concept) -> Result<Registration, String> {
        // 撞名纪律：规范名/别名与已有任何条目冲突 → 拒绝
        let new_terms: BTreeSet<&str> = std::iter::once(concept.name.as_str())
            .chain(concept.aliases.iter().map(|a| a.as_str()))
            .collect();
        for existing in &self.concepts {
            for term in &new_terms {
                if existing.name == *term || existing.aliases.iter().any(|a| a == term) {
                    return Err(format!(
                        "撞名：{term:?} 已被规范名 {:?} 占用——应 add_alias 收编，不立新词",
                        existing.name
                    ));
                }
            }
        }

        let new_attrs = concept.normalized_attributes();
        if !new_attrs.is_empty() {
            // 同构检测：换词不换设计 → 指认出处
            let mut overlaps = Vec::new();
            for existing in &self.concepts {
                let old = existing.normalized_attributes();
                if old.is_empty() {
                    continue;
                }
                if old == new_attrs {
                    return Ok(Registration::Duplicate {
                        of: existing.name.clone(),
                    });
                }
                if old.iter().any(|kv| new_attrs.contains(kv)) {
                    overlaps.push(existing.name.clone());
                }
            }
            if !overlaps.is_empty() {
                return Ok(Registration::Overlap { with: overlaps });
            }
        }

        self.concepts.push(concept);
        Ok(Registration::Accepted)
    }

    /// 给已有概念收编一个别名（外面换的词进来，规范名不变）。
    pub fn add_alias(&mut self, name: &str, alias: &str) -> Result<(), String> {
        for existing in &self.concepts {
            if existing.name == alias || existing.aliases.iter().any(|a| a == &alias) {
                return Err(format!("撞名：{alias:?} 已被 {:?} 占用", existing.name));
            }
        }
        let c = self
            .concepts
            .iter_mut()
            .find(|c| c.name == name)
            .ok_or_else(|| format!("无此规范名：{name:?}"))?;
        c.aliases.push(alias.to_string());
        Ok(())
    }

    /// **标签是解释用的**：按规范名或任意别名查出概念（规范名 + 定义 + 属性）。
    pub fn explain(&self, term: &str) -> Option<&Concept> {
        self.concepts.iter().find(|c| {
            c.name == term || c.aliases.iter().any(|a| a == term)
        })
    }

    pub fn concepts(&self) -> &[Concept] {
        &self.concepts
    }

    /// 折叠度量：词 → 概念的收敛率。**台账体积上限 = 不同设计的数量**；
    /// 别名收编越多 ratio 越高，新词入账必须带来新属性（见 `docs/GOAL.md`）。
    pub fn absorption(&self) -> Absorption {
        let aliases: usize = self.concepts.iter().map(|c| c.aliases.len()).sum();
        let concepts = self.concepts.len();
        let total = concepts + aliases;
        Absorption {
            concepts,
            aliases,
            ratio: if total == 0 {
                1.0
            } else {
                aliases as f64 / total as f64
            },
        }
    }

    pub fn len(&self) -> usize {
        self.concepts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.concepts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_seed_registers_all() {
        let v = Vocabulary::with_kernel_concepts();
        assert_eq!(v.len(), 8);
    }

    #[test]
    fn tags_are_explanatory_lookup_by_alias() {
        let v = Vocabulary::with_kernel_concepts();
        // 换的词查得到规范概念——标签是解释用的
        let c = v.explain("混合逻辑时钟").expect("别名应命中");
        assert_eq!(c.name, "HLC");
        assert!(c.definition.contains("因果戳"));
        let c = v.explain("后到者胜").expect("别名应命中");
        assert_eq!(c.name, "LWW");
        assert!(v.explain("不存在的词").is_none());
    }

    #[test]
    fn renamed_duplicate_design_is_flagged() {
        let mut v = Vocabulary::with_kernel_concepts();
        // 换词不换设计：属性集合与 HLC 完全一致
        let renamed = Concept::new(
            "超级时间戳",
            &["SuperStamp"],
            "sync",
            "物理加逻辑的超级因果时间",
            &[("ordering", "total"), ("causal", "hlc"), ("clock", "hybrid")],
        );
        match v.register(renamed) {
            Ok(Registration::Duplicate { of }) => assert_eq!(of, "HLC"),
            other => panic!("应判重复，实际 {other:?}"),
        }
        // 被拒后台账里没有它
        assert!(v.explain("超级时间戳").is_none());
    }

    #[test]
    fn partial_overlap_goes_to_human() {
        let mut v = Vocabulary::with_kernel_concepts();
        let partial = Concept::new(
            "向量时钟",
            &["Vector Clock", "向量钟"],
            "sync",
            "每写者计数的因果戳（无物理混合）",
            &[("causal", "vector"), ("ordering", "partial"), ("clock", "logical")],
        );
        match v.register(partial) {
            // 与 HLC 共享 ("clock","hybrid")? 否——共享的是因果域相关项；
            // 向量时钟与 HLC 无相同键值 → Accepted
            Ok(Registration::Accepted) => {}
            other => panic!("无共同属性应入账，实际 {other:?}"),
        }
        // 与 LWW 共享 ("conflict","lww") 但属性集不同 → Overlap（人工裁决）
        let overlapped = Concept::new(
            "MVP",
            &["多值寄存器", "Multi-Value Register"],
            "sync",
            "冲突保留全部值由读方裁决",
            &[("conflict", "lww"), ("resolution", "read-side")],
        );
        match v.register(overlapped) {
            Ok(Registration::Overlap { with }) => assert_eq!(with, vec!["LWW"]),
            other => panic!("应判部分重叠，实际 {other:?}"),
        }
    }

    #[test]
    fn name_collision_is_rejected_and_alias_absorbs() {
        let mut v = Vocabulary::with_kernel_concepts();
        // 立新词撞已有别名（"混合逻辑时钟"是 HLC 的种子别名）→ 拒绝，指向收编通道
        let err = v
            .register(Concept::new(
                "混合逻辑时钟",
                &[],
                "sync",
                "换词立新词",
                &[("causal", "x")],
            ))
            .unwrap_err();
        assert!(err.contains("收编"), "{err}");
        // 正确姿势：把全新词收编进已有概念
        v.add_alias("HLC", "逻辑物理时钟").unwrap();
        assert!(v.explain("逻辑物理时钟").is_some());
        // 再收编同别名 → 撞名
        assert!(v.add_alias("LWW", "逻辑物理时钟").is_err());
    }

    #[test]
    fn attribute_order_does_not_matter() {
        let mut v = Vocabulary::new();
        v.register(Concept::new("甲", &[], "d", "x", &[("a", "1"), ("b", "2")]))
            .unwrap();
        let dup = v.register(Concept::new("乙", &[], "d", "y", &[("b", "2"), ("a", "1")]));
        match dup {
            Ok(Registration::Duplicate { of }) => assert_eq!(of, "甲"),
            other => panic!("属性无序应判同构，实际 {other:?}"),
        }
    }

    #[test]
    fn absorption_measures_word_folding() {
        let v = Vocabulary::with_kernel_concepts();
        let a = v.absorption();
        assert_eq!(a.concepts, 8);
        // 种子别名数：前六条各 3 + 后两条各 2 = 22
        assert_eq!(a.aliases, 22);
        assert!(a.ratio > 0.5, "折叠率应过半：{}", a.ratio);
        // 收编一个新别名 → 折叠率上升（单向改善）
        let mut v2 = v.clone();
        v2.add_alias("K3", "三值代数").unwrap();
        assert!(v2.absorption().ratio > a.ratio);
    }
}
