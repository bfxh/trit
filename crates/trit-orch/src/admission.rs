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

/// 第五门：认领。调用方用 [`crate::ClaimStore::gate`] 得到 `claim`，再与四门一起做 K3 AND。
pub fn admit_with_claim(
    dependencies: Ternary,
    authorization: Ternary,
    resources: Ternary,
    freshness: Ternary,
    claim: Ternary,
) -> Ternary {
    admit(dependencies, authorization, resources, freshness) & claim
}
