//! 轨迹级重写用例：重写后仍过同一门、JSON 往返保真。

use super::*;
use crate::gate::Decision;
use trit_learn::json::Json;

struct Append;

impl Rewriter for Append {
    fn rewrite(&self, text: &str) -> Result<String, String> {
        Ok(format!("{text}（改写）"))
    }
}

const OK: &str = r#"{"task_id":"t1","model":"m","steps":[{"role":"assistant","thought":"I will run the test suite now to check the build","action":{"tool":"bash","args":{"cmd":"pytest -q"}},"observation":"3 passed"},{"role":"assistant","thought":"All tests pass so I can finish this task","action":{"tool":"finish","args":{}},"observation":"done"}],"outcome":{"verifier":"pytest","passed":true,"evidence":["sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],"final":"修复了 42 个用例"}}"#;

fn td(s: &str) -> Trajectory {
    Trajectory::from_json(&Json::parse(s).unwrap()).unwrap()
}

#[test]
fn rewrite_then_gate_keeps_accept() {
    let t = td(OK);
    let (out, fid, v) = rewrite_trajectory(&Append, &t, &Config::default()).unwrap();
    assert!(fid.ok, "{:?}", fid.missing);
    assert_eq!(v.decision, Decision::Accept, "{:?}", v.reasons);
    assert!(out.outcome.final_text.contains("42"));
    assert!(out.steps[0].thought.as_deref().unwrap().contains("改写"));
}

#[test]
fn to_json_roundtrips_a_rewritten_trajectory() {
    let t = td(OK);
    let (out, _, _) = rewrite_trajectory(&Append, &t, &Config::default()).unwrap();
    let j = to_json(&out);
    let back = Trajectory::from_json(&j).unwrap();
    assert_eq!(back, out);
    assert_eq!(back.source, out.source);
    assert_eq!(back.steps[0].args, out.steps[0].args);
}
