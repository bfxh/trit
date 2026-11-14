//! 能力命名：输入模式与外部标签。

use trit_wspace::attribution::{AttributeOptions, EditMode, attribute};
use trit_wspace::capability::name_capabilities;
use trit_wspace::{Layer, Net, Oracle, Trit};

fn x0_net() -> Net {
    Net::new(vec![Layer::new(1, 2, &[1, 0], &[0]).expect("l")]).expect("net")
}

fn x0_oracle() -> Oracle {
    let inputs: Vec<Vec<Trit>> = (-1i8..=1)
        .flat_map(|a| (-1i8..=1).map(move |b| (a, b)))
        .map(|(a, b)| {
            vec![
                Trit::from_i8(a).expect("trit"),
                Trit::from_i8(b).expect("trit"),
            ]
        })
        .collect();
    let outputs = inputs.iter().map(|row| vec![row[0]]).collect();
    Oracle::new(inputs, outputs).expect("oracle")
}

fn options() -> AttributeOptions {
    AttributeOptions {
        mode: EditMode::Flip,
        top: 0,
        ..AttributeOptions::default()
    }
}

#[test]
fn names_capabilities_without_labels() {
    let oracle = x0_oracle();
    let report = attribute(&x0_net(), &oracle, &options()).expect("attribute");
    let named = name_capabilities(&oracle, &report, None);
    assert_eq!(named.len(), report.capabilities.len());
    assert!(named.iter().all(|cap| !cap.name.is_empty()));
    assert!(
        named
            .iter()
            .all(|cap| cap.name.starts_with("pattern:") || cap.name.starts_with("samples:"))
    );
}

#[test]
fn labels_take_precedence() {
    let oracle = x0_oracle();
    let report = attribute(&x0_net(), &oracle, &options()).expect("attribute");
    let labels: Vec<String> = oracle
        .inputs()
        .iter()
        .map(|row| match row[0] {
            Trit::Pos => "pos".to_string(),
            Trit::Neg => "neg".to_string(),
            Trit::Zero => "zero".to_string(),
        })
        .collect();
    let named = name_capabilities(&oracle, &report, Some(&labels));
    assert!(named.iter().all(|cap| cap.label.is_some()));
    assert!(named.iter().all(|cap| cap.name.starts_with("label:")));
}
