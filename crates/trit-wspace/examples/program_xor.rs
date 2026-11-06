//! 端到端演示：读行为 → 写补丁 → 事务式回滚 → 文本工件。
//!
//! 运行：`cargo run -p trit-wspace --example program_xor`

use trit_wspace::{
    Behavior, Edit, Layer, Net, Oracle, Patch, Program, RepairOptions, Trit, checksum, from_wsp,
    repair, to_wsp,
};

fn main() {
    // 手写 XOR：h1 = ¬OR(a,b)，h2 = ¬AND(a,b)，out = sign(-h1+h2-1)。
    let layer0 = Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("layer0");
    let layer1 = Layer::new(1, 2, &[-1, 1], &[-1]).expect("layer1");
    let xor = Oracle::boolean(2, |b| b[0] != b[1]).expect("oracle");
    let net = Net::new(vec![layer0, layer1]).expect("net");
    println!("== 读：XOR 网络的行为");
    print_table(&net, &xor);
    assert!(xor.exact(&net).expect("eval"));

    println!("\n== 写：一个三值工件（WSP1）");
    let text = to_wsp(&net);
    print!("{text}");
    println!("checksum = {:016x}", checksum(&net));
    let parsed = from_wsp(&text).expect("parse");
    assert_eq!(parsed.state_hash(), net.state_hash());

    println!("\n== 坏一格：把 layer0/row0/col0 从 + 改成 0");
    let mut broken = net.clone();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
    .apply(&mut broken)
    .expect("corrupt");
    print_table(&broken, &xor);
    let loss = xor.loss(&broken).expect("eval");
    assert!(loss > 0);

    println!("\n== 修补：无梯度、最小编辑");
    let found = repair(&broken, &xor, &RepairOptions::default())
        .expect("repair")
        .expect("exact patch in budget");
    println!("补丁 = {:?}", found.patch.edits());
    println!(
        "改动量 = {:?}（评估 {} 个候选）",
        found.patch.cost(&broken),
        found.evaluated
    );
    let mut fixed = broken.clone();
    found.patch.apply(&mut fixed).expect("apply");
    assert!(xor.exact(&fixed).expect("eval"));

    println!("\n== 事务：后置条件不成立就原样回滚");
    let before = fixed.state_hash();
    let attempted = Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }]);
    let want = Behavior::of(&fixed, &xor).expect("behavior");
    let report = Program::new("break-and-check", attempted)
        .require_after(want)
        .run(&mut fixed, &xor)
        .expect("run");
    println!(
        "applied={} rolled_back={} after_ok={}",
        report.applied, report.rolled_back, report.after_ok
    );
    assert!(report.rolled_back);
    assert_eq!(fixed.state_hash(), before);
    println!("回滚后权重哈希不变：{:016x}", fixed.state_hash());
}

fn print_table(net: &Net, oracle: &Oracle) {
    for (input, _) in oracle.inputs().iter().zip(oracle.outputs()) {
        let out = net.forward(input).expect("forward");
        let bits: String = input
            .iter()
            .map(|t| t.to_char().to_string())
            .collect::<Vec<_>>()
            .join(",");
        let result: String = out.iter().map(|t| t.to_char()).collect();
        println!("  ({bits}) -> {result}");
    }
}
