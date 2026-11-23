//! `lib` 的单元测试（从 lib.rs 拆出，控制单文件行数预算）。

use super::*;

fn chains() -> Vec<Chain> {
    (1..=5).map(|n| Chain::new(n).unwrap()).collect()
}

#[test]
fn tnorm_axioms_hold_on_small_chains() {
    for chain in chains() {
        for kind in TNorm::ALL {
            for x in chain.values() {
                for y in chain.values() {
                    // 交换律
                    assert_eq!(
                        chain.t_norm(kind, x, y).unwrap(),
                        chain.t_norm(kind, y, x).unwrap(),
                        "{kind:?} 交换律失败 x={x} y={y}"
                    );
                    // 单位元：top
                    assert_eq!(
                        chain.t_norm(kind, chain.top(), x).unwrap(),
                        x,
                        "{kind:?} 单位元失败 x={x}"
                    );
                    // 单调性
                    for x2 in x..=chain.top() {
                        assert!(
                            chain.t_norm(kind, x, y).unwrap() <= chain.t_norm(kind, x2, y).unwrap(),
                            "{kind:?} 单调性失败 x={x} x2={x2} y={y}"
                        );
                    }
                    // 结合律
                    for z in chain.values() {
                        let left = chain
                            .t_norm(kind, chain.t_norm(kind, x, y).unwrap(), z)
                            .unwrap();
                        let right = chain
                            .t_norm(kind, x, chain.t_norm(kind, y, z).unwrap())
                            .unwrap();
                        assert_eq!(left, right, "{kind:?} 结合律失败 x={x} y={y} z={z}");
                    }
                }
            }
        }
    }
}

#[test]
fn conorm_is_dual_of_norm() {
    for chain in chains() {
        for kind in TNorm::ALL {
            for x in chain.values() {
                for y in chain.values() {
                    let dual = chain
                        .neg(
                            chain
                                .t_norm(kind, chain.neg(x).unwrap(), chain.neg(y).unwrap())
                                .unwrap(),
                        )
                        .unwrap();
                    assert_eq!(chain.t_conorm(kind, x, y).unwrap(), dual);
                    assert_eq!(
                        chain.t_conorm(kind, chain.bottom(), x).unwrap(),
                        x,
                        "conorm 单位元失败"
                    );
                }
            }
        }
    }
}

#[test]
fn residuum_adjunction_holds_for_left_continuous_norms() {
    for chain in chains() {
        for kind in [TNorm::Minimum, TNorm::Lukasiewicz, TNorm::NilpotentMinimum] {
            for x in chain.values() {
                for y in chain.values() {
                    for z in chain.values() {
                        let product = chain.t_norm(kind, x, y).unwrap();
                        let implication = chain.residuum(kind, x, z).unwrap();
                        assert_eq!(
                            product <= z,
                            y <= implication,
                            "{kind:?} 伴随失败 x={x} y={y} z={z}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn luk_residuum_closed_form_and_bounded() {
    // 穷举 n=1..=6 全链：residuum 必须落在 0..=n，且等于闭式解
    // R(x, y) = min(n, max(0, n − x + y))。
    for n in 1..=6u32 {
        let chain = Chain::new(n).unwrap();
        for x in chain.values() {
            for y in chain.values() {
                let got = chain.residuum(TNorm::Lukasiewicz, x, y).unwrap();
                let want = n.min(n.saturating_sub(x).saturating_add(y));
                assert!(got <= n, "Łukasiewicz 残差越界 n={n} x={x} y={y} got={got}");
                assert_eq!(
                    got, want,
                    "闭式解不一致 n={n} x={x} y={y} got={got} want={want}"
                );
            }
        }
    }
}

#[test]
fn residuum_adjunction_holds_on_chains_up_to_six() {
    // 伴随不变量：t(x, y) ≤ z ⟺ y ≤ R(x, z)，n=1..=6 全量校验三条左连续族。
    for n in 1..=6u32 {
        let chain = Chain::new(n).unwrap();
        for kind in [TNorm::Minimum, TNorm::Lukasiewicz, TNorm::NilpotentMinimum] {
            for x in chain.values() {
                for y in chain.values() {
                    for z in chain.values() {
                        let product = chain.t_norm(kind, x, y).unwrap();
                        let implication = chain.residuum(kind, x, z).unwrap();
                        assert_eq!(
                            product <= z,
                            y <= implication,
                            "{kind:?} 伴随失败 n={n} x={x} y={y} z={z}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn drastic_has_no_residuum() {
    let chain = Chain::new(3).unwrap();
    let err = chain.residuum(TNorm::Drastic, 1, 2).unwrap_err();
    assert!(matches!(err, MvError::NoResiduum { .. }), "{err}");
}

#[test]
fn negation_and_weak_ops_are_exact() {
    let chain = Chain::new(4).unwrap();
    for x in chain.values() {
        assert_eq!(chain.neg(chain.neg(x).unwrap()).unwrap(), x);
        assert_eq!(chain.weak_and(x, chain.top()).unwrap(), x);
        assert_eq!(chain.weak_or(x, chain.bottom()).unwrap(), x);
    }
}

#[test]
fn guards_and_bounds_are_structured() {
    assert!(matches!(Chain::new(0), Err(MvError::TooFewLevels)));
    assert!(matches!(
        Chain::new(MAX_LEVELS + 1),
        Err(MvError::TooManyLevels { .. })
    ));
    let chain = Chain::new(2).unwrap();
    assert!(matches!(chain.neg(3), Err(MvError::OutOfRange { .. })));
    assert!(matches!(
        chain.t_norm(TNorm::Minimum, 1, 3),
        Err(MvError::OutOfRange { .. })
    ));
}

#[test]
fn ratios_cover_endpoints() {
    let chain = Chain::new(10).unwrap();
    assert!((chain.ratio(0) - 0.0).abs() < f64::EPSILON);
    assert!((chain.ratio(10) - 1.0).abs() < f64::EPSILON);
    assert!((chain.ratio(5) - 0.5).abs() < f64::EPSILON);
}
