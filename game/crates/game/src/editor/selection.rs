//! The editor's multi-piece selection.

/// The pieces the editor has selected, in selection order.
///
/// The first id is the *primary* piece: it drives the properties panel, the
/// transform gizmo and the motion/route tools, because those all address one
/// object. Every other selected piece rides along with moves, rotations,
/// deletes and copies, which are the actions that read the whole set.
///
/// An empty selection has no primary, and any primary is always one of the
/// selected ids.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Selection {
    ids: Vec<u32>,
}

impl Selection {
    pub(super) fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub(super) fn len(&self) -> usize {
        self.ids.len()
    }

    pub(super) fn ids(&self) -> &[u32] {
        &self.ids
    }

    /// The piece the single-object tools address.
    pub(super) fn primary(&self) -> Option<u32> {
        self.ids.first().copied()
    }

    pub(super) fn is_primary(&self, id: u32) -> bool {
        self.ids.first() == Some(&id)
    }

    pub(super) fn contains(&self, id: u32) -> bool {
        self.ids.contains(&id)
    }

    /// Replace the selection with one piece.
    pub(super) fn set(&mut self, id: u32) {
        self.ids.clear();
        self.ids.push(id);
    }

    /// Replace the selection with `ids`, keeping their order: the first one
    /// becomes the primary. Duplicates collapse.
    pub(super) fn set_all(&mut self, ids: impl IntoIterator<Item = u32>) {
        self.ids.clear();
        for id in ids {
            if !self.ids.contains(&id) {
                self.ids.push(id);
            }
        }
    }

    /// Shift-click a piece: drop it from the selection when it is there
    /// already, otherwise make it the new primary.
    pub(super) fn toggle(&mut self, id: u32) {
        if self.contains(id) {
            self.ids.retain(|&kept| kept != id);
        } else {
            self.ids.insert(0, id);
        }
    }

    pub(super) fn clear(&mut self) {
        self.ids.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::Selection;

    #[test]
    fn an_empty_selection_has_no_primary() {
        let selection = Selection::default();
        assert!(selection.is_empty());
        assert_eq!(selection.len(), 0);
        assert_eq!(selection.primary(), None);
    }

    #[test]
    fn setting_one_piece_replaces_the_selection() {
        let mut selection = Selection::default();
        selection.set_all([1, 2, 3]);
        selection.set(9);
        assert_eq!(selection.ids(), [9]);
        assert!(selection.is_primary(9));
    }

    #[test]
    fn set_all_keeps_the_given_order_and_drops_duplicates() {
        let mut selection = Selection::default();
        selection.set_all([4, 2, 4, 7]);
        assert_eq!(selection.ids(), [4, 2, 7]);
        assert_eq!(selection.primary(), Some(4));
    }

    #[test]
    fn toggling_adds_then_removes_a_piece() {
        let mut selection = Selection::default();
        selection.set(1);
        selection.toggle(2);
        assert_eq!(selection.ids(), [2, 1], "the new piece becomes primary");
        assert_eq!(selection.primary(), Some(2));
        selection.toggle(2);
        assert_eq!(selection.ids(), [1]);
        assert_eq!(selection.primary(), Some(1));
        selection.toggle(1);
        assert!(selection.is_empty(), "removing the last piece clears it");
        assert_eq!(selection.primary(), None);
    }
}
