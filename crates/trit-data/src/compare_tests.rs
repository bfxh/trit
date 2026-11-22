//! compare 单测。

    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn perfect_agreement_is_one() {
        let k = cohen_kappa(
            &v(&["accept", "reject", "defer"]),
            &v(&["accept", "reject", "defer"]),
        )
        .unwrap();
        assert_eq!(k.n, 3);
        assert!((k.kappa - 1.0).abs() < 1e-12);
    }

    #[test]
    fn known_zero_kappa_example() {
        let k = cohen_kappa(
            &v(&["yes", "no", "yes", "no"]),
            &v(&["yes", "no", "no", "yes"]),
        )
        .unwrap();
        assert!((k.observed - 0.5).abs() < 1e-12);
        assert!((k.expected - 0.5).abs() < 1e-12);
        assert!(k.kappa.abs() < 1e-12);
    }

    #[test]
    fn length_mismatch_is_an_error() {
        assert!(cohen_kappa(&v(&["a"]), &v(&[])).is_err());
    }
