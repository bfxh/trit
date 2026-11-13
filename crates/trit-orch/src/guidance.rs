//! 从自然语言里抽取引导草稿。
//!
//! 这是确定性的规则抽取，不调用模型：支持显式段落标题（中英文），
//! 以及无标题时按句号 / 分号切句，用关键词区分约束与停止条件。
//! 宿主可以把它当作 fallback，再用 LLM 替换或修正。

use crate::TaskGuidance;

#[derive(Clone, Copy)]
enum Section {
    Objective,
    Constraints,
    Stop,
}

pub fn guidance_from_text(text: &str) -> TaskGuidance {
    let mut guidance = TaskGuidance::default();
    let mut section: Option<Section> = None;
    let mut prelude: Vec<&str> = Vec::new();

    for raw in text.lines() {
        let line = strip_bullet(raw);
        if line.is_empty() {
            continue;
        }
        if let Some((kind, content)) = header(line) {
            section = Some(kind);
            push_content(&mut guidance, kind, content);
            continue;
        }
        match section {
            Some(kind) => push_content(&mut guidance, kind, line),
            None => prelude.push(line),
        }
    }

    for sentence in split_sentences(&prelude.join(" ")) {
        let lower = sentence.to_ascii_lowercase();
        if is_stop_sentence(&lower, &sentence) {
            guidance.stop_when.push(sentence);
        } else if is_constraint_sentence(&lower, &sentence) {
            guidance.constraints.push(sentence);
        } else if guidance.objective.is_empty() {
            guidance.objective = sentence;
        } else {
            guidance.objective.push(' ');
            guidance.objective.push_str(&sentence);
        }
    }
    guidance
}

fn strip_bullet(line: &str) -> &str {
    line.trim_start_matches(['-', '*', '•']).trim()
}

fn header(line: &str) -> Option<(Section, &str)> {
    let (label, content) = split_header(line)?;
    let label = label.to_ascii_lowercase();
    let section = match label.as_str() {
        "objective" | "目标" => Section::Objective,
        "constraints" | "constraint" | "must" | "约束" | "硬约束" => Section::Constraints,
        "stop when" | "stop" | "停止" | "停止条件" | "何时停止" => Section::Stop,
        _ => return None,
    };
    Some((section, content))
}

fn split_header(line: &str) -> Option<(&str, &str)> {
    for (index, ch) in line.char_indices() {
        if ch == ':' || ch == '：' {
            return Some((line[..index].trim(), line[index + ch.len_utf8()..].trim()));
        }
    }
    None
}

fn push_content(guidance: &mut TaskGuidance, section: Section, content: &str) {
    let content = strip_bullet(content);
    if content.is_empty() {
        return;
    }
    match section {
        Section::Objective => {
            if guidance.objective.is_empty() {
                guidance.objective = content.to_owned();
            } else {
                guidance.objective.push(' ');
                guidance.objective.push_str(content);
            }
        }
        Section::Constraints => guidance.constraints.push(content.to_owned()),
        Section::Stop => guidance.stop_when.push(content.to_owned()),
    }
}

fn split_sentences(text: &str) -> Vec<String> {
    text.split(['。', '；', ';'])
        .map(str::trim)
        .filter(|sentence| !sentence.is_empty())
        .map(str::to_owned)
        .collect()
}

fn is_constraint_sentence(lower: &str, original: &str) -> bool {
    ["must", "never", "do not", "only"]
        .iter()
        .any(|needle| lower.contains(needle))
        || ["必须", "禁止", "不得", "只能"]
            .iter()
            .any(|needle| original.contains(needle))
}

fn is_stop_sentence(lower: &str, original: &str) -> bool {
    ["if ", "when ", "until "]
        .iter()
        .any(|needle| lower.starts_with(needle))
        || ["如果", "一旦", "当"]
            .iter()
            .any(|needle| original.starts_with(needle))
}
