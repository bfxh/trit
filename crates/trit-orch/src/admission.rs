use crate::Ternary;

pub fn dependency_gate(required: Ternary, evidence: Ternary) -> Ternary {
    match required {
        Ternary::False => Ternary::True,
        Ternary::Unknown => Ternary::Unknown,
        Ternary::True => evidence,
    }
}

pub fn admit(
    dependencies: Ternary,
    authorization: Ternary,
    resources: Ternary,
    freshness: Ternary,
) -> Ternary {
    dependencies & authorization & resources & freshness
}
