//! Dual-rail CNF encoding for ternary expressions.
//!
//! Each ternary value is `(t, x)` with the constraint `¬(t ∧ x)`:
//! `(0,0)=False`, `(1,0)=True`, `(0,1)=Unknown`.
//! Gate outputs are encoded from the algebra truth table, exactly like
//! `trit-sat`'s symbolic encoder, so the CNF's models are in bijection
//! with the satisfying input assignments (gate rails are determined).
//! The encoder is deliberately independent of `trit-sat`'s private one:
//! `tests/crosscheck.rs` compares the positive counts with
//! `trit_sat::satisfiable` as an independent oracle.

use std::collections::{HashMap, HashSet};

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::value::Ternary;

/// A pair of literals representing one ternary value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rails {
    /// Literal that is true iff the `t` rail is one.
    pub t: i32,
    /// Literal that is true iff the `x` rail is one.
    pub x: i32,
}

/// CNF in 1-based DIMACS-style literals.
#[derive(Clone, Debug, Default)]
pub struct Cnf {
    /// Number of Boolean variables (including the shared false constant).
    pub nvars: usize,
    /// Clauses; each literal is a signed variable index.
    pub clauses: Vec<Vec<i32>>,
}

/// Builds a CNF for `expr == target` over the listed input variables.
pub struct Encoder {
    nvars: i32,
    clauses: Vec<Vec<i32>>,
    alg: Algebra,
    vars: HashMap<String, Rails>,
    binary: HashSet<String>,
    false_var: i32,
}

impl Encoder {
    /// Creates an encoder and pre-creates rails for every declared variable,
    /// including variables that do not occur in the expression.
    #[must_use]
    pub fn new(alg: Algebra, vars: &[String], binary: &HashSet<String>) -> Self {
        let mut enc = Self {
            nvars: 1,
            clauses: vec![vec![-1]],
            alg,
            vars: HashMap::new(),
            binary: binary.clone(),
            false_var: 1,
        };
        for name in vars {
            enc.var_rails(name);
        }
        enc
    }

    fn fresh(&mut self) -> i32 {
        self.nvars += 1;
        self.nvars
    }

    fn clause(&mut self, lits: &[i32]) {
        self.clauses.push(lits.to_vec());
    }

    fn pair(&mut self) -> Rails {
        let t = self.fresh();
        let x = self.fresh();
        self.clause(&[-t, -x]);
        Rails { t, x }
    }

    fn var_rails(&mut self, name: &str) -> Rails {
        if let Some(rails) = self.vars.get(name) {
            return *rails;
        }
        let t = self.fresh();
        let x = self.fresh();
        self.clause(&[-t, -x]);
        if self.binary.contains(name) {
            self.clause(&[-x]);
        }
        let rails = Rails { t, x };
        self.vars.insert(name.to_string(), rails);
        rails
    }

    /// Constants use the shared false variable; `(f,f)` is False,
    /// `(¬f,f)` is True, `(f,¬f)` is Unknown.
    fn const_rails(&self, value: Ternary) -> Rails {
        match value {
            Ternary::False => Rails {
                t: self.false_var,
                x: self.false_var,
            },
            Ternary::True => Rails {
                t: -self.false_var,
                x: self.false_var,
            },
            Ternary::Unknown => Rails {
                t: self.false_var,
                x: -self.false_var,
            },
        }
    }

    /// Encodes the expression and asserts the root has the target value.
    #[must_use]
    pub fn encode(mut self, expr: &Expr, target: Ternary) -> Cnf {
        let root = self.encode_expr(expr);
        match target {
            Ternary::False => {
                self.clause(&[-root.t]);
                self.clause(&[-root.x]);
            }
            Ternary::True => {
                self.clause(&[root.t]);
                self.clause(&[-root.x]);
            }
            Ternary::Unknown => {
                self.clause(&[-root.t]);
                self.clause(&[root.x]);
            }
        }
        Cnf {
            nvars: self.nvars as usize,
            clauses: self.clauses,
        }
    }

    fn encode_expr(&mut self, expr: &Expr) -> Rails {
        match expr {
            Expr::Const(value) => self.const_rails(*value),
            Expr::Var(name) => self.var_rails(name),
            Expr::Not(a) => {
                let a = self.encode_expr(a);
                let out = self.pair();
                let alg = self.alg;
                self.table(&[a], out, move |v| alg.not(v[0]));
                out
            }
            Expr::And(a, b) => {
                let a = self.encode_expr(a);
                let b = self.encode_expr(b);
                let out = self.pair();
                let alg = self.alg;
                self.table(&[a, b], out, move |v| alg.and(v[0], v[1]));
                out
            }
            Expr::Or(a, b) => {
                let a = self.encode_expr(a);
                let b = self.encode_expr(b);
                let out = self.pair();
                let alg = self.alg;
                self.table(&[a, b], out, move |v| alg.or(v[0], v[1]));
                out
            }
            Expr::Xor(a, b) => {
                let a = self.encode_expr(a);
                let b = self.encode_expr(b);
                let out = self.pair();
                let alg = self.alg;
                self.table(&[a, b], out, move |v| alg.strong_add(v[0], v[1]));
                out
            }
            Expr::Implies(a, b) => {
                let a = self.encode_expr(a);
                let b = self.encode_expr(b);
                let out = self.pair();
                let alg = self.alg;
                self.table(&[a, b], out, move |v| alg.implies(v[0], v[1]));
                out
            }
            Expr::Equiv(a, b) => {
                let a = self.encode_expr(a);
                let b = self.encode_expr(b);
                let out = self.pair();
                let alg = self.alg;
                self.table(&[a, b], out, move |v| alg.equiv(v[0], v[1]));
                out
            }
        }
    }

    /// For each input combination, add clauses forcing both output rails.
    fn table(&mut self, ins: &[Rails], out: Rails, f: impl Fn(&[Ternary]) -> Ternary) {
        let combos = 3usize.pow(ins.len() as u32);
        for index in 0..combos {
            let mut rest = index;
            let mut values = Vec::with_capacity(ins.len());
            let mut base = Vec::with_capacity(ins.len() * 2);
            for node in ins {
                let value = match rest % 3 {
                    0 => Ternary::False,
                    1 => Ternary::True,
                    _ => Ternary::Unknown,
                };
                rest /= 3;
                values.push(value);
                match value {
                    Ternary::False => {
                        base.push(node.t);
                        base.push(node.x);
                    }
                    Ternary::True => {
                        base.push(-node.t);
                        base.push(node.x);
                    }
                    Ternary::Unknown => {
                        base.push(node.t);
                        base.push(-node.x);
                    }
                }
            }
            let (t_pos, x_pos) = match f(&values) {
                Ternary::False => (false, false),
                Ternary::True => (true, false),
                Ternary::Unknown => (false, true),
            };
            let mut t_clause = base.clone();
            t_clause.push(if t_pos { out.t } else { -out.t });
            self.clause(&t_clause);
            let mut x_clause = base;
            x_clause.push(if x_pos { out.x } else { -out.x });
            self.clause(&x_clause);
        }
    }
}
