//! 生成树视图：父指针 + DFS 先序；后序用逆先序（树上的合法后序）。

/// 一棵（有根）树：由 [`crate::TritGraph::dfs_tree`] 生成，也可以手工构造用于测试。
#[derive(Clone, Debug)]
pub struct Tree {
    parent: Vec<Option<usize>>,
    preorder: Vec<usize>,
}

impl Tree {
    pub(crate) fn new(parent: Vec<Option<usize>>, preorder: Vec<usize>) -> Self {
        Self { parent, preorder }
    }

    /// 节点数（含不在树上的节点）。
    pub fn len(&self) -> usize {
        self.parent.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    /// 父节点；根/未访问节点为 `None`。
    pub fn parent(&self, node: usize) -> Option<usize> {
        self.parent.get(node).copied().flatten()
    }

    /// 根（没有父的节点）。
    pub fn roots(&self) -> impl Iterator<Item = usize> + '_ {
        self.parent
            .iter()
            .enumerate()
            .filter(|(_, parent)| parent.is_none())
            .map(|(node, _)| node)
    }

    /// DFS 先序。
    pub fn preorder(&self) -> &[usize] {
        &self.preorder
    }

    /// 后序 = 逆 DFS 先序（子一定在父之前）。
    pub fn postorder(&self) -> Vec<usize> {
        let mut order = self.preorder.clone();
        order.reverse();
        order
    }

    /// 子节点（按 DFS 先序里的出现顺序）。
    pub fn children(&self, node: usize) -> Vec<usize> {
        let mut children: Vec<usize> = self
            .preorder
            .iter()
            .copied()
            .filter(|&child| self.parent(child) == Some(node))
            .collect();
        let order = |v: usize| {
            self.preorder
                .iter()
                .position(|&x| x == v)
                .unwrap_or(usize::MAX)
        };
        children.sort_by_key(|&v| order(v));
        children
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Tree {
        // 0 -> 1 -> 3, 0 -> 2
        Tree::new(vec![None, Some(0), Some(0), Some(1)], vec![0, 2, 1, 3])
    }

    #[test]
    fn parent_and_roots() {
        let tree = sample();
        assert_eq!(tree.parent(0), None);
        assert_eq!(tree.parent(3), Some(1));
        assert_eq!(tree.roots().collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn children_follow_preorder() {
        let tree = sample();
        assert_eq!(tree.children(0), vec![2, 1]);
        assert_eq!(tree.children(1), vec![3]);
        assert!(tree.children(3).is_empty());
    }

    #[test]
    fn postorder_puts_children_before_parent() {
        let tree = sample();
        let post = tree.postorder();
        let pos = |v: usize| post.iter().position(|&x| x == v).expect("包含");
        for v in 0..4 {
            if let Some(p) = tree.parent(v) {
                assert!(pos(v) < pos(p));
            }
        }
    }
}
