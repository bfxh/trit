//! 三值逻辑真值表：K3 全 9 组合逐项对拍，Ł3 蕴含对照。

use trit_immune::Trit::{self, *};

#[test]
fn k3_not() {
    assert_eq!(!Vulnerable, Hardened);
    assert_eq!(!Unknown, Unknown);
    assert_eq!(!Hardened, Vulnerable);
}

#[test]
fn k3_and_or_tables() {
    let vals = [Vulnerable, Unknown, Hardened];
    for a in vals {
        for b in vals {
            // AND：-1 支配，否则 0 传染
            let exp_and = match (a, b) {
                (Vulnerable, _) | (_, Vulnerable) => Vulnerable,
                (Unknown, _) | (_, Unknown) => Unknown,
                _ => Hardened,
            };
            assert_eq!(a.and(b), exp_and, "and({a}, {b})");

            // OR：+1 支配，否则 0 传染
            let exp_or = match (a, b) {
                (Hardened, _) | (_, Hardened) => Hardened,
                (Unknown, _) | (_, Unknown) => Unknown,
                _ => Vulnerable,
            };
            assert_eq!(a.or(b), exp_or, "or({a}, {b})");
        }
    }
}

#[test]
fn k3_implies_table() {
    let vals = [Vulnerable, Unknown, Hardened];
    // table[a][b]
    let table = [
        [Hardened, Hardened, Hardened],  // a = Vulnerable（空真）
        [Unknown, Unknown, Hardened],    // a = Unknown
        [Vulnerable, Unknown, Hardened], // a = Hardened
    ];
    for (i, &a) in vals.iter().enumerate() {
        for (j, &b) in vals.iter().enumerate() {
            assert_eq!(a.implies_k3(b), table[i][j], "k3 {a} => {b}");
            // K3 蕴含恒等于 ¬a ∨ b
            assert_eq!(a.implies_k3(b), (!a).or(b));
        }
    }
}

#[test]
fn lukasiewicz_implies_table() {
    let vals = [Vulnerable, Unknown, Hardened];
    let table = [
        [Hardened, Hardened, Hardened],  // a = Vulnerable
        [Unknown, Hardened, Hardened],   // a = Unknown ← 与 K3 的差别
        [Vulnerable, Unknown, Hardened], // a = Hardened
    ];
    for (i, &a) in vals.iter().enumerate() {
        for (j, &b) in vals.iter().enumerate() {
            assert_eq!(a.implies_l3(b), table[i][j], "ł3 {a} => {b}");
        }
    }
    // 两套蕴含恰好只在 (0 => 0) 上分岔
    assert_eq!(Unknown.implies_k3(Unknown), Unknown);
    assert_eq!(Unknown.implies_l3(Unknown), Hardened);
}

#[test]
fn k3_xor_table() {
    let vals = [Vulnerable, Unknown, Hardened];
    let table = [
        [Vulnerable, Unknown, Hardened],
        [Unknown, Unknown, Unknown],
        [Hardened, Unknown, Vulnerable],
    ];
    for (i, &a) in vals.iter().enumerate() {
        for (j, &b) in vals.iter().enumerate() {
            assert_eq!(a.xor(b), table[i][j], "xor({a}, {b})");
        }
    }
}

#[test]
fn folds_and_identities() {
    assert_eq!(Trit::fold_and(Vec::<Trit>::new()), Hardened);
    assert_eq!(Trit::fold_or(Vec::<Trit>::new()), Vulnerable);
    assert_eq!(Trit::fold_and([Hardened, Unknown]), Unknown);
    assert_eq!(Trit::fold_and([Unknown, Vulnerable]), Vulnerable);
    assert_eq!(Trit::fold_or([Unknown, Vulnerable]), Unknown);
    assert_eq!(Trit::fold_or([Vulnerable, Hardened]), Hardened);
}

#[test]
fn parsing_and_display() {
    assert_eq!("vuln".parse::<Trit>(), Ok(Vulnerable));
    assert_eq!("  +1 ".parse::<Trit>(), Ok(Hardened));
    assert_eq!("0".parse::<Trit>(), Ok(Unknown));
    assert!("2".parse::<Trit>().is_err());
    assert_eq!(Hardened.to_string(), "+1");
    assert_eq!(Vulnerable.symbol(), "✗");
    assert_eq!(Trit::from_i8(2), None);
}
