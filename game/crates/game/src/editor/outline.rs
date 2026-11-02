//! The scene's object graph: named pieces arranged into nested groups.
//!
//! The graph is deliberately separate from the Bevy entity hierarchy. Pieces
//! stay plain [`Piece`] values (copyable, serializable, easy to snapshot), and
//! this module owns the extra authoring metadata: user labels, groups, and
//! which group each piece belongs to. [`SceneOutline::rows`] flattens it into the
//! depth-sorted list the outliner panel draws.
//!
//! Group ids share the editor's `next_id` counter with piece ids. A group id is
//! therefore unique across both namespaces, which lets one map key label
//! either kind of object and makes parent references unambiguous.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::*;

/// Longest label the editor stores, in characters.
pub(crate) const LABEL_LIMIT: usize = 64;

/// One node in the object graph. `Root` is the scene itself, used as the drop
/// target for objects that should not belong to any group.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) enum GraphNode {
    #[default]
    Root,
    Character,
    Group(u32),
    Piece(u32),
}

impl GraphNode {
    /// Only pieces and groups can be renamed or dragged between groups.
    pub(crate) fn is_object(self) -> bool {
        matches!(self, Self::Piece(_) | Self::Group(_))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Group {
    pub(crate) id: u32,
    /// Empty means "use the generated `Group <id>` label".
    pub(crate) name: String,
    pub(crate) parent: Option<u32>,
    pub(crate) collapsed: bool,
}

impl Group {
    pub(crate) fn display_name(&self) -> String {
        let name = self.name.trim();
        if name.is_empty() {
            format!("Group {}", self.id)
        } else {
            name.to_owned()
        }
    }
}

/// Authoring metadata layered over the pieces: labels, groups, membership.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SceneOutline {
    pub(crate) groups: Vec<Group>,
    /// Piece id -> owning group id. Missing means root.
    pub(crate) membership: BTreeMap<u32, u32>,
    /// Custom piece labels. Missing means "use the shape name".
    pub(crate) labels: BTreeMap<u32, String>,
    /// Pieces that cannot be picked from the viewport. They remain selectable
    /// from the object graph so their lock can be released.
    pub(crate) locked: BTreeSet<u32>,
    /// Monotonic counter the UI uses to know when to rebuild its rows.
    pub(crate) revision: u64,
}

/// One visible row of the flattened tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GraphRow {
    pub(crate) node: GraphNode,
    pub(crate) depth: usize,
    pub(crate) has_children: bool,
    pub(crate) collapsed: bool,
    pub(crate) child_count: usize,
}

/// Strip control characters and clamp the length. Shared with the save loader
/// so a hand-edited JSON cannot inject newlines into the panel.
pub(crate) fn clean_label(text: &str) -> String {
    let mut out = String::new();
    let mut count = 0;
    for c in text.chars() {
        if c.is_control() {
            continue;
        }
        if count >= LABEL_LIMIT {
            break;
        }
        out.push(c);
        count += 1;
    }
    out.trim().to_owned()
}

impl SceneOutline {
    pub(crate) fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn group(&self, id: u32) -> Option<&Group> {
        self.groups.iter().find(|group| group.id == id)
    }

    pub(crate) fn group_mut(&mut self, id: u32) -> Option<&mut Group> {
        self.groups.iter_mut().find(|group| group.id == id)
    }

    /// A piece's custom label, if it has one.
    pub(crate) fn label(&self, piece: u32) -> Option<&str> {
        self.labels
            .get(&piece)
            .map(String::as_str)
            .filter(|label| !label.is_empty())
    }

    pub(crate) fn is_locked(&self, piece: u32) -> bool {
        self.locked.contains(&piece)
    }

    /// Flip a piece's viewport-lock state and return the new value.
    pub(crate) fn toggle_lock(&mut self, piece: u32) -> bool {
        let locked = if self.locked.remove(&piece) {
            false
        } else {
            self.locked.insert(piece);
            true
        };
        self.bump();
        locked
    }

    pub(crate) fn set_label(&mut self, piece: u32, label: &str) {
        let label = clean_label(label);
        if label.is_empty() {
            self.labels.remove(&piece);
        } else {
            self.labels.insert(piece, label);
        }
        self.bump();
    }

    /// The group a piece currently belongs to, if that group still exists.
    pub(crate) fn piece_parent(&self, piece: u32) -> Option<u32> {
        self.membership
            .get(&piece)
            .copied()
            .filter(|group| self.group(*group).is_some())
    }

    /// Whether `candidate` is `ancestor` itself or one of its descendants.
    /// Used to reject dropping a group into its own subtree.
    pub(crate) fn is_descendant(&self, candidate: u32, ancestor: u32) -> bool {
        let mut current = Some(candidate);
        for _ in 0..=LABEL_LIMIT {
            let Some(id) = current else {
                return false;
            };
            if id == ancestor {
                return true;
            }
            current = self.group(id).and_then(|group| group.parent);
        }
        // A cycle: treat it as a descendant so the move cannot create one.
        true
    }

    /// Move a piece or group under `parent` (`None` is the scene root).
    /// Returns false when nothing changed or the move would make a cycle.
    pub(crate) fn move_node(&mut self, node: GraphNode, parent: Option<u32>) -> bool {
        if let Some(parent) = parent
            && self.group(parent).is_none()
        {
            return false;
        }
        match node {
            GraphNode::Piece(id) => {
                if self.piece_parent(id) == parent {
                    return false;
                }
                match parent {
                    Some(group) => {
                        self.membership.insert(id, group);
                    }
                    None => {
                        self.membership.remove(&id);
                    }
                }
                self.bump();
                true
            }
            GraphNode::Group(id) => {
                if parent == Some(id) {
                    return false;
                }
                if let Some(parent) = parent
                    && self.is_descendant(parent, id)
                {
                    return false;
                }
                let Some(group) = self.group_mut(id) else {
                    return false;
                };
                if group.parent == parent {
                    return false;
                }
                group.parent = parent;
                self.bump();
                true
            }
            GraphNode::Root | GraphNode::Character => false,
        }
    }

    /// Dissolve a group, promoting its children into the group's parent.
    /// Returns the group's former parent if the group existed.
    pub(crate) fn ungroup(&mut self, id: u32) -> Option<Option<u32>> {
        let parent = self.group(id)?.parent;
        for group in &mut self.groups {
            if group.parent == Some(id) {
                group.parent = parent;
            }
        }
        let members: Vec<u32> = self
            .membership
            .iter()
            .filter(|(_, group)| **group == id)
            .map(|(piece, _)| *piece)
            .collect();
        for piece in members {
            match parent {
                Some(parent) => {
                    self.membership.insert(piece, parent);
                }
                None => {
                    self.membership.remove(&piece);
                }
            }
        }
        self.groups.retain(|group| group.id != id);
        self.bump();
        Some(parent)
    }

    /// Drop labels/membership that point at pieces or groups which no longer
    /// exist. Returns true when something changed.
    pub(crate) fn prune(&mut self, pieces: &[Piece]) -> bool {
        let piece_ids: HashSet<u32> = pieces.iter().map(|piece| piece.id).collect();
        let group_ids: HashSet<u32> = self.groups.iter().map(|group| group.id).collect();
        let before = (
            self.membership.len(),
            self.labels.len(),
            self.locked.len(),
            self.groups.iter().filter(|g| g.parent.is_some()).count(),
        );
        self.membership
            .retain(|piece, group| piece_ids.contains(piece) && group_ids.contains(group));
        self.labels.retain(|piece, _| piece_ids.contains(piece));
        self.locked.retain(|piece| piece_ids.contains(piece));
        for group in &mut self.groups {
            if group
                .parent
                .is_some_and(|parent| !group_ids.contains(&parent))
            {
                group.parent = None;
            }
        }
        let after = (
            self.membership.len(),
            self.labels.len(),
            self.locked.len(),
            self.groups.iter().filter(|g| g.parent.is_some()).count(),
        );
        if before != after {
            self.bump();
            true
        } else {
            false
        }
    }

    fn child_count(&self, parent: u32, pieces: &[Piece], group_ids: &HashSet<u32>) -> usize {
        let groups = self
            .groups
            .iter()
            .filter(|group| {
                group.parent.filter(|parent| group_ids.contains(parent)) == Some(parent)
            })
            .count();
        let members = pieces
            .iter()
            .filter(|piece| {
                self.membership
                    .get(&piece.id)
                    .copied()
                    .filter(|group| group_ids.contains(group))
                    == Some(parent)
            })
            .count();
        groups + members
    }

    /// Flatten the tree into rows in display order. Collapsed groups emit their
    /// own row but not their children.
    pub(crate) fn rows(&self, pieces: &[Piece], has_character: bool) -> Vec<GraphRow> {
        let group_ids: HashSet<u32> = self.groups.iter().map(|group| group.id).collect();
        let mut rows = Vec::with_capacity(self.groups.len() + pieces.len() + 1);
        let mut visited = HashSet::new();
        self.walk(None, 0, pieces, &group_ids, &mut visited, &mut rows);
        if has_character {
            rows.push(GraphRow {
                node: GraphNode::Character,
                depth: 0,
                has_children: false,
                collapsed: false,
                child_count: 0,
            });
        }
        rows
    }

    fn walk(
        &self,
        parent: Option<u32>,
        depth: usize,
        pieces: &[Piece],
        group_ids: &HashSet<u32>,
        visited: &mut HashSet<u32>,
        rows: &mut Vec<GraphRow>,
    ) {
        if depth > LABEL_LIMIT {
            return;
        }
        let mut children: Vec<(u32, GraphNode)> = Vec::new();
        for group in &self.groups {
            let effective = group.parent.filter(|parent| group_ids.contains(parent));
            if effective == parent {
                children.push((group.id, GraphNode::Group(group.id)));
            }
        }
        for piece in pieces {
            let effective = self
                .membership
                .get(&piece.id)
                .copied()
                .filter(|group| group_ids.contains(group));
            if effective == parent {
                children.push((piece.id, GraphNode::Piece(piece.id)));
            }
        }
        // Group and piece ids share one counter, so sorting by id preserves the
        // order in which objects were added to the scene.
        // Groups read as folders, so they come before loose objects at the
        // same depth even when ids interleave.
        children.sort_by_key(|(id, node)| {
            let rank = if matches!(node, GraphNode::Group(_)) {
                0
            } else {
                1
            };
            (rank, *id)
        });

        for (_, node) in children {
            match node {
                GraphNode::Group(group_id) => {
                    if !visited.insert(group_id) {
                        continue;
                    }
                    let Some(group) = self.group(group_id) else {
                        continue;
                    };
                    let child_count = self.child_count(group_id, pieces, group_ids);
                    rows.push(GraphRow {
                        node,
                        depth,
                        has_children: child_count > 0,
                        collapsed: group.collapsed,
                        child_count,
                    });
                    if !group.collapsed {
                        self.walk(Some(group_id), depth + 1, pieces, group_ids, visited, rows);
                    }
                }
                GraphNode::Piece(_) => rows.push(GraphRow {
                    node,
                    depth,
                    has_children: false,
                    collapsed: false,
                    child_count: 0,
                }),
                GraphNode::Root | GraphNode::Character => {}
            }
        }
    }
}

impl Editor {
    /// Allocate an id that collides with neither a piece nor a group. The
    /// editor normally keeps `next_id` ahead of both, but a hand-built level or
    /// an old save can leave it behind; the graph must never share an id
    /// between a piece and a group because labels and parents are keyed by id.
    fn next_free_id(&mut self) -> u32 {
        let mut id = self.next_id.max(1);
        // At most one id in this window can be free if every other id is in
        // use, and the window is one larger than the number of existing
        // objects, so a free slot is guaranteed.
        for _ in 0..=(self.pieces.len() + self.outline.groups.len()) {
            let used = self.pieces.iter().any(|piece| piece.id == id)
                || self.outline.groups.iter().any(|group| group.id == id);
            if !used {
                break;
            }
            id = id.saturating_add(1);
        }
        self.next_id = id.saturating_add(1);
        id
    }

    pub(crate) fn piece(&self, id: u32) -> Option<&Piece> {
        self.pieces.iter().find(|piece| piece.id == id)
    }

    /// Display label for a graph node. Pieces fall back to their shape name;
    /// groups to `Group <id>` when unnamed.
    pub(crate) fn node_label(&self, node: GraphNode) -> String {
        match node {
            GraphNode::Root => "SCENE".to_owned(),
            GraphNode::Character => "Character".to_owned(),
            GraphNode::Group(id) => self
                .outline
                .group(id)
                .map(Group::display_name)
                .unwrap_or_else(|| format!("Group {id}")),
            GraphNode::Piece(id) => {
                if let Some(label) = self.outline.label(id) {
                    return label.to_owned();
                }
                self.piece(id)
                    .and_then(|piece| shapes().get(piece.shape))
                    .map(|shape| shape.name.to_owned())
                    .unwrap_or_else(|| format!("Object {id}"))
            }
        }
    }

    /// The muted suffix drawn beside a row's label.
    pub(crate) fn node_suffix(&self, node: GraphNode) -> Option<String> {
        match node {
            GraphNode::Root => Some(format!("{}", self.pieces.len())),
            GraphNode::Piece(id) => Some(format!("#{id}")),
            GraphNode::Group(id) => {
                let groups = self
                    .outline
                    .groups
                    .iter()
                    .filter(|group| group.parent == Some(id))
                    .count();
                let pieces = self
                    .pieces
                    .iter()
                    .filter(|piece| self.outline.piece_parent(piece.id) == Some(id))
                    .count();
                Some(format!("({})", groups + pieces))
            }
            GraphNode::Character => None,
        }
    }

    /// Whether a graph node is locked against viewport selection.
    pub(crate) fn node_locked(&self, node: GraphNode) -> bool {
        matches!(node, GraphNode::Piece(id) if self.outline.is_locked(id))
    }

    /// Toggle viewport-lock for a piece as a single undo step.
    pub(crate) fn toggle_lock(&mut self, id: u32) -> bool {
        if self.piece(id).is_none() {
            return false;
        }
        self.commit();
        let locked = self.outline.toggle_lock(id);
        self.hard_dirty = true;
        self.note = if locked { "locked" } else { "unlocked" };
        locked
    }

    /// Whether a node's label can be edited.
    pub(crate) fn node_renamable(&self, node: GraphNode) -> bool {
        match node {
            GraphNode::Piece(id) => self.piece(id).is_some(),
            GraphNode::Group(id) => self.outline.group(id).is_some(),
            GraphNode::Root | GraphNode::Character => false,
        }
    }

    /// Wrap the selected piece or group in a fresh parent group. Returns the new
    /// group id, or None for nodes that cannot be grouped.
    pub(crate) fn group_node(&mut self, node: GraphNode) -> Option<u32> {
        let parent = match node {
            GraphNode::Piece(id) => {
                if self.piece(id).is_none() {
                    return None;
                }
                self.outline.piece_parent(id)
            }
            GraphNode::Group(id) => self.outline.group(id)?.parent,
            GraphNode::Root | GraphNode::Character => return None,
        };
        self.commit();
        let id = self.next_free_id();
        // A new group inside a collapsed parent would be invisible. Open the
        // parent so the result of the action is visible.
        if let Some(parent) = parent
            && let Some(group) = self.outline.group_mut(parent)
        {
            group.collapsed = false;
        }
        self.outline.groups.push(Group {
            id,
            name: format!("Group {id}"),
            parent,
            collapsed: false,
        });
        self.outline.move_node(node, Some(id));
        self.outline.bump();
        self.hard_dirty = true;
        self.note = "grouped";
        Some(id)
    }

    /// Add an empty group at the scene root.
    pub(crate) fn new_group(&mut self) -> u32 {
        self.commit();
        let id = self.next_free_id();
        self.outline.groups.push(Group {
            id,
            name: format!("Group {id}"),
            parent: None,
            collapsed: false,
        });
        self.outline.bump();
        self.hard_dirty = true;
        self.note = "group added";
        id
    }

    /// Dissolve a group and select its former parent.
    pub(crate) fn ungroup_node(&mut self, node: GraphNode) -> Option<GraphNode> {
        let GraphNode::Group(id) = node else {
            return None;
        };
        if self.outline.group(id).is_none() {
            return None;
        }
        self.commit();
        let parent = self.outline.ungroup(id)?;
        self.hard_dirty = true;
        self.note = "ungrouped";
        Some(parent.map(GraphNode::Group).unwrap_or(GraphNode::Root))
    }

    /// Reparent a piece or group. Commits only when the move is valid.
    pub(crate) fn move_node(&mut self, node: GraphNode, parent: Option<u32>) -> bool {
        match node {
            GraphNode::Piece(id) if self.piece(id).is_some() => {}
            GraphNode::Group(id) if self.outline.group(id).is_some() => {
                if let Some(parent) = parent
                    && self.outline.is_descendant(parent, id)
                {
                    return false;
                }
            }
            _ => return false,
        }
        if parent.is_some_and(|parent| self.outline.group(parent).is_none()) {
            return false;
        }
        let current = match node {
            GraphNode::Piece(id) => self.outline.piece_parent(id),
            GraphNode::Group(id) => self.outline.group(id).and_then(|group| group.parent),
            _ => return false,
        };
        if current == parent {
            return false;
        }
        self.commit();
        let moved = self.outline.move_node(node, parent);
        if moved {
            self.hard_dirty = true;
            self.note = "moved";
        }
        moved
    }

    /// Rename a piece or group. An empty piece label removes the custom name
    /// and falls back to the shape name. The caller owns undo grouping.
    pub(crate) fn rename_node(&mut self, node: GraphNode, label: &str) -> bool {
        let label = clean_label(label);
        match node {
            GraphNode::Piece(id) => {
                if self.piece(id).is_none() {
                    return false;
                }
                let current = self.outline.label(id).unwrap_or_default().to_owned();
                if current == label {
                    return false;
                }
                self.outline.set_label(id, &label);
            }
            GraphNode::Group(id) => {
                let Some(group) = self.outline.group_mut(id) else {
                    return false;
                };
                if group.name == label {
                    return false;
                }
                group.name = label;
                self.outline.bump();
            }
            GraphNode::Root | GraphNode::Character => return false,
        }
        self.hard_dirty = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(id: u32, shape: &str) -> Piece {
        Piece {
            id,
            shape: model::shape_index(shape).unwrap(),
            pos: Vec2::ZERO,
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        }
    }

    fn group(id: u32, name: &str, parent: Option<u32>) -> Group {
        Group {
            id,
            name: name.into(),
            parent,
            collapsed: false,
        }
    }

    #[test]
    fn rows_follow_group_membership_and_id_order() {
        let pieces = vec![piece(1, "wall"), piece(2, "crate"), piece(3, "block")];
        let mut outline = SceneOutline::default();
        outline.groups.push(group(10, "Room A", None));
        outline.groups.push(group(11, "Props", Some(10)));
        outline.membership.insert(2, 11);
        outline.membership.insert(1, 10);

        let rows = outline.rows(&pieces, true);
        let summary: Vec<(GraphNode, usize)> =
            rows.iter().map(|row| (row.node, row.depth)).collect();
        assert_eq!(
            summary,
            vec![
                (GraphNode::Group(10), 0),
                (GraphNode::Group(11), 1),
                (GraphNode::Piece(2), 2),
                (GraphNode::Piece(1), 1),
                (GraphNode::Piece(3), 0),
                (GraphNode::Character, 0),
            ]
        );
    }

    #[test]
    fn a_collapsed_group_hides_its_children_but_keeps_the_group_row() {
        let pieces = vec![piece(1, "wall")];
        let mut outline = SceneOutline::default();
        outline.groups.push(group(5, "Walls", None));
        outline.membership.insert(1, 5);
        outline.groups[0].collapsed = true;

        let rows = outline.rows(&pieces, false);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].node, GraphNode::Group(5));
        assert!(rows[0].has_children);
        assert!(rows[0].collapsed);
        assert_eq!(rows[0].child_count, 1);
    }

    #[test]
    fn a_group_cannot_be_dropped_into_its_own_subtree() {
        let mut outline = SceneOutline::default();
        outline.groups.push(group(1, "Root", None));
        outline.groups.push(group(2, "Child", Some(1)));

        assert!(!outline.move_node(GraphNode::Group(1), Some(2)));
        assert!(!outline.move_node(GraphNode::Group(1), Some(1)));
        assert!(outline.move_node(GraphNode::Group(2), None));
        assert_eq!(outline.group(2).unwrap().parent, None);
    }

    #[test]
    fn ungrouping_promotes_children_to_the_grandparent() {
        let mut outline = SceneOutline::default();
        outline.groups.push(group(1, "Outer", None));
        outline.groups.push(group(2, "Inner", Some(1)));
        outline.groups.push(group(3, "Leaf", Some(2)));
        outline.membership.insert(7, 2);

        let parent = outline.ungroup(2).unwrap();
        assert_eq!(parent, Some(1));
        assert_eq!(outline.group(3).unwrap().parent, Some(1));
        assert_eq!(outline.membership.get(&7), Some(&1));
        assert!(outline.group(2).is_none());
    }

    #[test]
    fn orphaned_membership_and_labels_are_pruned() {
        let pieces = vec![piece(1, "wall")];
        let mut outline = SceneOutline::default();
        outline.groups.push(group(5, "Group", None));
        outline.membership.insert(1, 5);
        outline.membership.insert(9, 5);
        outline.labels.insert(1, "North wall".into());
        outline.labels.insert(9, "Ghost".into());

        assert!(outline.prune(&pieces));
        assert_eq!(outline.membership.len(), 1);
        assert_eq!(outline.labels.len(), 1);
        assert!(!outline.prune(&pieces));
    }

    #[test]
    fn grouping_a_piece_creates_a_group_and_undo_restores_the_outline() {
        let mut editor = Editor::default();
        editor.pieces.push(piece(1, "wall"));

        let id = editor.group_node(GraphNode::Piece(1)).unwrap();
        assert_eq!(editor.outline.piece_parent(1), Some(id));
        assert_eq!(editor.next_id, id + 1);
        assert_eq!(editor.note, "grouped");

        editor.undo();
        assert!(editor.outline.groups.is_empty());
        assert_eq!(editor.outline.piece_parent(1), None);
        assert_eq!(editor.pieces.len(), 1);

        editor.redo();
        assert_eq!(editor.outline.piece_parent(1), Some(id));
    }

    #[test]
    fn grouping_a_group_nests_it_and_keeps_its_children() {
        let mut editor = Editor::default();
        editor.pieces.push(piece(1, "wall"));
        let inner = editor.group_node(GraphNode::Piece(1)).unwrap();

        let outer = editor.group_node(GraphNode::Group(inner)).unwrap();
        assert_eq!(editor.outline.group(inner).unwrap().parent, Some(outer));
        assert_eq!(editor.outline.piece_parent(1), Some(inner));
    }

    #[test]
    fn moving_and_renaming_are_undoable() {
        let mut editor = Editor::default();
        editor.pieces.push(piece(1, "wall"));
        editor.pieces.push(piece(2, "crate"));
        let group = editor.new_group();

        assert!(editor.move_node(GraphNode::Piece(1), Some(group)));
        editor.commit();
        assert!(editor.rename_node(GraphNode::Piece(1), "North wall"));
        assert_eq!(editor.node_label(GraphNode::Piece(1)), "North wall");
        assert_eq!(
            editor.node_suffix(GraphNode::Piece(1)).as_deref(),
            Some("#1")
        );

        editor.undo();
        assert_eq!(editor.node_label(GraphNode::Piece(1)), "Wall");
        editor.undo();
        assert_eq!(editor.outline.piece_parent(1), None);
        editor.undo();
        assert!(editor.outline.groups.is_empty());
    }

    #[test]
    fn ungrouping_selects_the_former_parent() {
        let mut editor = Editor::default();
        editor.pieces.push(piece(1, "wall"));
        let inner = editor.group_node(GraphNode::Piece(1)).unwrap();
        let outer = editor.group_node(GraphNode::Group(inner)).unwrap();

        assert_eq!(
            editor.ungroup_node(GraphNode::Group(inner)),
            Some(GraphNode::Group(outer))
        );
        assert_eq!(editor.outline.piece_parent(1), Some(outer));
        assert_eq!(
            editor.ungroup_node(GraphNode::Group(outer)),
            Some(GraphNode::Root)
        );
    }

    #[test]
    fn labels_are_cleaned_and_limited() {
        assert_eq!(clean_label("  North\r\nwall  "), "Northwall");
        let long = "x".repeat(LABEL_LIMIT + 20);
        assert_eq!(clean_label(&long).chars().count(), LABEL_LIMIT);
    }

    #[test]
    fn the_character_is_listed_after_root_objects() {
        let pieces = vec![piece(1, "wall")];
        let rows = SceneOutline::default().rows(&pieces, true);
        assert_eq!(rows.last().unwrap().node, GraphNode::Character);
    }

    #[test]
    fn deleting_a_grouped_piece_prunes_its_metadata_and_undo_restores_it() {
        let mut editor = Editor::default();
        editor.pieces.push(piece(1, "wall"));
        let group = editor.group_node(GraphNode::Piece(1)).unwrap();
        editor.commit();
        editor.rename_node(GraphNode::Piece(1), "North wall");
        editor.selected = Some(1);

        delete_piece(&mut editor);
        assert!(editor.pieces.is_empty());
        // Rows prune metadata lazily; deleting removes the piece's outline
        // entries so no ghost node can come back.
        assert!(editor.outline.prune(&editor.pieces));
        assert_eq!(editor.outline.piece_parent(1), None);
        assert!(editor.outline.label(1).is_none());

        editor.undo();
        assert_eq!(editor.pieces.len(), 1);
        assert_eq!(editor.outline.piece_parent(1), Some(group));
        assert_eq!(editor.outline.label(1), Some("North wall"));
    }

    #[test]
    fn viewport_locking_is_undoable_and_prunes_with_the_piece() {
        let mut editor = Editor::default();
        editor.pieces.push(piece(1, "wall"));

        assert!(editor.toggle_lock(1));
        assert!(editor.outline.is_locked(1));
        assert_eq!(editor.note, "locked");
        assert_eq!(editor.undo.len(), 1);

        editor.undo();
        assert!(!editor.outline.is_locked(1));
        editor.redo();
        assert!(editor.outline.is_locked(1));

        editor.selected = Some(1);
        delete_piece(&mut editor);
        assert!(editor.outline.prune(&editor.pieces));
        assert!(!editor.outline.is_locked(1));
    }

    #[test]
    fn toggling_a_lock_twice_restores_the_open_state() {
        let mut editor = Editor::default();
        editor.pieces.push(piece(1, "crate"));
        assert!(editor.toggle_lock(1));
        assert!(!editor.toggle_lock(1));
        assert!(!editor.outline.is_locked(1));
        assert_eq!(editor.note, "unlocked");
    }
}
