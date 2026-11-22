//! 语义 / 领域识别（可复算规则）：代码 / 数学 / 科学 / 医学 / 法律 / 新闻 / 小说 / 对话。

use crate::content::code_ratio;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticKind {
    Code,
    Math,
    Science,
    Medical,
    Legal,
    News,
    Fiction,
    Dialogue,
    Other,
}

impl SemanticKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SemanticKind::Code => "code",
            SemanticKind::Math => "math",
            SemanticKind::Science => "science",
            SemanticKind::Medical => "medical",
            SemanticKind::Legal => "legal",
            SemanticKind::News => "news",
            SemanticKind::Fiction => "fiction",
            SemanticKind::Dialogue => "dialogue",
            SemanticKind::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticScores {
    pub code: f64,
    pub math: f64,
    pub science: f64,
    pub medical: f64,
    pub legal: f64,
    pub news: f64,
    pub fiction: f64,
    pub dialogue: f64,
}

impl SemanticScores {
    pub fn best(&self) -> SemanticKind {
        let all = [
            (SemanticKind::Code, self.code),
            (SemanticKind::Math, self.math),
            (SemanticKind::Science, self.science),
            (SemanticKind::Medical, self.medical),
            (SemanticKind::Legal, self.legal),
            (SemanticKind::News, self.news),
            (SemanticKind::Fiction, self.fiction),
            (SemanticKind::Dialogue, self.dialogue),
        ];
        let (kind, score) = all
            .iter()
            .copied()
            .fold((SemanticKind::Other, 0.0), |acc, cur| {
                if cur.1 > acc.1 { cur } else { acc }
            });
        if score < 0.2 {
            SemanticKind::Other
        } else {
            kind
        }
    }
}

pub fn score(text: &str) -> SemanticScores {
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
    let code_words = [
        "api",
        "函数",
        "变量",
        "代码",
        "编译",
        "部署",
        "接口",
        "参数",
        "python",
        "rust",
        "javascript",
        "sql",
        "git",
        "npm",
        "cargo",
    ];
    let math_words = [
        "定理",
        "证明",
        "方程",
        "矩阵",
        "概率",
        "积分",
        "导数",
        "向量",
        "特征值",
        "theorem",
        "proof",
        "equation",
        "matrix",
        "probability",
    ];
    let sci_words = [
        "实验",
        "假设",
        "观测",
        "样本",
        "物理",
        "化学",
        "生物",
        "hypothesis",
        "experiment",
        "observation",
    ];
    let med_words = [
        "患者",
        "症状",
        "诊断",
        "治疗",
        "药物",
        "临床",
        "手术",
        "patient",
        "symptom",
        "diagnosis",
    ];
    let legal_words = [
        "法律",
        "条款",
        "合同",
        "责任",
        "诉讼",
        "法院",
        "判决",
        "contract",
        "liability",
        "court",
    ];
    let news_words = [
        "记者",
        "报道",
        "今日",
        "发布",
        "声明",
        "选举",
        "news",
        "report",
        "announced",
    ];
    let fic_words = [
        "故事", "小说", "那晚", "梦中", "人物", "情节", "story", "novel", "dream",
    ];
    let dia_words = [
        "你：", "我：", "他：", "她说", "对话", "问：", "答：", "you:", "me:", "he said",
    ];
    let symbols = text.chars().filter(|c| "∑∫≤≥±×÷√∞≈≠".contains(*c)).count() as f64;
    let q = (t.matches('?').count() + t.matches('？').count()) as f64 / sentences;
    let fi = (["我", "我们", "i ", "we "]
        .iter()
        .map(|w| t.matches(w).count())
        .sum::<usize>() as f64
        / sentences)
        .min(1.0);
    SemanticScores {
        code: (code_ratio(text) * 0.6 + freq(&code_words) * 0.4).clamp(0.0, 1.0),
        math: (freq(&math_words) * 0.6 + (symbols / chars * 4.0).min(1.0) * 0.4).clamp(0.0, 1.0),
        science: (freq(&sci_words) * 0.8).clamp(0.0, 1.0),
        medical: (freq(&med_words) * 0.8).clamp(0.0, 1.0),
        legal: (freq(&legal_words) * 0.8).clamp(0.0, 1.0),
        news: (freq(&news_words) * 0.8).clamp(0.0, 1.0),
        fiction: (freq(&fic_words) * 0.8).clamp(0.0, 1.0),
        dialogue: (freq(&dia_words) * 0.5 + q.min(1.0) * 0.3 + fi * 0.2).clamp(0.0, 1.0),
    }
}

pub fn detect(text: &str) -> SemanticKind {
    score(text).best()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_domains() {
        assert_eq!(
            detect("python 函数 变量 接口 参数 编译 部署 git npm cargo"),
            SemanticKind::Code
        );
        assert_eq!(
            detect("定理：对于任意实数，证明该方程成立，矩阵的特征值与概率满足积分关系。"),
            SemanticKind::Math
        );
        assert_eq!(
            detect("实验假设与观测样本：物理化学与生物现象。"),
            SemanticKind::Science
        );
        assert_eq!(
            detect("患者出现发热症状，诊断为感染，给予药物治疗。"),
            SemanticKind::Medical
        );
        assert_eq!(
            detect("根据合同条款，乙方承担违约责任，法院判决生效。"),
            SemanticKind::Legal
        );
        assert_eq!(
            detect("记者报道，今日发布声明，选举结果公布。"),
            SemanticKind::News
        );
        assert_eq!(
            detect("那晚，他梦见一座城市，故事从此开始。"),
            SemanticKind::Fiction
        );
        assert_eq!(
            detect("你：今天怎么样？我：还行。他：那就好。"),
            SemanticKind::Dialogue
        );
    }

    #[test]
    fn empty_is_other_and_scores_bounded() {
        assert_eq!(detect(""), SemanticKind::Other);
        let s = score("普通文本");
        for v in [
            s.code, s.math, s.science, s.medical, s.legal, s.news, s.fiction, s.dialogue,
        ] {
            assert!((0.0..=1.0).contains(&v));
        }
    }
}
