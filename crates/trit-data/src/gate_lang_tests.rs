//! 语言三支映射的用例：strict 把 Uncertain 也转人工、Bad 进硬门；off 只剩识别。

use super::super::*;
use super::td;
use crate::language::LangClass;

/// 无教育性结构词、但也不重复不空白的文本 → `Uncertain`。
const PLAIN: &str = "今天下午的天气比较平稳，窗口外的树影缓慢移动。桌面上放着几个不同大小的杯子，旁边有一本翻开的册子。走廊里偶尔传来脚步声，随后又恢复安静。记录里写下了时间、地点和简单的观察，没有额外的解释。远处有一辆小车经过，颜色和上午看到的那辆不同。人们按自己的节奏处理手头的事情，偶尔停下来看看窗外。这个场景没有特别的说明，只是把看到的内容按顺序写下来，供以后核对。文件中还包含若干编号和日期，便于定位到具体的那一行记录。";

fn plain_trajectory() -> Trajectory {
    let json = format!(
        r#"{{"task_id":"t4","model":"m","steps":[{{"role":"assistant","thought":"先看当前目录里有什么文件","action":{{"tool":"bash","args":{{"cmd":"ls"}}}},"observation":"列出了三个子目录"}},{{"role":"assistant","thought":"再确认一下配置文件的位置","action":{{"tool":"finish","args":{{}}}},"observation":"找到配置并记录路径"}}],"outcome":{{"verifier":"manual","passed":true,"evidence":["sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],"final":"{PLAIN}"}}}}"#
    );
    td(&json)
}

fn repeated_trajectory(task: &str) -> Trajectory {
    let bad = "重复句子。".repeat(80);
    let json = format!(
        r#"{{"task_id":"{task}","model":"m","steps":[{{"role":"assistant","thought":"{bad}","action":{{"tool":"bash","args":{{"cmd":"ls"}}}},"observation":"{bad}"}},{{"role":"assistant","thought":"{bad}","action":{{"tool":"finish","args":{{}}}},"observation":"{bad}"}}],"outcome":{{"verifier":"pytest","passed":true,"evidence":["sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],"final":"{bad}"}}}}"#
    );
    td(&json)
}

#[test]
fn strict_language_policy_splits_bad_and_uncertain() {
    let strict = Config {
        lang_policy: LangPolicy::Strict,
        ..Config::default()
    };
    let mut seen = BTreeSet::new();
    let v = evaluate(&repeated_trajectory("t5"), &strict, &mut seen);
    assert_eq!(v.decision, Decision::Reject, "{:?}", v.reasons);
    assert!(v.reasons.contains(&"language_bad"), "{:?}", v.reasons);

    let t = plain_trajectory();
    assert_eq!(
        classify(&features(&trajectory_text(&t)), &Limits::default()),
        LangClass::Uncertain
    );
    let mut seen = BTreeSet::new();
    let v = evaluate(&t, &strict, &mut seen);
    assert_eq!(v.decision, Decision::Defer, "{:?}", v.reasons);
    assert_eq!(v.reasons, vec!["language_uncertain"]);
}

#[test]
fn language_policy_off_and_defer_differ_on_bad_text() {
    let t = plain_trajectory();
    let mut seen = BTreeSet::new();
    let v = evaluate(&t, &Config::default(), &mut seen);
    assert_eq!(v.decision, Decision::Accept, "{:?}", v.reasons);

    let off = Config {
        lang_policy: LangPolicy::Off,
        ..Config::default()
    };
    let mut seen = BTreeSet::new();
    let v = evaluate(&repeated_trajectory("t6"), &off, &mut seen);
    assert_eq!(v.decision, Decision::Accept, "{:?}", v.reasons);
}
