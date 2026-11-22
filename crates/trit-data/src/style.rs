//! 文本风格识别（可复算规则）：正式 / 口语 / 技术 / 教程 / 叙事 / 问答。
//!
//! 规则只看可重算的密度与标记；不训练分类器。分数用于筛选，也可只报告。

use crate::content::code_ratio;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleKind {
    Formal,
    Conversational,
    Technical,
    Tutorial,
    Narrative,
    Qa,
    Other,
}

impl StyleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StyleKind::Formal => "formal",
            StyleKind::Conversational => "conversational",
            StyleKind::Technical => "technical",
            StyleKind::Tutorial => "tutorial",
            StyleKind::Narrative => "narrative",
            StyleKind::Qa => "qa",
            StyleKind::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StyleScores {
    pub formal: f64,
    pub conversational: f64,
    pub technical: f64,
    pub tutorial: f64,
    pub narrative: f64,
    pub qa: f64,
}

impl StyleScores {
    pub fn best(&self) -> StyleKind {
        let all = [
            (StyleKind::Formal, self.formal),
            (StyleKind::Conversational, self.conversational),
            (StyleKind::Technical, self.technical),
            (StyleKind::Tutorial, self.tutorial),
            (StyleKind::Narrative, self.narrative),
            (StyleKind::Qa, self.qa),
        ];
        let (kind, score) = all
            .iter()
            .copied()
            .fold((StyleKind::Other, 0.0), |acc, cur| {
                if cur.1 > acc.1 { cur } else { acc }
            });
        if score < 0.2 { StyleKind::Other } else { kind }
    }
}

pub fn score(text: &str) -> StyleScores {
    let t = text.to_lowercase();
    let chars = text.chars().count().max(1) as f64;
    let sentences = text
        .split(['。', '！', '？', '.', '!', '?'])
        .filter(|s| !s.trim().is_empty())
        .count()
        .max(1) as f64;
    let freq = |words: &[&str]| {
        (words.iter().map(|w| t.matches(w).count()).sum::<usize>() as f64
            / (chars / 100.0).max(1.0))
        .min(1.0)
    };
    let fi = (["我", "我们", "i ", "we ", "my ", "our "]
        .iter()
        .map(|w| t.matches(w).count())
        .sum::<usize>() as f64
        / sentences)
        .min(1.0);
    let q = (t.matches('?').count() + t.matches('？').count()) as f64 / sentences;
    let ex = (t.matches('!').count() + t.matches('！').count()) as f64 / sentences;
    let code = code_ratio(text);
    let formal_w = [
        "因此",
        "综上",
        "此外",
        "然而",
        "并且",
        "therefore",
        "moreover",
        "however",
        "furthermore",
    ];
    let conv_w = [
        "嗯",
        "啊",
        "吧",
        "呢",
        "我觉得",
        "其实",
        "就是说",
        "you know",
        "kind of",
        "lol",
        "哈哈",
    ];
    let tech_w = [
        "api",
        "参数",
        "接口",
        "协议",
        "性能",
        "内存",
        "线程",
        "并发",
        "函数",
        "变量",
        "编译",
        "部署",
        "cache",
        "latency",
        "throughput",
        "runtime",
        "schema",
    ];
    let tut_w = [
        "第一步",
        "第二步",
        "然后",
        "接下来",
        "最后",
        "示例",
        "例如",
        "step",
        "first",
        "then",
        "next",
        "finally",
        "example",
    ];
    let narr_w = [
        "那天",
        "后来",
        "他说",
        "她说",
        "故事",
        "once",
        "she said",
        "he said",
        "narrative",
    ];
    let qa_w = ["问：", "答：", "q:", "a:", "问题：", "回答："];
    StyleScores {
        formal: (freq(&formal_w) * 0.6 + (chars / sentences / 40.0).min(1.0) * 0.4
            - freq(&conv_w) * 0.5)
            .clamp(0.0, 1.0),
        conversational: (freq(&conv_w) * 0.7 + fi * 0.2 + (q + ex) * 0.3).clamp(0.0, 1.0),
        technical: (freq(&tech_w) * 0.6 + code * 0.4).clamp(0.0, 1.0),
        tutorial: (freq(&tut_w) * 0.75 + code * 0.15 + fi * 0.1).clamp(0.0, 1.0),
        narrative: (freq(&narr_w) * 0.7 + fi * 0.2).clamp(0.0, 1.0),
        qa: (q.min(1.0) * 0.5 + freq(&qa_w) * 0.5).clamp(0.0, 1.0),
    }
}

pub fn detect(text: &str) -> StyleKind {
    score(text).best()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_formal_and_conversational() {
        assert_eq!(
            detect("因此，我们采用该方案。此外，还需要考虑性能。综上，结论如下。"),
            StyleKind::Formal
        );
        assert_eq!(
            detect("嗯，我觉得这个吧，其实也还行，你说呢？"),
            StyleKind::Conversational
        );
    }

    #[test]
    fn detects_technical_tutorial_qa_narrative() {
        assert_eq!(
            detect("API 参数与内存线程并发性能：runtime cache latency throughput schema。"),
            StyleKind::Technical
        );
        assert_eq!(
            detect("第一步，安装依赖。然后运行示例。接下来检查输出。最后部署。"),
            StyleKind::Tutorial
        );
        assert_eq!(
            detect("问：什么是三值逻辑？答：它是一种三值语义。问题：为什么？回答：因为需要未知。"),
            StyleKind::Qa
        );
        assert_eq!(
            detect("那天下午，他走进房间。后来，她说起了故事。"),
            StyleKind::Narrative
        );
    }

    #[test]
    fn empty_is_other_and_scores_bounded() {
        assert_eq!(detect(""), StyleKind::Other);
        let s = score("普通文本");
        for v in [
            s.formal,
            s.conversational,
            s.technical,
            s.tutorial,
            s.narrative,
            s.qa,
        ] {
            assert!((0.0..=1.0).contains(&v));
        }
    }
}
