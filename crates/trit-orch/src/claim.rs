//! 路径认领/租约：单写者、fencing、TTL、冲突可见。
//!
//! - 时间由调用方传入（`now`），模块不读系统时钟，测试可复现。
//! - 过期 **不等于** 自动释放：`gate` 对过期/未认领返回 `Unknown`，必须重新 claim。
//! - `fencing` 单调：旧执行器的 renew/release 一律拒绝。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use trit_core::value::Ternary;

use crate::Error;

/// 单次认领允许的最大路径数，防止无界输入。
pub const MAX_CLAIM_PATHS: usize = 256;
const MAX_PATH_LEN: usize = 512;
const MAX_OWNER_LEN: usize = 128;
const MAX_INTENT_LEN: usize = 512;

/// 一条路径认领。`paths` 是仓库相对路径；目录前缀覆盖其子路径。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub id: String,
    pub owner: String,
    pub paths: Vec<String>,
    pub intent: String,
    pub fencing: u64,
    pub acquired_at: u64,
    pub expires_at: u64,
}

/// 单个认领的派生状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClaimState {
    Free,
    Held,
    Contested,
    Expired,
}

/// 一次 claim 的结果：要么拿到，要么看到竞争者。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClaimOutcome {
    Acquired(Claim),
    Contested {
        contenders: Vec<String>,
        paths: Vec<String>,
    },
}

/// 选批时的认领上下文：谁在什么时间执行，每个任务要覆盖哪些路径。
///
/// 任务未在 `paths` 中出现时按“未声明/未认领”处理为 `Unknown`，而不是默认放行。
#[derive(Debug)]
pub struct ClaimSet<'a> {
    pub store: &'a ClaimStore,
    pub owner: String,
    pub now: u64,
    pub paths: BTreeMap<String, Vec<String>>,
}

impl<'a> ClaimSet<'a> {
    pub fn new(store: &'a ClaimStore, owner: impl Into<String>, now: u64) -> Self {
        Self {
            store,
            owner: owner.into(),
            now,
            paths: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, task: impl Into<String>, paths: Vec<String>) {
        self.paths.insert(task.into(), paths);
    }

    pub fn with_task(mut self, task: impl Into<String>, paths: Vec<String>) -> Self {
        self.insert(task, paths);
        self
    }

    pub fn paths_for(&self, task: &str) -> Option<&[String]> {
        self.paths.get(task).map(Vec::as_slice)
    }
}

/// 内存认领表。宿主可序列化持久化；恢复用 [`ClaimStore::from_claims`]。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimStore {
    claims: BTreeMap<String, Claim>,
    next_fencing: u64,
}

impl ClaimStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从已持久化的认领恢复。允许重叠（恢复数据可能已经是 Contested），
    /// 但拒绝空 id/owner、非法路径、重复 id、时间区间倒置。
    pub fn from_claims(claims: Vec<Claim>) -> Result<Self, Error> {
        let mut store = Self::default();
        for claim in claims {
            let id = claim.id.trim().to_owned();
            if id.is_empty() {
                return Err(Error::ClaimIdBlank);
            }
            let owner = normalize_owner(&claim.owner)?;
            let paths = normalize_paths(&claim.paths)?;
            if claim.expires_at <= claim.acquired_at {
                return Err(Error::ClaimTimeRange { id });
            }
            let normalized = Claim {
                id: id.clone(),
                owner,
                paths,
                intent: claim.intent.trim().to_owned(),
                fencing: claim.fencing,
                acquired_at: claim.acquired_at,
                expires_at: claim.expires_at,
            };
            store.next_fencing = store.next_fencing.max(normalized.fencing);
            if store.claims.insert(id.clone(), normalized).is_some() {
                return Err(Error::ClaimDuplicateId { id });
            }
        }
        Ok(store)
    }

    /// 认领一组路径。与别的 owner 的有效认领重叠 ⇒ `Contested`，不写入。
    /// 与自己的有效认领重叠 ⇒ 合并成一条，`fencing` 递增。
    pub fn claim(
        &mut self,
        owner: &str,
        paths: &[String],
        intent: &str,
        now: u64,
        ttl: u64,
    ) -> Result<ClaimOutcome, Error> {
        let owner = normalize_owner(owner)?;
        let paths = normalize_paths(paths)?;
        let intent = normalize_intent(intent)?;
        if ttl == 0 {
            return Err(Error::ClaimZeroTtl);
        }
        let expires_at = now.checked_add(ttl).ok_or(Error::ClaimTimeOverflow)?;

        let mut contenders: Vec<String> = self
            .claims
            .values()
            .filter(|claim| claim.expires_at > now && claim.owner != owner)
            .filter(|claim| overlaps(&claim.paths, &paths))
            .map(|claim| claim.id.clone())
            .collect();
        if !contenders.is_empty() {
            contenders.sort();
            return Ok(ClaimOutcome::Contested { contenders, paths });
        }

        let mut merge: Vec<String> = self
            .claims
            .values()
            .filter(|claim| claim.expires_at > now && claim.owner == owner)
            .filter(|claim| overlaps(&claim.paths, &paths))
            .map(|claim| claim.id.clone())
            .collect();
        merge.sort();

        let fencing = self.bump_fencing()?;
        if let Some(first) = merge.first().cloned() {
            let mut merged_paths: Vec<String> = Vec::new();
            let mut acquired_at = now;
            for id in &merge {
                if let Some(claim) = self.claims.remove(id) {
                    merged_paths.extend(claim.paths);
                    acquired_at = acquired_at.min(claim.acquired_at);
                }
            }
            merged_paths.extend(paths);
            merged_paths.sort();
            merged_paths.dedup();
            let claim = Claim {
                id: first.clone(),
                owner,
                paths: merged_paths,
                intent,
                fencing,
                acquired_at,
                expires_at,
            };
            self.claims.insert(first, claim.clone());
            return Ok(ClaimOutcome::Acquired(claim));
        }

        let id = format!("claim-{fencing}");
        let claim = Claim {
            id: id.clone(),
            owner,
            paths,
            intent,
            fencing,
            acquired_at: now,
            expires_at,
        };
        self.claims.insert(id, claim.clone());
        Ok(ClaimOutcome::Acquired(claim))
    }

    /// 续约。必须 owner 匹配、fence 当前、未过期。
    pub fn renew(
        &mut self,
        id: &str,
        owner: &str,
        fencing: u64,
        now: u64,
        ttl: u64,
    ) -> Result<Claim, Error> {
        let owner = normalize_owner(owner)?;
        if ttl == 0 {
            return Err(Error::ClaimZeroTtl);
        }
        let expires_at = now.checked_add(ttl).ok_or(Error::ClaimTimeOverflow)?;
        {
            let claim = self
                .claims
                .get(id)
                .ok_or_else(|| Error::ClaimNotFound { id: id.to_owned() })?;
            if claim.owner != owner {
                return Err(Error::ClaimNotOwner {
                    id: id.to_owned(),
                    owner,
                });
            }
            if claim.expires_at <= now {
                return Err(Error::ClaimExpired { id: id.to_owned() });
            }
            if claim.fencing != fencing {
                return Err(Error::StaleFence {
                    operation: id.to_owned(),
                    given: fencing,
                    current: claim.fencing,
                });
            }
        }
        let next = self.bump_fencing()?;
        let claim = self
            .claims
            .get_mut(id)
            .ok_or_else(|| Error::ClaimNotFound { id: id.to_owned() })?;
        claim.fencing = next;
        claim.expires_at = expires_at;
        Ok(claim.clone())
    }

    /// 释放。必须 owner 匹配、fence 当前；过期也允许清理。
    pub fn release(&mut self, id: &str, owner: &str, fencing: u64) -> Result<Claim, Error> {
        let owner = normalize_owner(owner)?;
        let claim = self
            .claims
            .get(id)
            .ok_or_else(|| Error::ClaimNotFound { id: id.to_owned() })?;
        if claim.owner != owner {
            return Err(Error::ClaimNotOwner {
                id: id.to_owned(),
                owner,
            });
        }
        if claim.fencing != fencing {
            return Err(Error::StaleFence {
                operation: id.to_owned(),
                given: fencing,
                current: claim.fencing,
            });
        }
        self.claims
            .remove(id)
            .ok_or_else(|| Error::ClaimNotFound { id: id.to_owned() })
    }

    /// 单个认领的派生状态。
    pub fn state_of(&self, id: &str, now: u64) -> ClaimState {
        match self.claims.get(id) {
            None => ClaimState::Free,
            Some(claim) if claim.expires_at <= now => ClaimState::Expired,
            Some(_) => ClaimState::Held,
        }
    }

    /// 一组路径的派生状态：无认领=Free；一个 owner=Held；多个 owner=Contested；
    /// 只有过期认领=Expired。
    pub fn state_of_paths(&self, paths: &[String], now: u64) -> Result<ClaimState, Error> {
        let paths = normalize_paths(paths)?;
        let mut owners: Vec<&str> = Vec::new();
        let mut expired = false;
        for claim in self.claims.values() {
            if !overlaps(&claim.paths, &paths) {
                continue;
            }
            if claim.expires_at <= now {
                expired = true;
            } else if !owners.contains(&claim.owner.as_str()) {
                owners.push(claim.owner.as_str());
            }
        }
        Ok(match owners.len() {
            0 if expired => ClaimState::Expired,
            0 => ClaimState::Free,
            1 => ClaimState::Held,
            _ => ClaimState::Contested,
        })
    }

    /// 认领门：True=这些路径都由 `owner` 的有效认领覆盖；
    /// False=被别的 owner 占着；Unknown=未认领/已过期/只覆盖了一部分。
    ///
    /// 这是 `admit_with_claim` 的第五门。False 支配 Unknown。
    pub fn gate(&self, paths: &[String], owner: &str, now: u64) -> Result<Ternary, Error> {
        let owner = normalize_owner(owner)?;
        let paths = normalize_paths(paths)?;

        for path in &paths {
            for claim in self.claims.values() {
                if claim.expires_at > now
                    && claim.owner != owner
                    && claim.paths.iter().any(|scope| paths_overlap(scope, path))
                {
                    return Ok(Ternary::False);
                }
            }
        }

        for path in &paths {
            let covered = self.claims.values().any(|claim| {
                claim.expires_at > now
                    && claim.owner == owner
                    && claim.paths.iter().any(|scope| covers(scope, path))
            });
            if !covered {
                return Ok(Ternary::Unknown);
            }
        }
        Ok(Ternary::True)
    }

    pub fn get(&self, id: &str) -> Option<&Claim> {
        self.claims.get(id)
    }

    pub fn claims(&self) -> impl Iterator<Item = &Claim> {
        self.claims.values()
    }

    pub fn len(&self) -> usize {
        self.claims.len()
    }

    pub fn is_empty(&self) -> bool {
        self.claims.is_empty()
    }

    fn bump_fencing(&mut self) -> Result<u64, Error> {
        self.next_fencing = self
            .next_fencing
            .checked_add(1)
            .ok_or(Error::ClaimFenceOverflow)?;
        Ok(self.next_fencing)
    }
}

/// 两条路径是否重叠：互为前缀（按 `/` 边界）或相等。
pub fn paths_overlap(left: &str, right: &str) -> bool {
    covers(left, right) || covers(right, left)
}

fn covers(scope: &str, path: &str) -> bool {
    scope == path
        || (path.len() > scope.len()
            && path.starts_with(scope)
            && path.as_bytes().get(scope.len()) == Some(&b'/'))
}

fn overlaps(scopes: &[String], paths: &[String]) -> bool {
    scopes
        .iter()
        .any(|scope| paths.iter().any(|path| paths_overlap(scope, path)))
}

fn normalize_owner(raw: &str) -> Result<String, Error> {
    let owner = raw.trim();
    if owner.is_empty() {
        return Err(Error::ClaimBlankOwner);
    }
    if owner.len() > MAX_OWNER_LEN {
        return Err(Error::ClaimOwnerTooLong {
            owner: owner.to_owned(),
        });
    }
    Ok(owner.to_owned())
}

fn normalize_intent(raw: &str) -> Result<String, Error> {
    let intent = raw.trim();
    if intent.is_empty() {
        return Err(Error::ClaimIntentBlank);
    }
    if intent.len() > MAX_INTENT_LEN {
        return Err(Error::ClaimIntentTooLong);
    }
    Ok(intent.to_owned())
}

fn normalize_paths(paths: &[String]) -> Result<Vec<String>, Error> {
    if paths.is_empty() {
        return Err(Error::ClaimBlankPath {
            path: String::new(),
        });
    }
    if paths.len() > MAX_CLAIM_PATHS {
        return Err(Error::ClaimTooManyPaths {
            count: paths.len(),
            max: MAX_CLAIM_PATHS,
        });
    }
    let mut normalized = Vec::with_capacity(paths.len());
    for raw in paths {
        let path = normalize_path(raw)?;
        if !normalized.contains(&path) {
            normalized.push(path);
        }
    }
    normalized.sort();
    Ok(normalized)
}

fn normalize_path(raw: &str) -> Result<String, Error> {
    let path = raw.trim().replace('\\', "/");
    if path.is_empty() {
        return Err(Error::ClaimBlankPath { path });
    }
    if path.len() > MAX_PATH_LEN {
        return Err(Error::ClaimPathTooLong { path });
    }
    if path.starts_with('/') || path.contains(':') {
        return Err(Error::ClaimPathEscape { path });
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." | "*" => return Err(Error::ClaimPathEscape { path }),
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(Error::ClaimBlankPath { path });
    }
    Ok(parts.join("/"))
}
