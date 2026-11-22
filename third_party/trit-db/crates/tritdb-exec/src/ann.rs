//! Annotated relations: K3 / Belnap / Łukasiewicz / ℕ annotations, exact
//! model counting, and consistent query answering over minimal repairs.
//!
//! This is the semantic core of M3. It deliberately does not touch the SQL
//! parser yet: callers build [`BoolExpr`] or fact sets directly.

use std::collections::{BTreeSet, HashMap};
use std::fmt;

use crate::TruthValue;

/// Maximum variables accepted by [`count_models`].
pub const MAX_MODEL_VARS: usize = 32;
/// Maximum facts accepted by [`under_repair`].
pub const MAX_REPAIR_FACTS: usize = 64;
/// Maximum facts inside one connected component of the denial graph.
pub const MAX_COMPONENT_FACTS: usize = 16;

/// Belnap four-valued truth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum B4 {
    /// True in every model.
    True,
    /// False in every model.
    False,
    /// Both true and false (conflict).
    Both,
    /// Neither true nor false (no information).
    Neither,
}

impl B4 {
    /// Stable numeric code used by storage and ordering.
    pub const fn code(self) -> u8 {
        match self {
            B4::False => 0,
            B4::Neither => 1,
            B4::Both => 2,
            B4::True => 3,
        }
    }

    /// Parses [`B4::code`].
    pub const fn from_code(value: u8) -> Option<Self> {
        match value {
            0 => Some(B4::False),
            1 => Some(B4::Neither),
            2 => Some(B4::Both),
            3 => Some(B4::True),
            _ => None,
        }
    }

    /// Maps a two-valued boolean into Belnap.
    pub const fn from_bool(value: bool) -> Self {
        if value {
            B4::True
        } else {
            B4::False
        }
    }

    /// Stable display text.
    pub const fn as_str(self) -> &'static str {
        match self {
            B4::True => "TRUE",
            B4::False => "FALSE",
            B4::Both => "BOTH",
            B4::Neither => "NEITHER",
        }
    }

    /// Belnap negation: `True <-> False`, `Both` and `Neither` are fixed.
    #[allow(clippy::should_implement_trait)]
    pub const fn neg(self) -> Self {
        match self {
            B4::True => B4::False,
            B4::False => B4::True,
            B4::Both => B4::Both,
            B4::Neither => B4::Neither,
        }
    }

    /// Conjunction (meet in the truth lattice).
    pub const fn meet(self, other: Self) -> Self {
        match (self, other) {
            (B4::False, _) | (_, B4::False) => B4::False,
            (B4::True, value) | (value, B4::True) => value,
            (B4::Both, B4::Both) => B4::Both,
            (B4::Neither, B4::Neither) => B4::Neither,
            _ => B4::False,
        }
    }

    /// Disjunction (join in the truth lattice).
    pub const fn join(self, other: Self) -> Self {
        match (self, other) {
            (B4::True, _) | (_, B4::True) => B4::True,
            (B4::False, value) | (value, B4::False) => value,
            (B4::Both, B4::Both) => B4::Both,
            (B4::Neither, B4::Neither) => B4::Neither,
            _ => B4::True,
        }
    }
}

impl PartialOrd for B4 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for B4 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.code().cmp(&other.code())
    }
}

/// An annotation from one of four semirings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Annotation {
    /// Kleene three-valued logic (same truth table as SQL `NULL`).
    K3(TruthValue),
    /// Belnap four-valued logic.
    Belnap(B4),
    /// Łukasiewicz `[0, 1]` chain.
    Lukasiewicz(f64),
    /// Natural numbers as a counting semiring.
    Nat(u64),
}

impl Annotation {
    /// Builds a Łukasiewicz annotation, rejecting values outside `[0, 1]`.
    pub fn lukasiewicz(value: f64) -> Result<Self, AnnError> {
        if (0.0..=1.0).contains(&value) {
            Ok(Annotation::Lukasiewicz(value))
        } else {
            Err(AnnError::LukasiewiczOutOfRange)
        }
    }

    /// Negation. `Nat` has no negation in the counting semiring.
    #[allow(clippy::should_implement_trait)]
    pub fn neg(self) -> Result<Self, AnnError> {
        match self {
            Annotation::K3(value) => Ok(Annotation::K3(value.not())),
            Annotation::Belnap(value) => Ok(Annotation::Belnap(value.neg())),
            Annotation::Lukasiewicz(value) => Ok(Annotation::Lukasiewicz(1.0 - value)),
            Annotation::Nat(_) => Err(AnnError::NegationUndefined("Nat")),
        }
    }

    /// Conjunction. K3/Belnap use their truth tables; Łukasiewicz uses `min`;
    /// `Nat` uses multiplication (independent-event counting).
    pub fn meet(self, other: Self) -> Result<Self, AnnError> {
        match (self, other) {
            (Annotation::K3(left), Annotation::K3(right)) => Ok(Annotation::K3(left.and(right))),
            (Annotation::Belnap(left), Annotation::Belnap(right)) => {
                Ok(Annotation::Belnap(left.meet(right)))
            }
            (Annotation::Lukasiewicz(left), Annotation::Lukasiewicz(right)) => {
                Ok(Annotation::Lukasiewicz(left.min(right)))
            }
            (Annotation::Nat(left), Annotation::Nat(right)) => {
                Ok(Annotation::Nat(left.saturating_mul(right)))
            }
            _ => Err(AnnError::DomainMismatch),
        }
    }

    /// Disjunction. K3/Belnap use their truth tables; Łukasiewicz uses `max`;
    /// `Nat` uses addition (union counting).
    pub fn join(self, other: Self) -> Result<Self, AnnError> {
        match (self, other) {
            (Annotation::K3(left), Annotation::K3(right)) => Ok(Annotation::K3(left.or(right))),
            (Annotation::Belnap(left), Annotation::Belnap(right)) => {
                Ok(Annotation::Belnap(left.join(right)))
            }
            (Annotation::Lukasiewicz(left), Annotation::Lukasiewicz(right)) => {
                Ok(Annotation::Lukasiewicz(left.max(right)))
            }
            (Annotation::Nat(left), Annotation::Nat(right)) => {
                Ok(Annotation::Nat(left.saturating_add(right)))
            }
            _ => Err(AnnError::DomainMismatch),
        }
    }

    /// Conjunction under an explicit t-norm family.
    ///
    /// Only Łukasiewicz annotations use the family; K3 and Belnap keep their
    /// truth tables and ℕ its multiplication.
    pub fn meet_with(self, other: Self, family: TNorm) -> Result<Self, AnnError> {
        match (self, other) {
            (Annotation::Lukasiewicz(left), Annotation::Lukasiewicz(right)) => {
                Ok(Annotation::Lukasiewicz(family.meet(left, right)))
            }
            _ => self.meet(other),
        }
    }

    /// Disjunction under an explicit t-conorm family.
    pub fn join_with(self, other: Self, family: TNorm) -> Result<Self, AnnError> {
        match (self, other) {
            (Annotation::Lukasiewicz(left), Annotation::Lukasiewicz(right)) => {
                Ok(Annotation::Lukasiewicz(family.join(left, right)))
            }
            _ => self.join(other),
        }
    }

    /// Material implication.
    ///
    /// K3 / Belnap use `¬a ∨ b`; Łukasiewicz uses the residuum of `family`;
    /// ℕ has no implication in the counting semiring.
    pub fn implication_with(self, other: Self, family: TNorm) -> Result<Self, AnnError> {
        match (self, other) {
            (Annotation::Lukasiewicz(left), Annotation::Lukasiewicz(right)) => {
                Ok(Annotation::Lukasiewicz(family.residuum(left, right)))
            }
            (Annotation::Nat(_), _) | (_, Annotation::Nat(_)) => {
                Err(AnnError::NegationUndefined("Nat"))
            }
            _ => self.neg()?.join(other),
        }
    }

    /// Verdict on whether this annotation forces a statement.
    ///
    /// K3 / Belnap map their truth tables directly. Łukasiewicz and ℕ keep a
    /// `Contingent` middle band: a value below the threshold is not "false",
    /// it is "partly true", so only `0` is `Impossible`.
    pub fn certainty(self, threshold: Option<f64>) -> Result<Certainty, AnnError> {
        match self {
            Annotation::K3(TruthValue::True) => Ok(Certainty::Certain),
            Annotation::K3(TruthValue::False) => Ok(Certainty::Impossible),
            Annotation::K3(TruthValue::Unknown) => Ok(Certainty::Contingent),
            Annotation::Belnap(B4::True) => Ok(Certainty::Certain),
            Annotation::Belnap(B4::False) => Ok(Certainty::Impossible),
            Annotation::Belnap(B4::Both | B4::Neither) => Ok(Certainty::Contingent),
            Annotation::Lukasiewicz(value) => {
                let threshold = threshold.unwrap_or(0.5);
                if !(0.0..=1.0).contains(&threshold) {
                    return Err(AnnError::LukasiewiczOutOfRange);
                }
                if value >= threshold {
                    Ok(Certainty::Certain)
                } else if value <= 0.0 {
                    Ok(Certainty::Impossible)
                } else {
                    Ok(Certainty::Contingent)
                }
            }
            Annotation::Nat(value) => {
                let threshold = threshold.unwrap_or(1.0).max(0.0);
                #[allow(clippy::cast_precision_loss)]
                let scaled = value as f64;
                if scaled >= threshold {
                    Ok(Certainty::Certain)
                } else if value == 0 {
                    Ok(Certainty::Impossible)
                } else {
                    Ok(Certainty::Contingent)
                }
            }
        }
    }
}

/// A triangular norm family over `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TNorm {
    /// Gödel: meet `min`, conorm `max` (the default used by [`Annotation`]).
    Godel,
    /// Łukasiewicz: meet `max(0, a + b - 1)`, conorm `min(1, a + b)`.
    Lukasiewicz,
    /// Product (Goguen): meet `a * b`, conorm `a + b - a * b`.
    Product,
}

impl TNorm {
    /// Every family, in a stable order.
    pub const ALL: [TNorm; 3] = [TNorm::Godel, TNorm::Lukasiewicz, TNorm::Product];

    /// Name used by CLI output and errors.
    pub const fn name(self) -> &'static str {
        match self {
            TNorm::Godel => "godel",
            TNorm::Lukasiewicz => "lukasiewicz",
            TNorm::Product => "product",
        }
    }

    /// Resolves a family from its name (`godel` / `lukasiewicz` / `product`).
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "godel" | "gödel" | "min" => Some(TNorm::Godel),
            "lukasiewicz" | "luk" => Some(TNorm::Lukasiewicz),
            "product" | "prob" => Some(TNorm::Product),
            _ => None,
        }
    }

    /// Conjunction over `[0, 1]`.
    pub fn meet(self, left: f64, right: f64) -> f64 {
        match self {
            TNorm::Godel => left.min(right),
            TNorm::Lukasiewicz => (left + right - 1.0).max(0.0),
            TNorm::Product => left * right,
        }
    }

    /// Disjunction over `[0, 1]`.
    pub fn join(self, left: f64, right: f64) -> f64 {
        match self {
            TNorm::Godel => left.max(right),
            TNorm::Lukasiewicz => (left + right).min(1.0),
            TNorm::Product => left + right - left * right,
        }
    }

    /// Residuum `a → b = sup { z : meet(a, z) ≤ b }`.
    pub fn residuum(self, left: f64, right: f64) -> f64 {
        if left <= right {
            1.0
        } else {
            match self {
                TNorm::Godel => right,
                TNorm::Lukasiewicz => 1.0 - left + right,
                TNorm::Product => right / left,
            }
        }
    }
}

/// Errors from annotation evaluation and counting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnnError {
    /// Operations mixed two annotation domains.
    DomainMismatch,
    /// A Łukasiewicz value was outside `[0, 1]`.
    LukasiewiczOutOfRange,
    /// The domain has no negation.
    NegationUndefined(&'static str),
    /// Too many variables for exact enumeration.
    TooManyVars {
        /// Requested variables.
        vars: usize,
        /// Maximum supported.
        max: usize,
    },
    /// Too many facts for exact repair enumeration.
    TooManyFacts {
        /// Requested facts.
        facts: usize,
        /// Maximum supported.
        max: usize,
    },
    /// A variable or fact index was outside the declared range.
    OutOfRange {
        /// Requested index.
        index: usize,
        /// Declared count.
        count: usize,
    },
    /// A boolean expression could not be parsed.
    Parse(String),
}

impl fmt::Display for AnnError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnnError::DomainMismatch => write!(formatter, "annotation domains do not match"),
            AnnError::LukasiewiczOutOfRange => {
                write!(formatter, "Lukasiewicz annotation must be in [0, 1]")
            }
            AnnError::NegationUndefined(domain) => {
                write!(formatter, "{domain} annotations have no negation")
            }
            AnnError::TooManyVars { vars, max } => {
                write!(formatter, "too many variables: {vars} > {max}")
            }
            AnnError::TooManyFacts { facts, max } => {
                write!(formatter, "too many facts: {facts} > {max}")
            }
            AnnError::OutOfRange { index, count } => {
                write!(formatter, "index {index} is outside 0..{count}")
            }
            AnnError::Parse(reason) => write!(formatter, "bad boolean expression: {reason}"),
        }
    }
}

impl std::error::Error for AnnError {}

/// Boolean expression used by [`count_models`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoolExpr {
    /// Variable by index.
    Var(usize),
    /// Constant.
    Const(bool),
    /// Negation.
    Not(Box<BoolExpr>),
    /// Conjunction.
    And(Vec<BoolExpr>),
    /// Disjunction.
    Or(Vec<BoolExpr>),
}

impl BoolExpr {
    /// Evaluates under `assignment[i]` for `Var(i)`.
    pub fn eval(&self, assignment: &[bool]) -> Result<bool, AnnError> {
        match self {
            BoolExpr::Const(value) => Ok(*value),
            BoolExpr::Var(index) => assignment.get(*index).copied().ok_or(AnnError::OutOfRange {
                index: *index,
                count: assignment.len(),
            }),
            BoolExpr::Not(inner) => Ok(!inner.eval(assignment)?),
            BoolExpr::And(items) => {
                for item in items {
                    if !item.eval(assignment)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            BoolExpr::Or(items) => {
                for item in items {
                    if item.eval(assignment)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }
}

/// Counts satisfying assignments exactly by enumeration.
///
/// `vars` must not exceed [`MAX_MODEL_VARS`]; this is an exact counter, not a
/// sampler.
pub fn count_models(expr: &BoolExpr, vars: usize) -> Result<u64, AnnError> {
    count_models_components(expr, vars)
}

/// DPLL model counter, kept as the independent oracle for the default backend.
pub fn count_models_dpll(expr: &BoolExpr, vars: usize) -> Result<u64, AnnError> {
    if vars > MAX_MODEL_VARS {
        return Err(AnnError::TooManyVars {
            vars,
            max: MAX_MODEL_VARS,
        });
    }
    check_vars(expr, vars)?;
    let mut assignment = vec![None; vars];
    Ok(count_models_dpll_inner(expr, &mut assignment))
}

fn check_vars(expr: &BoolExpr, vars: usize) -> Result<(), AnnError> {
    match expr {
        BoolExpr::Var(index) if *index >= vars => Err(AnnError::OutOfRange {
            index: *index,
            count: vars,
        }),
        BoolExpr::Var(_) | BoolExpr::Const(_) => Ok(()),
        BoolExpr::Not(inner) => check_vars(inner, vars),
        BoolExpr::And(items) | BoolExpr::Or(items) => {
            for item in items {
                check_vars(item, vars)?;
            }
            Ok(())
        }
    }
}

/// DPLL-style exact model counter with partial evaluation and short-circuiting.
fn count_models_dpll_inner(expr: &BoolExpr, assignment: &mut [Option<bool>]) -> u64 {
    match eval_partial(expr, assignment) {
        Some(true) => {
            let free = assignment.iter().filter(|value| value.is_none()).count();
            return 1u64 << free;
        }
        Some(false) => return 0,
        None => {}
    }
    let Some(index) = assignment.iter().position(|value| value.is_none()) else {
        return 0;
    };
    assignment[index] = Some(true);
    let mut models = count_models_dpll_inner(expr, assignment);
    assignment[index] = Some(false);
    models += count_models_dpll_inner(expr, assignment);
    assignment[index] = None;
    models
}

fn eval_partial(expr: &BoolExpr, assignment: &[Option<bool>]) -> Option<bool> {
    match expr {
        BoolExpr::Const(value) => Some(*value),
        BoolExpr::Var(index) => assignment.get(*index).copied().flatten(),
        BoolExpr::Not(inner) => eval_partial(inner, assignment).map(|value| !value),
        BoolExpr::And(items) => {
            let mut unknown = false;
            for item in items {
                match eval_partial(item, assignment) {
                    Some(false) => return Some(false),
                    Some(true) => {}
                    None => unknown = true,
                }
            }
            if unknown {
                None
            } else {
                Some(true)
            }
        }
        BoolExpr::Or(items) => {
            let mut unknown = false;
            for item in items {
                match eval_partial(item, assignment) {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => unknown = true,
                }
            }
            if unknown {
                None
            } else {
                Some(false)
            }
        }
    }
}

/// Answer of a query under all minimal repairs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepairAnswer {
    /// True in every minimal repair.
    Certain,
    /// False in every minimal repair.
    Impossible,
    /// True in some repairs and false in others (Belnap `Both`).
    Contingent,
}

impl RepairAnswer {
    /// Same verdict vocabulary as [`Annotation::certainty`].
    pub const fn certainty(self) -> Certainty {
        match self {
            RepairAnswer::Certain => Certainty::Certain,
            RepairAnswer::Contingent => Certainty::Contingent,
            RepairAnswer::Impossible => Certainty::Impossible,
        }
    }
}

/// Verdict on whether a statement is forced by the data.
///
/// This is the shared vocabulary of [`Annotation::certainty`] and
/// [`RepairAnswer::certainty`]: both answer the same question, "does this
/// hold in every model / repair?", so callers (and models) see one word.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Certainty {
    /// Holds in every model or repair.
    Certain,
    /// Holds in some but not all.
    Contingent,
    /// Holds in none.
    Impossible,
}

impl Certainty {
    /// Stable uppercase name used by SQL output.
    pub const fn as_str(self) -> &'static str {
        match self {
            Certainty::Certain => "CERTAIN",
            Certainty::Contingent => "CONTINGENT",
            Certainty::Impossible => "IMPOSSIBLE",
        }
    }
}

/// Consistent query answering over minimal repairs.
///
/// `denials` are sets of facts that must not all hold; a repair is a maximal
/// consistent subset of the facts. `query` is a set of facts that must all
/// hold.
pub fn under_repair(
    facts: usize,
    denials: &[Vec<usize>],
    query: &[usize],
) -> Result<RepairAnswer, AnnError> {
    if facts > MAX_REPAIR_FACTS {
        return Err(AnnError::TooManyFacts {
            facts,
            max: MAX_REPAIR_FACTS,
        });
    }
    for denial in denials {
        for index in denial {
            if *index >= facts {
                return Err(AnnError::OutOfRange {
                    index: *index,
                    count: facts,
                });
            }
        }
    }
    for index in query {
        if *index >= facts {
            return Err(AnnError::OutOfRange {
                index: *index,
                count: facts,
            });
        }
    }
    // An empty denial cannot be satisfied, so no repair exists. Checked before
    // decomposition: such a denial has no anchor fact and would be dropped
    // when denials are assigned to components.
    if denials.iter().any(|denial| denial.is_empty()) {
        return Ok(RepairAnswer::Impossible);
    }

    // Denials only connect facts inside one component. Answers compose
    // independently, so each component is enumerated on its own.
    let mut any_impossible = false;
    let mut any_contingent = false;
    for component in components(facts, denials) {
        let position: std::collections::BTreeMap<usize, usize> = component
            .iter()
            .enumerate()
            .map(|(local, &fact)| (fact, local))
            .collect();
        let local_denials: Vec<Vec<usize>> = denials
            .iter()
            .filter(|denial| {
                denial
                    .first()
                    .is_some_and(|first| position.contains_key(first))
            })
            .map(|denial| denial.iter().map(|fact| position[fact]).collect())
            .collect();
        let local_query: Vec<usize> = query
            .iter()
            .filter_map(|fact| position.get(fact).copied())
            .collect();
        match component_answer(component.len(), &local_denials, &local_query)? {
            RepairAnswer::Impossible => any_impossible = true,
            RepairAnswer::Contingent => any_contingent = true,
            RepairAnswer::Certain => {}
        }
    }

    Ok(if any_impossible {
        RepairAnswer::Impossible
    } else if any_contingent {
        RepairAnswer::Contingent
    } else {
        RepairAnswer::Certain
    })
}

fn component_answer(
    facts: usize,
    denials: &[Vec<usize>],
    query: &[usize],
) -> Result<RepairAnswer, AnnError> {
    if facts > MAX_COMPONENT_FACTS {
        return Err(AnnError::TooManyFacts {
            facts,
            max: MAX_COMPONENT_FACTS,
        });
    }
    // Unit propagation: a one-fact denial pins that fact out of every repair,
    // so it must fail the query no matter what. An empty denial can never be
    // satisfied, which leaves no repair at all.
    let mut forced: u64 = 0;
    for denial in denials {
        match denial.len() {
            0 => return Ok(RepairAnswer::Impossible),
            1 => forced |= 1 << denial[0],
            _ => {}
        }
    }
    if query.iter().any(|index| forced & (1 << index) != 0) {
        return Ok(RepairAnswer::Impossible);
    }
    // Only the surviving facts are enumerated; forced removals drop out of
    // the search space entirely.
    let free: Vec<usize> = (0..facts)
        .filter(|index| forced & (1 << index) == 0)
        .collect();
    if free.len() > MAX_COMPONENT_FACTS {
        return Err(AnnError::TooManyFacts {
            facts: free.len(),
            max: MAX_COMPONENT_FACTS,
        });
    }
    let mut certain = true;
    let mut possible = false;
    for combination in 0..(1u64 << free.len()) {
        let mut mask = 0u64;
        for (bit, &index) in free.iter().enumerate() {
            if combination & (1 << bit) != 0 {
                mask |= 1 << index;
            }
        }
        if !consistent(mask, denials) || !maximal(mask, facts, denials) {
            continue;
        }
        let holds = query.iter().all(|index| mask & (1 << index) != 0);
        if holds {
            possible = true;
        } else {
            certain = false;
        }
    }
    Ok(if certain {
        RepairAnswer::Certain
    } else if possible {
        RepairAnswer::Contingent
    } else {
        RepairAnswer::Impossible
    })
}

fn components(facts: usize, denials: &[Vec<usize>]) -> Vec<Vec<usize>> {
    fn find(parent: &mut [usize], mut value: usize) -> usize {
        while parent[value] != value {
            parent[value] = parent[parent[value]];
            value = parent[value];
        }
        value
    }

    let mut parent: Vec<usize> = (0..facts).collect();
    for denial in denials {
        if let Some(&first) = denial.first() {
            for &other in &denial[1..] {
                let left = find(&mut parent, first);
                let right = find(&mut parent, other);
                if left != right {
                    parent[left] = right;
                }
            }
        }
    }

    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> =
        std::collections::BTreeMap::new();
    for fact in 0..facts {
        let root = find(&mut parent, fact);
        groups.entry(root).or_default().push(fact);
    }
    groups.into_values().collect()
}

fn consistent(mask: u64, denials: &[Vec<usize>]) -> bool {
    denials
        .iter()
        .all(|denial| !denial.iter().all(|index| mask & (1 << index) != 0))
}

fn maximal(mask: u64, facts: usize, denials: &[Vec<usize>]) -> bool {
    (0..facts).all(|index| mask & (1 << index) != 0 || !consistent(mask | (1 << index), denials))
}

/// Parses a boolean expression over `a`..`z`, `vN`, or `N` variables.
///
/// Returns the expression and the minimal variable count (maximum index + 1).
pub fn parse_bool_expr(text: &str) -> Result<(BoolExpr, usize), AnnError> {
    let expr = BoolParser::new(text).parse()?;
    let mut max_index = None;
    collect_vars(&expr, &mut max_index);
    let vars = max_index.map_or(0, |index| index + 1);
    if vars > MAX_MODEL_VARS {
        return Err(AnnError::TooManyVars {
            vars,
            max: MAX_MODEL_VARS,
        });
    }
    Ok((expr, vars))
}

/// Counts models by splitting a top-level conjunction into independent variable
/// components, counting each with the BDD backend, and multiplying.
pub fn count_models_components(expr: &BoolExpr, vars: usize) -> Result<u64, AnnError> {
    if vars > MAX_MODEL_VARS {
        return Err(AnnError::TooManyVars {
            vars,
            max: MAX_MODEL_VARS,
        });
    }
    check_vars(expr, vars)?;

    let items = match expr {
        BoolExpr::And(items) if items.len() > 1 => items,
        _ => return count_models_bdd(expr, vars),
    };

    let mut parent: Vec<usize> = (0..vars).collect();
    fn find(parent: &mut [usize], mut value: usize) -> usize {
        while parent[value] != value {
            parent[value] = parent[parent[value]];
            value = parent[value];
        }
        value
    }

    let mut item_vars: Vec<BTreeSet<usize>> = Vec::with_capacity(items.len());
    for item in items {
        let mut set = BTreeSet::new();
        collect_expr_vars(item, &mut set);
        let mut iter = set.iter();
        if let Some(&first) = iter.next() {
            for &other in iter {
                let left = find(&mut parent, first);
                let right = find(&mut parent, other);
                if left != right {
                    parent[left] = right;
                }
            }
        }
        item_vars.push(set);
    }

    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (index, set) in item_vars.iter().enumerate() {
        let root = set
            .iter()
            .next()
            .map_or(usize::MAX, |&value| find(&mut parent, value));
        groups.entry(root).or_default().push(index);
    }

    let mut total = 1u64;
    let mut used = 0usize;
    for (root, indices) in groups {
        let mut local_vars = BTreeSet::new();
        for &index in &indices {
            collect_expr_vars(&items[index], &mut local_vars);
        }
        if root == usize::MAX || local_vars.is_empty() {
            let group_expr =
                BoolExpr::And(indices.iter().map(|&index| items[index].clone()).collect());
            total = total.saturating_mul(count_models_bdd(&group_expr, 0)?);
            continue;
        }
        let map: std::collections::BTreeMap<usize, usize> = local_vars
            .iter()
            .enumerate()
            .map(|(local, &global)| (global, local))
            .collect();
        let local_items: Vec<BoolExpr> = indices
            .iter()
            .map(|&index| remap_expr(&items[index], &map))
            .collect();
        let group_expr = if local_items.len() == 1 {
            local_items[0].clone()
        } else {
            BoolExpr::And(local_items)
        };
        total = total.saturating_mul(count_models_bdd(&group_expr, local_vars.len())?);
        used += local_vars.len();
    }

    // Variables that never appear are free in every model.
    total = total.saturating_mul(1u64 << (vars - used));
    Ok(total)
}

fn collect_expr_vars(expr: &BoolExpr, out: &mut BTreeSet<usize>) {
    match expr {
        BoolExpr::Const(_) => {}
        BoolExpr::Var(index) => {
            out.insert(*index);
        }
        BoolExpr::Not(inner) => collect_expr_vars(inner, out),
        BoolExpr::And(items) | BoolExpr::Or(items) => {
            for item in items {
                collect_expr_vars(item, out);
            }
        }
    }
}

fn remap_expr(expr: &BoolExpr, map: &std::collections::BTreeMap<usize, usize>) -> BoolExpr {
    match expr {
        BoolExpr::Const(value) => BoolExpr::Const(*value),
        BoolExpr::Var(index) => BoolExpr::Var(map.get(index).copied().unwrap_or(*index)),
        BoolExpr::Not(inner) => BoolExpr::Not(Box::new(remap_expr(inner, map))),
        BoolExpr::And(items) => {
            BoolExpr::And(items.iter().map(|item| remap_expr(item, map)).collect())
        }
        BoolExpr::Or(items) => {
            BoolExpr::Or(items.iter().map(|item| remap_expr(item, map)).collect())
        }
    }
}

fn collect_vars(expr: &BoolExpr, max_index: &mut Option<usize>) {
    match expr {
        BoolExpr::Var(index) => {
            *max_index = Some(max_index.map_or(*index, |current| current.max(*index)));
        }
        BoolExpr::Const(_) => {}
        BoolExpr::Not(inner) => collect_vars(inner, max_index),
        BoolExpr::And(items) | BoolExpr::Or(items) => {
            for item in items {
                collect_vars(item, max_index);
            }
        }
    }
}

struct BoolParser<'a> {
    input: &'a [u8],
    position: usize,
}

impl<'a> BoolParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            position: 0,
        }
    }

    fn parse(mut self) -> Result<BoolExpr, AnnError> {
        let expr = self.parse_or()?;
        self.skip_ws();
        if self.position < self.input.len() {
            return Err(AnnError::Parse(format!(
                "unexpected input at byte {}",
                self.position
            )));
        }
        Ok(expr)
    }

    fn parse_or(&mut self) -> Result<BoolExpr, AnnError> {
        let mut items = vec![self.parse_and()?];
        loop {
            self.skip_ws();
            if self.eat_word("OR") || self.eat("||") {
                items.push(self.parse_and()?);
            } else {
                break;
            }
        }
        if items.len() == 1 {
            Ok(items.pop().expect("one item"))
        } else {
            Ok(BoolExpr::Or(items))
        }
    }

    fn parse_and(&mut self) -> Result<BoolExpr, AnnError> {
        let mut items = vec![self.parse_not()?];
        loop {
            self.skip_ws();
            if self.eat_word("AND") || self.eat("&&") {
                items.push(self.parse_not()?);
            } else {
                break;
            }
        }
        if items.len() == 1 {
            Ok(items.pop().expect("one item"))
        } else {
            Ok(BoolExpr::And(items))
        }
    }

    fn parse_not(&mut self) -> Result<BoolExpr, AnnError> {
        self.skip_ws();
        if self.eat_word("NOT") || self.eat("!") {
            return Ok(BoolExpr::Not(Box::new(self.parse_not()?)));
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Result<BoolExpr, AnnError> {
        self.skip_ws();
        if self.eat("(") {
            let expr = self.parse_or()?;
            self.skip_ws();
            if !self.eat(")") {
                return Err(AnnError::Parse("missing ')'".to_string()));
            }
            return Ok(expr);
        }
        if self.eat_word("TRUE") {
            return Ok(BoolExpr::Const(true));
        }
        if self.eat_word("FALSE") {
            return Ok(BoolExpr::Const(false));
        }
        let name = self.ident()?;
        Ok(BoolExpr::Var(parse_var_name(&name)?))
    }

    fn ident(&mut self) -> Result<String, AnnError> {
        self.skip_ws();
        let start = self.position;
        while self.position < self.input.len()
            && (self.input[self.position].is_ascii_alphanumeric()
                || self.input[self.position] == b'_')
        {
            self.position += 1;
        }
        if start == self.position {
            return Err(AnnError::Parse(format!(
                "expected a variable at byte {start}"
            )));
        }
        Ok(String::from_utf8_lossy(&self.input[start..self.position]).into_owned())
    }

    fn eat(&mut self, token: &str) -> bool {
        let bytes = token.as_bytes();
        if self.input[self.position..].starts_with(bytes) {
            self.position += bytes.len();
            true
        } else {
            false
        }
    }

    fn eat_word(&mut self, word: &str) -> bool {
        self.skip_ws();
        let bytes = word.as_bytes();
        if !self.input[self.position..].starts_with(bytes) {
            return false;
        }
        let end = self.position + bytes.len();
        if end < self.input.len()
            && (self.input[end].is_ascii_alphanumeric() || self.input[end] == b'_')
        {
            return false;
        }
        self.position = end;
        true
    }

    fn skip_ws(&mut self) {
        while self.position < self.input.len() && self.input[self.position].is_ascii_whitespace() {
            self.position += 1;
        }
    }
}

fn parse_var_name(name: &str) -> Result<usize, AnnError> {
    if name.len() == 1 && name.as_bytes()[0].is_ascii_alphabetic() {
        return Ok(usize::from(name.as_bytes()[0] - b'a'));
    }
    if let Some(rest) = name.strip_prefix('v') {
        return rest
            .parse::<usize>()
            .map_err(|_| AnnError::Parse(format!("invalid variable: {name}")));
    }
    name.parse::<usize>()
        .map_err(|_| AnnError::Parse(format!("invalid variable: {name}")))
}

/// Reduced ordered binary decision diagram used by [`count_models_bdd`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BddOp {
    And,
    Or,
}

#[derive(Clone, Copy, Debug)]
struct BddNode {
    var: usize,
    low: u32,
    high: u32,
}

#[derive(Default)]
struct Bdd {
    // Index 0 is FALSE, index 1 is TRUE; both are terminal.
    nodes: Vec<Option<BddNode>>,
    unique: HashMap<(usize, u32, u32), u32>,
    apply_cache: HashMap<(u8, u32, u32), u32>,
}

impl Bdd {
    fn new() -> Self {
        Self {
            nodes: vec![None, None],
            unique: HashMap::new(),
            apply_cache: HashMap::new(),
        }
    }

    fn mk(&mut self, var: usize, low: u32, high: u32) -> u32 {
        if low == high {
            return low;
        }
        let key = (var, low, high);
        if let Some(&node) = self.unique.get(&key) {
            return node;
        }
        let node = self.nodes.len() as u32;
        self.nodes.push(Some(BddNode { var, low, high }));
        self.unique.insert(key, node);
        node
    }

    fn var(&mut self, index: usize) -> u32 {
        self.mk(index, 0, 1)
    }

    fn apply(&mut self, op: BddOp, left: u32, right: u32) -> u32 {
        match op {
            BddOp::And => {
                if left == 0 || right == 0 {
                    return 0;
                }
                if left == 1 {
                    return right;
                }
                if right == 1 {
                    return left;
                }
            }
            BddOp::Or => {
                if left == 1 || right == 1 {
                    return 1;
                }
                if left == 0 {
                    return right;
                }
                if right == 0 {
                    return left;
                }
            }
        }

        let key = (op as u8, left, right);
        if let Some(&node) = self.apply_cache.get(&key) {
            return node;
        }

        let left_node = self.nodes[left as usize];
        let right_node = self.nodes[right as usize];
        let var = left_node
            .map_or(usize::MAX, |node| node.var)
            .min(right_node.map_or(usize::MAX, |node| node.var));
        let (left_low, left_high) = match left_node {
            Some(node) if node.var == var => (node.low, node.high),
            _ => (left, left),
        };
        let (right_low, right_high) = match right_node {
            Some(node) if node.var == var => (node.low, node.high),
            _ => (right, right),
        };

        let low = self.apply(op, left_low, right_low);
        let high = self.apply(op, left_high, right_high);
        let node = self.mk(var, low, high);
        self.apply_cache.insert(key, node);
        node
    }

    fn neg(&mut self, node: u32) -> u32 {
        match node {
            0 => 1,
            1 => 0,
            _ => {
                let current = self.nodes[node as usize].expect("non-terminal");
                let low = self.neg(current.low);
                let high = self.neg(current.high);
                self.mk(current.var, low, high)
            }
        }
    }

    fn build_expr(&mut self, expr: &BoolExpr) -> u32 {
        match expr {
            BoolExpr::Const(false) => 0,
            BoolExpr::Const(true) => 1,
            BoolExpr::Var(index) => self.var(*index),
            BoolExpr::Not(inner) => {
                let node = self.build_expr(inner);
                self.neg(node)
            }
            BoolExpr::And(items) => {
                let mut result = 1;
                for item in items {
                    let node = self.build_expr(item);
                    result = self.apply(BddOp::And, result, node);
                }
                result
            }
            BoolExpr::Or(items) => {
                let mut result = 0;
                for item in items {
                    let node = self.build_expr(item);
                    result = self.apply(BddOp::Or, result, node);
                }
                result
            }
        }
    }

    fn count(&self, node: u32, level: usize, vars: usize) -> u64 {
        match node {
            0 => 0,
            1 => 1u64 << (vars - level),
            _ => {
                let current = self.nodes[node as usize].expect("non-terminal");
                let factor = 1u64 << (current.var - level);
                factor
                    * (self.count(current.low, current.var + 1, vars)
                        + self.count(current.high, current.var + 1, vars))
            }
        }
    }
}

/// Counts satisfying assignments with a reduced ordered BDD.
///
/// This is a second exact backend for [`count_models`]; tests compare the two.
pub fn count_models_bdd(expr: &BoolExpr, vars: usize) -> Result<u64, AnnError> {
    if vars > MAX_MODEL_VARS {
        return Err(AnnError::TooManyVars {
            vars,
            max: MAX_MODEL_VARS,
        });
    }
    check_vars(expr, vars)?;
    let mut bdd = Bdd::new();
    let root = bdd.build_expr(expr);
    Ok(bdd.count(root, 0, vars))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(index: usize) -> BoolExpr {
        BoolExpr::Var(index)
    }

    #[test]
    fn belnap_truth_tables_are_lattice_operations() {
        assert_eq!(B4::True.meet(B4::Both), B4::Both);
        assert_eq!(B4::False.meet(B4::Both), B4::False);
        assert_eq!(B4::Both.meet(B4::Neither), B4::False);
        assert_eq!(B4::True.join(B4::Both), B4::True);
        assert_eq!(B4::False.join(B4::Both), B4::Both);
        assert_eq!(B4::Both.join(B4::Neither), B4::True);
        assert_eq!(B4::Both.neg(), B4::Both);
        assert_eq!(B4::Neither.neg(), B4::Neither);
    }

    #[test]
    fn tnorm_families_agree_on_corners_and_differ_inside() {
        for family in TNorm::ALL {
            // Crisp corners are shared by every family.
            assert_eq!(family.meet(1.0, 1.0), 1.0, "{}", family.name());
            assert_eq!(family.meet(1.0, 0.0), 0.0, "{}", family.name());
            assert_eq!(family.meet(0.0, 0.0), 0.0, "{}", family.name());
            assert_eq!(family.join(0.0, 0.0), 0.0, "{}", family.name());
            assert_eq!(family.join(1.0, 0.0), 1.0, "{}", family.name());
            assert_eq!(family.join(1.0, 1.0), 1.0, "{}", family.name());
        }

        // 0.5 ∧ 0.5 separates the three families.
        assert_eq!(TNorm::Godel.meet(0.5, 0.5), 0.5);
        assert_eq!(TNorm::Lukasiewicz.meet(0.5, 0.5), 0.0);
        assert_eq!(TNorm::Product.meet(0.5, 0.5), 0.25);
        // 0.5 ∨ 0.5 likewise.
        assert_eq!(TNorm::Godel.join(0.5, 0.5), 0.5);
        assert_eq!(TNorm::Lukasiewicz.join(0.5, 0.5), 1.0);
        assert_eq!(TNorm::Product.join(0.5, 0.5), 0.75);
    }

    #[test]
    fn tnorm_residuum_satisfies_adjunction() {
        for family in TNorm::ALL {
            for left in [0.0, 0.25, 0.5, 0.75, 1.0] {
                for right in [0.0, 0.25, 0.5, 0.75, 1.0] {
                    let implication = family.residuum(left, right);
                    // a → b is the largest z with a ∧ z ≤ b.
                    assert!(
                        family.meet(left, implication) <= right + 1e-12,
                        "{}: {} ∧ {} > {}",
                        family.name(),
                        left,
                        implication,
                        right
                    );
                    // And it is maximal: anything above it breaks the bound.
                    if implication < 1.0 {
                        assert!(
                            family.meet(left, (implication + 0.1).min(1.0)) > right + 1e-12,
                            "{}: residuum not maximal for {} → {}",
                            family.name(),
                            left,
                            right
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn tnorm_with_dispatches_only_for_lukasiewicz() {
        let product =
            Annotation::Lukasiewicz(0.5).meet_with(Annotation::Lukasiewicz(0.5), TNorm::Product);
        assert_eq!(product, Ok(Annotation::Lukasiewicz(0.25)));
        // Default meet stays Gödel.
        assert_eq!(
            Annotation::Lukasiewicz(0.5)
                .meet(Annotation::Lukasiewicz(0.5))
                .unwrap(),
            Annotation::Lukasiewicz(0.5)
        );
        // K3 keeps its truth table whatever the family.
        assert_eq!(
            Annotation::K3(TruthValue::Unknown)
                .meet_with(Annotation::K3(TruthValue::False), TNorm::Product)
                .unwrap(),
            Annotation::K3(TruthValue::False)
        );
        // Nat has no implication.
        assert_eq!(
            Annotation::Nat(2).implication_with(Annotation::Nat(3), TNorm::Godel),
            Err(AnnError::NegationUndefined("Nat"))
        );
        // K3 implication is material.
        assert_eq!(
            Annotation::K3(TruthValue::False)
                .implication_with(Annotation::K3(TruthValue::Unknown), TNorm::Godel)
                .unwrap(),
            Annotation::K3(TruthValue::True)
        );
    }

    #[test]
    fn annotation_operations_respect_domains() {
        let k3 = Annotation::K3(TruthValue::Unknown);
        assert_eq!(
            k3.meet(Annotation::K3(TruthValue::True)).unwrap(),
            Annotation::K3(TruthValue::Unknown)
        );
        assert_eq!(
            Annotation::Belnap(B4::Both)
                .join(Annotation::Belnap(B4::False))
                .unwrap(),
            Annotation::Belnap(B4::Both)
        );
        assert_eq!(
            Annotation::Nat(3).meet(Annotation::Nat(4)).unwrap(),
            Annotation::Nat(12)
        );
        assert_eq!(
            Annotation::Nat(3).join(Annotation::Nat(4)).unwrap(),
            Annotation::Nat(7)
        );
        assert_eq!(
            Annotation::lukasiewicz(0.25).unwrap().neg().unwrap(),
            Annotation::Lukasiewicz(0.75)
        );
        assert_eq!(k3.meet(Annotation::Nat(1)), Err(AnnError::DomainMismatch));
        assert_eq!(
            Annotation::Nat(1).neg(),
            Err(AnnError::NegationUndefined("Nat"))
        );
        assert_eq!(
            Annotation::lukasiewicz(1.5),
            Err(AnnError::LukasiewiczOutOfRange)
        );
    }

    #[test]
    fn count_models_enumerates_exactly() {
        // a AND (NOT b OR c)
        let expr = BoolExpr::And(vec![
            var(0),
            BoolExpr::Or(vec![BoolExpr::Not(Box::new(var(1))), var(2)]),
        ]);
        assert_eq!(count_models(&expr, 3).unwrap(), 3);

        // XOR over two variables has two models.
        let xor = BoolExpr::And(vec![
            BoolExpr::Or(vec![var(0), var(1)]),
            BoolExpr::Not(Box::new(BoolExpr::And(vec![var(0), var(1)]))),
        ]);
        assert_eq!(count_models(&xor, 2).unwrap(), 2);

        // A free variable doubles the model count.
        assert_eq!(count_models(&BoolExpr::Const(true), 3).unwrap(), 8);
        assert_eq!(
            count_models(&BoolExpr::Const(true), MAX_MODEL_VARS + 1),
            Err(AnnError::TooManyVars {
                vars: MAX_MODEL_VARS + 1,
                max: MAX_MODEL_VARS
            })
        );
    }

    #[test]
    fn dpll_counts_models_beyond_the_enumeration_limit() {
        let vars = 25;
        let all_true = BoolExpr::And((0..vars).map(BoolExpr::Var).collect());
        assert_eq!(count_models(&all_true, vars).unwrap(), 1);

        // a0 OR a1 leaves the other 23 variables free.
        let either = BoolExpr::Or(vec![BoolExpr::Var(0), BoolExpr::Var(1)]);
        // Satisfying assignments: 2^25 - 2^23 = 3 * 2^23.
        assert_eq!(count_models(&either, vars).unwrap(), 3 << 23);
    }

    #[test]
    fn certainty_maps_each_domain() {
        assert_eq!(
            Annotation::K3(TruthValue::True).certainty(None).unwrap(),
            Certainty::Certain
        );
        assert_eq!(
            Annotation::K3(TruthValue::False).certainty(None).unwrap(),
            Certainty::Impossible
        );
        assert_eq!(
            Annotation::K3(TruthValue::Unknown).certainty(None).unwrap(),
            Certainty::Contingent
        );
        assert_eq!(
            Annotation::Belnap(B4::True).certainty(None).unwrap(),
            Certainty::Certain
        );
        assert_eq!(
            Annotation::Belnap(B4::False).certainty(None).unwrap(),
            Certainty::Impossible
        );
        assert_eq!(
            Annotation::Belnap(B4::Both).certainty(None).unwrap(),
            Certainty::Contingent
        );
        assert_eq!(
            Annotation::Belnap(B4::Neither).certainty(None).unwrap(),
            Certainty::Contingent
        );
        // Ł: 0 is impossible, partial truth is contingent, >= threshold certain.
        assert_eq!(
            Annotation::Lukasiewicz(0.0).certainty(None).unwrap(),
            Certainty::Impossible
        );
        assert_eq!(
            Annotation::Lukasiewicz(0.2).certainty(None).unwrap(),
            Certainty::Contingent
        );
        assert_eq!(
            Annotation::Lukasiewicz(0.5).certainty(None).unwrap(),
            Certainty::Certain
        );
        assert_eq!(
            Annotation::Lukasiewicz(0.6).certainty(Some(0.7)).unwrap(),
            Certainty::Contingent
        );
        assert_eq!(
            Annotation::Lukasiewicz(0.8).certainty(Some(0.7)).unwrap(),
            Certainty::Certain
        );
        assert!(Annotation::Lukasiewicz(0.5).certainty(Some(1.5)).is_err());
        // ℕ: 0 is impossible, a positive count below the threshold contingent.
        assert_eq!(
            Annotation::Nat(0).certainty(None).unwrap(),
            Certainty::Impossible
        );
        assert_eq!(
            Annotation::Nat(1).certainty(None).unwrap(),
            Certainty::Certain
        );
        assert_eq!(
            Annotation::Nat(3).certainty(Some(5.0)).unwrap(),
            Certainty::Contingent
        );
        assert_eq!(
            Annotation::Nat(5).certainty(Some(5.0)).unwrap(),
            Certainty::Certain
        );
    }

    #[test]
    fn repair_answers_share_the_certainty_vocabulary() {
        assert_eq!(RepairAnswer::Certain.certainty(), Certainty::Certain);
        assert_eq!(RepairAnswer::Contingent.certainty(), Certainty::Contingent);
        assert_eq!(RepairAnswer::Impossible.certainty(), Certainty::Impossible);
        assert_eq!(Certainty::Certain.as_str(), "CERTAIN");
        assert_eq!(Certainty::Contingent.as_str(), "CONTINGENT");
        assert_eq!(Certainty::Impossible.as_str(), "IMPOSSIBLE");
        // Cross-check the two producers on the same conflict instance.
        let answer = under_repair(2, &[vec![0, 1]], &[0]).unwrap();
        assert_eq!(answer.certainty(), Certainty::Contingent);
        let answer = under_repair(2, &[vec![0, 1]], &[0, 1]).unwrap();
        assert_eq!(answer.certainty(), Certainty::Impossible);
        let answer = under_repair(2, &[], &[0]).unwrap();
        assert_eq!(answer.certainty(), Certainty::Certain);
    }

    #[test]
    fn repairs_answer_certain_possible_and_impossible() {
        // Two conflicting facts: one of them survives in each repair.
        assert_eq!(
            under_repair(2, &[vec![0, 1]], &[0]).unwrap(),
            RepairAnswer::Contingent
        );
        assert_eq!(
            under_repair(2, &[vec![0, 1]], &[0, 1]).unwrap(),
            RepairAnswer::Impossible
        );

        // No constraints: every fact is certain.
        assert_eq!(under_repair(2, &[], &[0]).unwrap(), RepairAnswer::Certain);

        // Chain conflict a-b, b-c: repairs are {a,c} and {b}.
        assert_eq!(
            under_repair(3, &[vec![0, 1], vec![1, 2]], &[0, 2]).unwrap(),
            RepairAnswer::Contingent
        );
        assert_eq!(
            under_repair(3, &[vec![0, 1], vec![1, 2]], &[1]).unwrap(),
            RepairAnswer::Contingent
        );
        assert_eq!(
            under_repair(3, &[vec![0, 1], vec![1, 2]], &[0, 1]).unwrap(),
            RepairAnswer::Impossible
        );
    }

    #[test]
    fn components_multiply_independent_conjunctions() {
        // (a OR b) AND (c OR d) over 4 vars: 3 * 3 = 9.
        let expr = BoolExpr::And(vec![
            BoolExpr::Or(vec![BoolExpr::Var(0), BoolExpr::Var(1)]),
            BoolExpr::Or(vec![BoolExpr::Var(2), BoolExpr::Var(3)]),
        ]);
        assert_eq!(count_models(&expr, 4).unwrap(), 9);
        // Two additional free variables multiply by 2^2.
        assert_eq!(count_models(&expr, 6).unwrap(), 9 * 4);
    }

    #[test]
    fn components_match_dpll_and_bdd_when_variables_are_shared() {
        let expr = BoolExpr::And(vec![
            BoolExpr::Or(vec![BoolExpr::Var(0), BoolExpr::Var(1)]),
            BoolExpr::Or(vec![BoolExpr::Var(0), BoolExpr::Var(2)]),
        ]);
        assert_eq!(count_models(&expr, 3), count_models_dpll(&expr, 3));
        assert_eq!(
            count_models(&expr, 3).unwrap(),
            count_models_bdd(&expr, 3).unwrap()
        );
    }

    #[test]
    fn bdd_matches_dpll_on_small_formulas() {
        let formulas = vec![
            BoolExpr::Const(true),
            BoolExpr::Const(false),
            BoolExpr::Var(0),
            BoolExpr::Not(Box::new(BoolExpr::Var(0))),
            BoolExpr::And(vec![BoolExpr::Var(0), BoolExpr::Var(1)]),
            BoolExpr::Or(vec![BoolExpr::Var(0), BoolExpr::Var(1)]),
            BoolExpr::And(vec![
                BoolExpr::Var(0),
                BoolExpr::Or(vec![
                    BoolExpr::Not(Box::new(BoolExpr::Var(1))),
                    BoolExpr::Var(2),
                ]),
            ]),
        ];
        for formula in formulas {
            for vars in 0..=6 {
                assert_eq!(
                    count_models_bdd(&formula, vars),
                    count_models(&formula, vars),
                    "vars={vars} formula={formula:?}"
                );
            }
        }
    }

    #[test]
    fn bdd_counts_beyond_enumeration_scale() {
        let vars = 25;
        let either = BoolExpr::Or(vec![BoolExpr::Var(0), BoolExpr::Var(1)]);
        assert_eq!(count_models_bdd(&either, vars).unwrap(), 3 << 23);
        assert_eq!(
            count_models_bdd(&BoolExpr::Const(true), MAX_MODEL_VARS).unwrap(),
            1u64 << MAX_MODEL_VARS
        );
    }

    #[test]
    fn repairs_decompose_into_connected_components() {
        // 20 facts, two independent conflicts. Only the first component is
        // contested; the second one is irrelevant to these queries.
        let denials = vec![vec![0, 1], vec![10, 11]];
        assert_eq!(
            under_repair(20, &denials, &[0]).unwrap(),
            RepairAnswer::Contingent
        );
        assert_eq!(
            under_repair(20, &denials, &[2]).unwrap(),
            RepairAnswer::Certain
        );
        assert_eq!(
            under_repair(20, &denials, &[0, 1]).unwrap(),
            RepairAnswer::Impossible
        );
    }

    /// Straight enumeration of every subset, kept as the oracle.
    #[cfg(test)]
    fn repair_answer_bruteforce(
        facts: usize,
        denials: &[Vec<usize>],
        query: &[usize],
    ) -> RepairAnswer {
        let mut certain = true;
        let mut possible = false;
        for mask in 0..(1u64 << facts) {
            if !consistent(mask, denials) || !maximal(mask, facts, denials) {
                continue;
            }
            let holds = query.iter().all(|index| mask & (1 << index) != 0);
            if holds {
                possible = true;
            } else {
                certain = false;
            }
        }
        if certain {
            RepairAnswer::Certain
        } else if possible {
            RepairAnswer::Contingent
        } else {
            RepairAnswer::Impossible
        }
    }

    #[test]
    fn unit_propagation_matches_bruteforce() {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..300 {
            let facts = (next() % 6) as usize + 1;
            let denial_count = (next() % 4) as usize;
            let mut denials = Vec::new();
            for _ in 0..denial_count {
                let size = (next() % 3) as usize + 1;
                let mut denial: Vec<usize> = (0..size).map(|_| (next() as usize) % facts).collect();
                denial.sort_unstable();
                denial.dedup();
                denials.push(denial);
            }
            let query: Vec<usize> = (0..facts).filter(|_| next() % 2 == 0).collect();
            let expected = repair_answer_bruteforce(facts, &denials, &query);
            assert_eq!(
                component_answer(facts, &denials, &query).unwrap(),
                expected,
                "facts={facts} denials={denials:?} query={query:?}"
            );
        }
    }

    #[test]
    fn unit_propagation_handles_oversized_components() {
        // Eight facts are pinned out by one-fact denials; the remaining
        // sixteen still fit the enumeration budget.
        let mut denials: Vec<Vec<usize>> = (0..8).map(|index| vec![index]).collect();
        let base = 8;
        for offset in 0..15 {
            denials.push(vec![base + offset, base + offset + 1]);
        }
        let answer = under_repair(24, &denials, &[9]).unwrap();
        assert_eq!(answer, RepairAnswer::Contingent);
        // The brute-force oracle cannot run at this size; the pinned facts
        // must make the query impossible instead.
        assert_eq!(
            under_repair(24, &denials, &[0]).unwrap(),
            RepairAnswer::Impossible
        );
    }

    #[test]
    fn empty_denial_leaves_no_repair() {
        assert_eq!(
            under_repair(2, &[vec![]], &[0]).unwrap(),
            RepairAnswer::Impossible
        );
    }

    #[test]
    fn repairs_reject_an_oversized_component() {
        let facts = MAX_COMPONENT_FACTS + 1;
        let denials: Vec<Vec<usize>> = (0..facts - 1).map(|i| vec![i, i + 1]).collect();
        assert_eq!(
            under_repair(facts, &denials, &[0]),
            Err(AnnError::TooManyFacts {
                facts,
                max: MAX_COMPONENT_FACTS
            })
        );
    }

    #[test]
    fn repairs_reject_out_of_range_inputs() {
        assert_eq!(
            under_repair(2, &[vec![0, 5]], &[0]),
            Err(AnnError::OutOfRange { index: 5, count: 2 })
        );
        assert_eq!(
            under_repair(MAX_REPAIR_FACTS + 1, &[], &[]),
            Err(AnnError::TooManyFacts {
                facts: MAX_REPAIR_FACTS + 1,
                max: MAX_REPAIR_FACTS
            })
        );
    }
}
