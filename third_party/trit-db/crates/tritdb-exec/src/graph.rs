//! Annotated graphs: CSR adjacency, three-valued reachability, and annotated
//! k-nearest-neighbour queries.
//!
//! This is the in-memory half of M5. Callers build a graph from an edge list
//! and query it directly; persisting the CSR arrays into the page store is a
//! separate step that this module deliberately does not take.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

use crate::ann::RepairAnswer;
use crate::TruthValue;

/// Maximum nodes accepted by [`Csr::from_edges`].
pub const MAX_NODES: usize = 4096;
/// Maximum edges accepted by [`Csr::from_edges`].
pub const MAX_EDGES: usize = 16_384;
/// Maximum vector dimensions accepted by [`annotated_knn`].
pub const MAX_DIMENSIONS: usize = 64;
/// Magic header of the persisted graph encoding.
pub const TRGR_MAGIC: &[u8; 4] = b"TRGR";
/// Version of the persisted graph encoding.
pub const TRGR_VERSION: u8 = 1;
/// Magic header of the persisted point set encoding.
pub const TRPN_MAGIC: &[u8; 4] = b"TRPN";
/// Version of the persisted point set encoding.
pub const TRPN_VERSION: u8 = 1;
/// Maximum points accepted by [`encode_points`] / [`decode_points`].
pub const MAX_POINTS: usize = 4096;

/// Structured graph errors.
#[derive(Debug)]
pub enum GraphError {
    /// Too many nodes for an in-memory CSR.
    TooManyNodes {
        /// Requested nodes.
        nodes: usize,
        /// Maximum supported.
        max: usize,
    },
    /// Too many edges after deduplication.
    TooManyEdges {
        /// Deduplicated edge count.
        edges: usize,
        /// Maximum supported.
        max: usize,
    },
    /// An edge endpoint is outside the declared node range.
    EdgeOutOfRange {
        /// Edge source.
        from: u32,
        /// Edge target.
        to: u32,
        /// Declared node count.
        nodes: usize,
    },
    /// A query node is outside the graph.
    NodeOutOfRange {
        /// Requested node.
        node: u32,
        /// Graph node count.
        nodes: usize,
    },
    /// A vector had no dimensions.
    EmptyVector,
    /// A vector had too many dimensions.
    TooManyDimensions {
        /// Requested dimensions.
        dimensions: usize,
        /// Maximum supported.
        max: usize,
    },
    /// A point and the query vector had different lengths.
    DimensionMismatch {
        /// Query length.
        expected: usize,
        /// Point length.
        actual: usize,
    },
    /// A squared distance did not fit in `i128`.
    DistanceOverflow,
    /// A stored `TRGR` graph is malformed.
    BadEncoding(&'static str),
    /// Too many points for a persisted point set.
    TooManyPoints {
        /// Requested points.
        points: usize,
        /// Maximum supported.
        max: usize,
    },
}

impl fmt::Display for GraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::TooManyNodes { nodes, max } => {
                write!(formatter, "too many graph nodes: {nodes} > {max}")
            }
            GraphError::TooManyEdges { edges, max } => {
                write!(formatter, "too many graph edges: {edges} > {max}")
            }
            GraphError::EdgeOutOfRange { from, to, nodes } => write!(
                formatter,
                "edge {from}->{to} is outside 0..{nodes}; pass a larger --nodes"
            ),
            GraphError::NodeOutOfRange { node, nodes } => {
                write!(formatter, "node {node} is outside 0..{nodes}")
            }
            GraphError::EmptyVector => write!(formatter, "vector must have at least one dimension"),
            GraphError::TooManyDimensions { dimensions, max } => {
                write!(
                    formatter,
                    "too many vector dimensions: {dimensions} > {max}"
                )
            }
            GraphError::DimensionMismatch { expected, actual } => {
                write!(
                    formatter,
                    "point has {actual} dimensions, query has {expected}"
                )
            }
            GraphError::DistanceOverflow => {
                write!(formatter, "squared distance does not fit in i128")
            }
            GraphError::BadEncoding(reason) => write!(formatter, "bad encoded data: {reason}"),
            GraphError::TooManyPoints { points, max } => {
                write!(formatter, "too many points: {points} > {max}")
            }
        }
    }
}

impl Error for GraphError {}

/// Compressed sparse row adjacency.
///
/// Every row is sorted and deduplicated, so [`Self::neighbors`] is stable and
/// [`Csr::has_edge`] can binary-search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Csr {
    offsets: Vec<usize>,
    targets: Vec<u32>,
}

impl Csr {
    /// Builds a CSR from a directed edge list.
    ///
    /// Duplicate edges are collapsed; `nodes` must cover every endpoint.
    pub fn from_edges(nodes: usize, edges: &[(u32, u32)]) -> Result<Self, GraphError> {
        if nodes > MAX_NODES {
            return Err(GraphError::TooManyNodes {
                nodes,
                max: MAX_NODES,
            });
        }
        let mut rows: Vec<BTreeSet<u32>> = vec![BTreeSet::new(); nodes];
        for &(from, to) in edges {
            if from as usize >= nodes || to as usize >= nodes {
                return Err(GraphError::EdgeOutOfRange { from, to, nodes });
            }
            rows[from as usize].insert(to);
        }
        let edge_count: usize = rows.iter().map(BTreeSet::len).sum();
        if edge_count > MAX_EDGES {
            return Err(GraphError::TooManyEdges {
                edges: edge_count,
                max: MAX_EDGES,
            });
        }

        let mut offsets = Vec::with_capacity(nodes + 1);
        let mut targets = Vec::with_capacity(edge_count);
        offsets.push(0);
        for row in &rows {
            targets.extend(row.iter().copied());
            offsets.push(targets.len());
        }
        Ok(Self { offsets, targets })
    }

    /// Number of nodes, including nodes with no outgoing edges.
    pub fn node_count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// Number of deduplicated directed edges.
    pub fn edge_count(&self) -> usize {
        self.targets.len()
    }

    /// Outgoing neighbours of `node`, in ascending order.
    pub fn neighbors(&self, node: u32) -> Result<&[u32], GraphError> {
        self.check_node(node)?;
        Ok(self.row(node as usize))
    }

    /// Returns whether `from -> to` exists.
    pub fn has_edge(&self, from: u32, to: u32) -> Result<bool, GraphError> {
        self.check_node(from)?;
        self.check_node(to)?;
        Ok(self.row(from as usize).binary_search(&to).is_ok())
    }

    fn check_node(&self, node: u32) -> Result<(), GraphError> {
        if node as usize >= self.node_count() {
            return Err(GraphError::NodeOutOfRange {
                node,
                nodes: self.node_count(),
            });
        }
        Ok(())
    }

    fn row(&self, node: usize) -> &[u32] {
        &self.targets[self.offsets[node]..self.offsets[node + 1]]
    }
}

/// Result of a three-valued reachability query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reach {
    /// `Certain` (all-true path), `Contingent` (only unknown edges) or
    /// `Impossible`.
    pub answer: RepairAnswer,
    /// Edge count of the witness path; `None` when impossible.
    pub hops: Option<usize>,
    /// Witness path, shortest first; empty when impossible.
    pub path: Vec<u32>,
}

impl Reach {
    fn certain(path: Vec<u32>) -> Self {
        Self {
            answer: RepairAnswer::Certain,
            hops: Some(path.len() - 1),
            path,
        }
    }

    fn contingent(path: Vec<u32>) -> Self {
        Self {
            answer: RepairAnswer::Contingent,
            hops: Some(path.len() - 1),
            path,
        }
    }

    fn impossible() -> Self {
        Self {
            answer: RepairAnswer::Impossible,
            hops: None,
            path: Vec::new(),
        }
    }
}

/// CSR adjacency plus one [`TruthValue`] label per edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnotatedGraph {
    csr: Csr,
    labels: Vec<TruthValue>,
}

impl AnnotatedGraph {
    /// Builds an annotated graph; a repeated edge keeps its last label.
    pub fn from_edges(nodes: usize, edges: &[(u32, u32, TruthValue)]) -> Result<Self, GraphError> {
        let pairs: Vec<(u32, u32)> = edges.iter().map(|&(from, to, _)| (from, to)).collect();
        let csr = Csr::from_edges(nodes, &pairs)?;
        let mut by_pair: BTreeMap<(u32, u32), TruthValue> = BTreeMap::new();
        for &(from, to, label) in edges {
            by_pair.insert((from, to), label);
        }
        let mut labels = Vec::with_capacity(csr.edge_count());
        for from in 0..csr.node_count() {
            for &to in csr.row(from) {
                let label = by_pair
                    .get(&(from as u32, to))
                    .copied()
                    .unwrap_or(TruthValue::Unknown);
                labels.push(label);
            }
        }
        Ok(Self { csr, labels })
    }

    /// Underlying CSR adjacency.
    pub fn graph(&self) -> &Csr {
        &self.csr
    }

    /// Number of nodes.
    pub fn node_count(&self) -> usize {
        self.csr.node_count()
    }

    /// Number of deduplicated edges.
    pub fn edge_count(&self) -> usize {
        self.csr.edge_count()
    }

    /// Label of `from -> to`, or `None` when the edge is absent.
    pub fn edge_label(&self, from: u32, to: u32) -> Result<Option<TruthValue>, GraphError> {
        self.csr.check_node(from)?;
        self.csr.check_node(to)?;
        let row = self.csr.row(from as usize);
        Ok(row
            .binary_search(&to)
            .ok()
            .map(|index| self.labels[self.csr.offsets[from as usize] + index]))
    }

    /// Three-valued reachability from `from` to `to` within `max_hops` edges.
    ///
    /// A path only made of `TRUE` edges yields [`RepairAnswer::Certain`]; if no
    /// such path exists but one using `TRUE`/`UNKNOWN` edges does, the answer is
    /// [`RepairAnswer::Contingent`]; otherwise it is
    /// [`RepairAnswer::Impossible`]. `FALSE` edges are never traversable.
    pub fn reach(&self, from: u32, to: u32, max_hops: usize) -> Result<Reach, GraphError> {
        self.csr.check_node(from)?;
        self.csr.check_node(to)?;
        if let Some(path) = self.bfs_path(from, to, max_hops, false) {
            return Ok(Reach::certain(path));
        }
        if let Some(path) = self.bfs_path(from, to, max_hops, true) {
            return Ok(Reach::contingent(path));
        }
        Ok(Reach::impossible())
    }

    /// Nodes reachable from `start` within `max_hops`, breadth-first.
    ///
    /// Traverses `TRUE` and `UNKNOWN` edges; each node appears once with its
    /// shortest hop count.
    pub fn reachable_within(
        &self,
        start: u32,
        max_hops: usize,
    ) -> Result<Vec<(u32, usize)>, GraphError> {
        self.csr.check_node(start)?;
        let nodes = self.csr.node_count();
        let mut depth = vec![usize::MAX; nodes];
        let mut queue = VecDeque::new();
        let mut reached = Vec::new();
        depth[start as usize] = 0;
        queue.push_back(start);
        while let Some(node) = queue.pop_front() {
            let hops = depth[node as usize];
            if hops >= max_hops {
                continue;
            }
            let base = self.csr.offsets[node as usize];
            for (offset, &next) in self.csr.row(node as usize).iter().enumerate() {
                if self.labels[base + offset] == TruthValue::False
                    || depth[next as usize] != usize::MAX
                {
                    continue;
                }
                depth[next as usize] = hops + 1;
                reached.push((next, hops + 1));
                queue.push_back(next);
            }
        }
        Ok(reached)
    }

    /// Encodes the graph as `TRGR`.
    ///
    /// Layout: magic `TRGR`, version byte, node count and edge count (`u32`
    /// little-endian), then `node_count + 1` offsets (`u32`), `edge_count`
    /// targets (`u32`), and one label byte per edge (`0` = `FALSE`, `1` =
    /// `UNKNOWN`, `2` = `TRUE`).
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes =
            Vec::with_capacity(13 + 4 * (self.node_count() + 1) + 5 * self.edge_count());
        bytes.extend_from_slice(TRGR_MAGIC);
        bytes.push(TRGR_VERSION);
        bytes.extend_from_slice(&(self.node_count() as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.edge_count() as u32).to_le_bytes());
        for offset in &self.csr.offsets {
            bytes.extend_from_slice(&(*offset as u32).to_le_bytes());
        }
        for target in &self.csr.targets {
            bytes.extend_from_slice(&target.to_le_bytes());
        }
        for label in &self.labels {
            bytes.push(label_code(*label));
        }
        bytes
    }

    /// Decodes a `TRGR` graph produced by [`Self::encode`].
    pub fn decode(bytes: &[u8]) -> Result<Self, GraphError> {
        if bytes.len() < 13 || &bytes[0..4] != TRGR_MAGIC {
            return Err(GraphError::BadEncoding("bad TRGR header"));
        }
        if bytes[4] != TRGR_VERSION {
            return Err(GraphError::BadEncoding("unsupported TRGR version"));
        }
        let node_count = read_u32(bytes, 5) as usize;
        let edge_count = read_u32(bytes, 9) as usize;
        if node_count > MAX_NODES {
            return Err(GraphError::TooManyNodes {
                nodes: node_count,
                max: MAX_NODES,
            });
        }
        if edge_count > MAX_EDGES {
            return Err(GraphError::TooManyEdges {
                edges: edge_count,
                max: MAX_EDGES,
            });
        }

        let offsets_size = (node_count + 1)
            .checked_mul(4)
            .ok_or(GraphError::BadEncoding("TRGR size overflows"))?;
        let targets_size = edge_count
            .checked_mul(4)
            .ok_or(GraphError::BadEncoding("TRGR size overflows"))?;
        let expected = 13usize
            .checked_add(offsets_size)
            .and_then(|size| size.checked_add(targets_size))
            .and_then(|size| size.checked_add(edge_count))
            .ok_or(GraphError::BadEncoding("TRGR size overflows"))?;
        if bytes.len() != expected {
            return Err(GraphError::BadEncoding("TRGR length mismatch"));
        }

        let mut cursor = 13;
        let mut offsets = Vec::with_capacity(node_count + 1);
        for _ in 0..=node_count {
            offsets.push(read_u32(bytes, cursor) as usize);
            cursor += 4;
        }
        let mut targets = Vec::with_capacity(edge_count);
        for _ in 0..edge_count {
            targets.push(read_u32(bytes, cursor));
            cursor += 4;
        }
        let mut labels = Vec::with_capacity(edge_count);
        for _ in 0..edge_count {
            labels.push(
                label_from_code(bytes[cursor])
                    .ok_or(GraphError::BadEncoding("bad TRGR edge label"))?,
            );
            cursor += 1;
        }

        if offsets[0] != 0 || offsets[node_count] != edge_count {
            return Err(GraphError::BadEncoding(
                "TRGR offsets do not cover the edges",
            ));
        }
        for window in offsets.windows(2) {
            if window[0] > window[1] {
                return Err(GraphError::BadEncoding("TRGR offsets decrease"));
            }
        }
        for window in offsets.windows(2) {
            let mut previous = None;
            for &target in &targets[window[0]..window[1]] {
                if target as usize >= node_count {
                    return Err(GraphError::BadEncoding("TRGR target out of range"));
                }
                if matches!(previous, Some(value) if target <= value) {
                    return Err(GraphError::BadEncoding("TRGR row is not sorted"));
                }
                previous = Some(target);
            }
        }
        Ok(Self {
            csr: Csr { offsets, targets },
            labels,
        })
    }

    fn bfs_path(
        &self,
        start: u32,
        target: u32,
        max_hops: usize,
        allow_unknown: bool,
    ) -> Option<Vec<u32>> {
        if start == target {
            return Some(vec![start]);
        }
        let nodes = self.csr.node_count();
        let mut parent = vec![u32::MAX; nodes];
        let mut depth = vec![usize::MAX; nodes];
        let mut queue = VecDeque::new();
        depth[start as usize] = 0;
        queue.push_back(start);
        while let Some(node) = queue.pop_front() {
            let hops = depth[node as usize];
            if hops >= max_hops {
                continue;
            }
            let base = self.csr.offsets[node as usize];
            for (offset, &next) in self.csr.row(node as usize).iter().enumerate() {
                let label = self.labels[base + offset];
                let allowed =
                    label == TruthValue::True || (allow_unknown && label == TruthValue::Unknown);
                if !allowed || depth[next as usize] != usize::MAX {
                    continue;
                }
                depth[next as usize] = hops + 1;
                parent[next as usize] = node;
                if next == target {
                    let mut path = vec![target];
                    let mut cursor = target;
                    while cursor != start {
                        cursor = parent[cursor as usize];
                        path.push(cursor);
                    }
                    path.reverse();
                    return Some(path);
                }
                queue.push_back(next);
            }
        }
        None
    }
}

/// A labelled vector for [`annotated_knn`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Point {
    /// Caller-defined identifier, echoed in results.
    pub id: u32,
    /// Integer coordinates.
    pub vector: Vec<i64>,
    /// Truth label: `True` = confirmed, `Unknown` = uncertain, `False` =
    /// excluded.
    pub label: TruthValue,
}

/// One scored point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Neighbor {
    /// Point identifier.
    pub id: u32,
    /// Squared Euclidean distance (exact integer arithmetic).
    pub distance: i128,
    /// Point label.
    pub label: TruthValue,
}

/// Top-k neighbours split by three-valued label.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnnNeighbors {
    /// Top-k over `True` points.
    pub certain: Vec<Neighbor>,
    /// Top-k over `Unknown` points.
    pub uncertain: Vec<Neighbor>,
    /// Top-k over `False` points, kept for diagnostics.
    pub excluded: Vec<Neighbor>,
}

/// Three-valued k-nearest-neighbour query over labelled integer vectors.
///
/// Distance is exact squared Euclidean (`i128`), so results are deterministic.
/// Each bucket holds at most `k` neighbours, ordered by distance then `id`.
pub fn annotated_knn(
    points: &[Point],
    query: &[i64],
    k: usize,
) -> Result<AnnNeighbors, GraphError> {
    if query.is_empty() {
        return Err(GraphError::EmptyVector);
    }
    if query.len() > MAX_DIMENSIONS {
        return Err(GraphError::TooManyDimensions {
            dimensions: query.len(),
            max: MAX_DIMENSIONS,
        });
    }
    let mut scored = Vec::with_capacity(points.len());
    for point in points {
        if point.vector.len() != query.len() {
            return Err(GraphError::DimensionMismatch {
                expected: query.len(),
                actual: point.vector.len(),
            });
        }
        let mut distance: i128 = 0;
        for (coordinate, wanted) in point.vector.iter().zip(query) {
            let difference = i128::from(*coordinate) - i128::from(*wanted);
            let square = difference
                .checked_mul(difference)
                .ok_or(GraphError::DistanceOverflow)?;
            distance = distance
                .checked_add(square)
                .ok_or(GraphError::DistanceOverflow)?;
        }
        scored.push(Neighbor {
            id: point.id,
            distance,
            label: point.label,
        });
    }
    scored.sort_by(|left, right| {
        left.distance
            .cmp(&right.distance)
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut answer = AnnNeighbors::default();
    for neighbor in scored {
        let bucket = match neighbor.label {
            TruthValue::True => &mut answer.certain,
            TruthValue::Unknown => &mut answer.uncertain,
            TruthValue::False => &mut answer.excluded,
        };
        if bucket.len() < k {
            bucket.push(neighbor);
        }
    }
    Ok(answer)
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

const fn label_code(label: TruthValue) -> u8 {
    match label {
        TruthValue::False => 0,
        TruthValue::Unknown => 1,
        TruthValue::True => 2,
    }
}

const fn label_from_code(code: u8) -> Option<TruthValue> {
    match code {
        0 => Some(TruthValue::False),
        1 => Some(TruthValue::Unknown),
        2 => Some(TruthValue::True),
        _ => None,
    }
}

/// Encodes labelled points as `TRPN`.
///
/// Layout: magic `TRPN`, version byte, point count (`u32` LE), then per point
/// `id` (`u32` LE), label byte, dimension count (`u32` LE) and `i64`
/// coordinates.
pub fn encode_points(points: &[Point]) -> Vec<u8> {
    let mut size = 9;
    for point in points {
        size += 9 + point.vector.len() * 8;
    }
    let mut bytes = Vec::with_capacity(size);
    bytes.extend_from_slice(TRPN_MAGIC);
    bytes.push(TRPN_VERSION);
    bytes.extend_from_slice(&(points.len() as u32).to_le_bytes());
    for point in points {
        bytes.extend_from_slice(&point.id.to_le_bytes());
        bytes.push(label_code(point.label));
        bytes.extend_from_slice(&(point.vector.len() as u32).to_le_bytes());
        for coordinate in &point.vector {
            bytes.extend_from_slice(&coordinate.to_le_bytes());
        }
    }
    bytes
}

/// Decodes a `TRPN` point set produced by [`encode_points`].
pub fn decode_points(bytes: &[u8]) -> Result<Vec<Point>, GraphError> {
    if bytes.len() < 9 || &bytes[0..4] != TRPN_MAGIC {
        return Err(GraphError::BadEncoding("bad TRPN header"));
    }
    if bytes[4] != TRPN_VERSION {
        return Err(GraphError::BadEncoding("unsupported TRPN version"));
    }
    let count = read_u32(bytes, 5) as usize;
    if count > MAX_POINTS {
        return Err(GraphError::TooManyPoints {
            points: count,
            max: MAX_POINTS,
        });
    }
    let mut cursor = 9;
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        if cursor + 9 > bytes.len() {
            return Err(GraphError::BadEncoding("TRPN truncated point header"));
        }
        let id = read_u32(bytes, cursor);
        cursor += 4;
        let label =
            label_from_code(bytes[cursor]).ok_or(GraphError::BadEncoding("bad TRPN label"))?;
        cursor += 1;
        let dimensions = read_u32(bytes, cursor) as usize;
        cursor += 4;
        if dimensions > MAX_DIMENSIONS {
            return Err(GraphError::TooManyDimensions {
                dimensions,
                max: MAX_DIMENSIONS,
            });
        }
        let needed = dimensions
            .checked_mul(8)
            .ok_or(GraphError::BadEncoding("TRPN size overflows"))?;
        if cursor + needed > bytes.len() {
            return Err(GraphError::BadEncoding("TRPN truncated coordinates"));
        }
        let mut vector = Vec::with_capacity(dimensions);
        for _ in 0..dimensions {
            let mut coordinate = [0u8; 8];
            coordinate.copy_from_slice(&bytes[cursor..cursor + 8]);
            cursor += 8;
            vector.push(i64::from_le_bytes(coordinate));
        }
        points.push(Point { id, vector, label });
    }
    if cursor != bytes.len() {
        return Err(GraphError::BadEncoding("TRPN trailing bytes"));
    }
    Ok(points)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(edges: &[(u32, u32, TruthValue)]) -> AnnotatedGraph {
        let nodes = edges
            .iter()
            .map(|&(from, to, _)| from.max(to) as usize + 1)
            .max()
            .unwrap_or(0);
        AnnotatedGraph::from_edges(nodes, edges).expect("build graph")
    }

    #[test]
    fn csr_sorts_dedups_and_rejects_out_of_range_edges() {
        let csr = Csr::from_edges(3, &[(1, 2), (0, 2), (0, 1), (0, 1)]).expect("csr");
        assert_eq!(csr.node_count(), 3);
        assert_eq!(csr.edge_count(), 3);
        assert_eq!(csr.neighbors(0).expect("row 0"), &[1, 2]);
        assert_eq!(csr.neighbors(2).expect("row 2"), &[] as &[u32]);
        assert!(csr.has_edge(0, 2).expect("has edge"));
        assert!(!csr.has_edge(2, 0).expect("has edge"));

        let error = Csr::from_edges(2, &[(0, 2)]).expect_err("out of range");
        assert!(matches!(error, GraphError::EdgeOutOfRange { .. }));
        let error = csr.neighbors(3).expect_err("node out of range");
        assert!(matches!(error, GraphError::NodeOutOfRange { .. }));
    }

    #[test]
    fn true_path_reaches_certainly() {
        let graph = build(&[(0, 1, TruthValue::True), (1, 2, TruthValue::True)]);
        let reach = graph.reach(0, 2, 2).expect("reach");
        assert_eq!(reach.answer, RepairAnswer::Certain);
        assert_eq!(reach.hops, Some(2));
        assert_eq!(reach.path, vec![0, 1, 2]);
    }

    #[test]
    fn unknown_edge_makes_reach_contingent() {
        let graph = build(&[(0, 1, TruthValue::True), (1, 2, TruthValue::Unknown)]);
        let reach = graph.reach(0, 2, 2).expect("reach");
        assert_eq!(reach.answer, RepairAnswer::Contingent);
        assert_eq!(reach.path, vec![0, 1, 2]);

        // A definite shortcut wins over the uncertain path.
        let graph = build(&[
            (0, 1, TruthValue::True),
            (1, 2, TruthValue::Unknown),
            (0, 2, TruthValue::True),
        ]);
        let reach = graph.reach(0, 2, 2).expect("reach");
        assert_eq!(reach.answer, RepairAnswer::Certain);
        assert_eq!(reach.path, vec![0, 2]);
    }

    #[test]
    fn false_edge_blocks_reach() {
        let graph = build(&[(0, 1, TruthValue::True), (1, 2, TruthValue::False)]);
        let reach = graph.reach(0, 2, 2).expect("reach");
        assert_eq!(reach.answer, RepairAnswer::Impossible);
        assert_eq!(reach.hops, None);
        assert!(reach.path.is_empty());
    }

    #[test]
    fn max_hops_bounds_reach() {
        let graph = build(&[(0, 1, TruthValue::True), (1, 2, TruthValue::True)]);
        assert_eq!(
            graph.reach(0, 2, 1).expect("reach").answer,
            RepairAnswer::Impossible
        );
        let reach = graph.reach(0, 1, 1).expect("reach");
        assert_eq!(reach.answer, RepairAnswer::Certain);
        // A zero-hop query only succeeds from a node to itself.
        assert_eq!(graph.reach(0, 0, 0).expect("reach").path, vec![0]);
    }

    #[test]
    fn reachable_within_returns_breadth_first_layers() {
        let graph = build(&[
            (0, 1, TruthValue::True),
            (0, 2, TruthValue::Unknown),
            (1, 3, TruthValue::True),
            (2, 4, TruthValue::False),
        ]);
        let reached = graph.reachable_within(0, 2).expect("reached");
        assert_eq!(reached, vec![(1, 1), (2, 1), (3, 2)]);
        let one_hop = graph.reachable_within(0, 1).expect("one hop");
        assert_eq!(one_hop, vec![(1, 1), (2, 1)]);
    }

    #[test]
    fn annotated_knn_groups_by_label_and_breaks_ties() {
        let points = vec![
            Point {
                id: 0,
                vector: vec![0, 0],
                label: TruthValue::True,
            },
            Point {
                id: 1,
                vector: vec![1, 0],
                label: TruthValue::Unknown,
            },
            Point {
                id: 2,
                vector: vec![2, 0],
                label: TruthValue::False,
            },
            Point {
                id: 3,
                vector: vec![0, 1],
                label: TruthValue::Unknown,
            },
        ];
        let answer = annotated_knn(&points, &[0, 0], 2).expect("knn");
        assert_eq!(answer.certain.len(), 1);
        assert_eq!(answer.certain[0].id, 0);
        assert_eq!(answer.certain[0].distance, 0);
        assert_eq!(
            answer.uncertain.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(
            answer.excluded.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn annotated_knn_rejects_dimension_mismatch_and_overflow() {
        let points = vec![Point {
            id: 0,
            vector: vec![1],
            label: TruthValue::True,
        }];
        let error = annotated_knn(&points, &[1, 2], 1).expect_err("dimensions");
        assert!(matches!(error, GraphError::DimensionMismatch { .. }));
        let error = annotated_knn(&[], &[], 1).expect_err("empty query");
        assert!(matches!(error, GraphError::EmptyVector));

        let huge = vec![Point {
            id: 0,
            vector: vec![i64::MAX],
            label: TruthValue::True,
        }];
        let error = annotated_knn(&huge, &[i64::MIN], 1).expect_err("overflow");
        assert!(matches!(error, GraphError::DistanceOverflow));
    }

    #[test]
    fn trgr_roundtrips_with_labels() {
        let original = build(&[
            (0, 1, TruthValue::True),
            (0, 2, TruthValue::Unknown),
            (2, 0, TruthValue::False),
        ]);
        let bytes = original.encode();
        assert_eq!(&bytes[0..4], TRGR_MAGIC);
        assert_eq!(bytes[4], TRGR_VERSION);

        let restored = AnnotatedGraph::decode(&bytes).expect("decode");
        assert_eq!(restored, original);
        assert_eq!(restored.node_count(), 3);
        assert_eq!(restored.edge_count(), 3);
        assert_eq!(
            restored.edge_label(2, 0).expect("label"),
            Some(TruthValue::False)
        );
        assert_eq!(
            restored.reach(0, 2, 2).expect("reach").answer,
            RepairAnswer::Contingent
        );
    }

    #[test]
    fn trgr_rejects_corrupt_bytes() {
        let original = build(&[(0, 1, TruthValue::True)]);
        let bytes = original.encode();

        let mut bad_magic = bytes.clone();
        bad_magic[0] = b'X';
        assert!(matches!(
            AnnotatedGraph::decode(&bad_magic),
            Err(GraphError::BadEncoding(_))
        ));

        let mut bad_version = bytes.clone();
        bad_version[4] = 9;
        assert!(matches!(
            AnnotatedGraph::decode(&bad_version),
            Err(GraphError::BadEncoding(_))
        ));

        assert!(matches!(
            AnnotatedGraph::decode(&bytes[..5]),
            Err(GraphError::BadEncoding(_))
        ));

        let mut bad_label = bytes.clone();
        let last = bad_label.len() - 1;
        bad_label[last] = 7;
        assert!(matches!(
            AnnotatedGraph::decode(&bad_label),
            Err(GraphError::BadEncoding(_))
        ));

        let mut bad_offsets = bytes.clone();
        bad_offsets[13] = 1;
        assert!(matches!(
            AnnotatedGraph::decode(&bad_offsets),
            Err(GraphError::BadEncoding(_))
        ));

        let mut truncated = bytes.clone();
        truncated.pop();
        assert!(matches!(
            AnnotatedGraph::decode(&truncated),
            Err(GraphError::BadEncoding(_))
        ));
    }

    #[test]
    fn trpn_roundtrips_points() {
        let points = vec![
            Point {
                id: 0,
                vector: vec![1, -2],
                label: TruthValue::True,
            },
            Point {
                id: 7,
                vector: vec![3],
                label: TruthValue::Unknown,
            },
        ];
        let bytes = encode_points(&points);
        assert_eq!(&bytes[0..4], TRPN_MAGIC);
        assert_eq!(decode_points(&bytes).expect("decode"), points);
    }

    #[test]
    fn trpn_rejects_corrupt_bytes() {
        let points = vec![Point {
            id: 0,
            vector: vec![1],
            label: TruthValue::False,
        }];
        let bytes = encode_points(&points);

        let mut bad_magic = bytes.clone();
        bad_magic[0] = b'X';
        assert!(matches!(
            decode_points(&bad_magic),
            Err(GraphError::BadEncoding(_))
        ));

        let mut bad_label = bytes.clone();
        bad_label[14] = 9;
        assert!(matches!(
            decode_points(&bad_label),
            Err(GraphError::BadEncoding(_))
        ));

        let mut truncated = bytes.clone();
        truncated.pop();
        assert!(matches!(
            decode_points(&truncated),
            Err(GraphError::BadEncoding(_))
        ));
    }

    #[test]
    fn annotated_knn_handles_negative_coordinates_and_bucket_limits() {
        let points = vec![
            Point {
                id: 3,
                vector: vec![-2, 1],
                label: TruthValue::True,
            },
            Point {
                id: 1,
                vector: vec![2, -1],
                label: TruthValue::True,
            },
            Point {
                id: 2,
                vector: vec![-2, 1],
                label: TruthValue::Unknown,
            },
        ];
        let answer = annotated_knn(&points, &[0, 0], 2).expect("knn");
        // Both true points are 5 away; ties break on the smaller id.
        assert_eq!(
            answer
                .certain
                .iter()
                .map(|neighbor| (neighbor.id, neighbor.distance))
                .collect::<Vec<_>>(),
            vec![(1, 5), (3, 5)]
        );
        assert_eq!(
            answer
                .uncertain
                .iter()
                .map(|neighbor| neighbor.id)
                .collect::<Vec<_>>(),
            vec![2]
        );

        let none = annotated_knn(&points, &[0, 0], 0).expect("k = 0");
        assert!(none.certain.is_empty());
        assert!(none.uncertain.is_empty());
        assert!(none.excluded.is_empty());

        let all = annotated_knn(&points, &[0, 0], 99).expect("k beyond the point count");
        assert_eq!(all.certain.len(), 2);
        assert_eq!(all.uncertain.len(), 1);
    }

    #[test]
    fn annotated_knn_rejects_oversized_queries_and_mismatched_points() {
        let too_wide = vec![0i64; MAX_DIMENSIONS + 1];
        let error = annotated_knn(&[], &too_wide, 1).expect_err("query too wide");
        assert!(matches!(error, GraphError::TooManyDimensions { .. }));

        let points = vec![Point {
            id: 0,
            vector: vec![1, 2, 3],
            label: TruthValue::True,
        }];
        let error = annotated_knn(&points, &[0, 0], 1).expect_err("point too wide");
        assert!(matches!(
            error,
            GraphError::DimensionMismatch {
                expected: 2,
                actual: 3
            }
        ));
    }

    #[test]
    fn graph_construction_enforces_node_and_edge_limits() {
        let error = AnnotatedGraph::from_edges(MAX_NODES + 1, &[]).expect_err("too many nodes");
        assert!(matches!(error, GraphError::TooManyNodes { .. }));
        let error = Csr::from_edges(MAX_NODES + 1, &[]).expect_err("too many csr nodes");
        assert!(matches!(error, GraphError::TooManyNodes { .. }));

        // Five source rows over MAX_NODES targets exceed MAX_EDGES.
        let mut plain = Vec::new();
        for from in 0..5u32 {
            for to in 0..MAX_NODES as u32 {
                plain.push((from, to));
            }
        }
        assert!(plain.len() > MAX_EDGES);
        let error = Csr::from_edges(MAX_NODES, &plain).expect_err("too many edges");
        assert!(matches!(error, GraphError::TooManyEdges { .. }));

        let labelled = plain
            .iter()
            .map(|&(from, to)| (from, to, TruthValue::True))
            .collect::<Vec<_>>();
        let error =
            AnnotatedGraph::from_edges(MAX_NODES, &labelled).expect_err("too many annotated edges");
        assert!(matches!(error, GraphError::TooManyEdges { .. }));
    }

    #[test]
    fn encoded_headers_reject_oversized_counts() {
        let mut graph_bytes = Vec::new();
        graph_bytes.extend_from_slice(TRGR_MAGIC);
        graph_bytes.push(TRGR_VERSION);
        graph_bytes.extend_from_slice(&((MAX_NODES + 1) as u32).to_le_bytes());
        graph_bytes.extend_from_slice(&0u32.to_le_bytes());
        let error = AnnotatedGraph::decode(&graph_bytes).expect_err("too many nodes");
        assert!(matches!(error, GraphError::TooManyNodes { .. }));

        let mut point_bytes = Vec::new();
        point_bytes.extend_from_slice(TRPN_MAGIC);
        point_bytes.push(TRPN_VERSION);
        point_bytes.extend_from_slice(&((MAX_POINTS + 1) as u32).to_le_bytes());
        let error = decode_points(&point_bytes).expect_err("too many points");
        assert!(matches!(error, GraphError::TooManyPoints { .. }));
    }

    #[test]
    fn trpn_roundtrips_empty_sets_and_coordinate_extremes() {
        assert_eq!(
            decode_points(&encode_points(&[])).expect("empty set"),
            Vec::new()
        );
        let points = vec![
            Point {
                id: 0,
                vector: vec![i64::MIN, i64::MAX],
                label: TruthValue::False,
            },
            Point {
                id: u32::MAX,
                vector: Vec::new(),
                label: TruthValue::True,
            },
        ];
        assert_eq!(
            decode_points(&encode_points(&points)).expect("extremes"),
            points
        );
    }

    #[test]
    fn reach_handles_huge_hop_bounds_and_out_of_range_nodes() {
        let graph = build(&[(0, 1, TruthValue::True), (1, 2, TruthValue::True)]);
        let reach = graph.reach(0, 2, usize::MAX).expect("huge hop bound");
        assert_eq!(reach.answer, RepairAnswer::Certain);
        assert_eq!(reach.hops, Some(2));

        let error = graph.reach(9, 0, 1).expect_err("start out of range");
        assert!(matches!(error, GraphError::NodeOutOfRange { .. }));
        let error = graph.reach(0, 9, 1).expect_err("target out of range");
        assert!(matches!(error, GraphError::NodeOutOfRange { .. }));
        let error = graph
            .reachable_within(9, 1)
            .expect_err("start out of range");
        assert!(matches!(error, GraphError::NodeOutOfRange { .. }));
    }
}
