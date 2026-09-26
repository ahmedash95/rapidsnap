//! Command-pattern undo/redo history.
//!
//! UI code never mutates a [`Document`] directly; it builds a [`Command`] and
//! calls [`History::apply`]. `History` stores, for every applied command, the
//! exact inverse command, so undo/redo are exact.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Annotation, AnnotationId, BeautifySettings, Document, ImageTransform};

/// A document mutation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// Append annotations on top (in order). Ids must be new.
    Add(Vec<Annotation>),
    /// Insert annotations at explicit indices. Applied in ascending index
    /// order, each index being the final position (used as the inverse of
    /// `Remove`; also usable for paste-at-position).
    Insert(Vec<(usize, Annotation)>),
    /// Remove annotations by id (multi-select delete).
    Remove(Vec<AnnotationId>),
    /// Replace an annotation (move / resize / restyle / text edit).
    /// `before` must equal the current value (checked by id only);
    /// `after.id` must equal `before.id`.
    Update { before: Box<Annotation>, after: Box<Annotation> },
    /// Change z-order: full id order bottom→top before and after. Both must be
    /// permutations of the current ids. Build with [`zorder`] helpers.
    Reorder { before: Vec<AnnotationId>, after: Vec<AnnotationId> },
    /// Replace the beautify settings.
    SetBeautify { before: Box<BeautifySettings>, after: Box<BeautifySettings> },
    /// Change crop/rotation. Applying also remaps every annotation so it stays
    /// on the same image pixels (see `document::remap_annotations`).
    SetImageTransform { before: ImageTransform, after: ImageTransform },
    /// Several commands as one undo step (applied in order, undone in reverse).
    Batch(Vec<Command>),
}

impl Command {
    /// Convenience constructor for `Update`.
    pub fn update(before: Annotation, after: Annotation) -> Self {
        Command::Update { before: Box::new(before), after: Box::new(after) }
    }

    pub fn set_beautify(before: BeautifySettings, after: BeautifySettings) -> Self {
        Command::SetBeautify { before: Box::new(before), after: Box::new(after) }
    }

    /// True if applying this command would change nothing (e.g. empty batch,
    /// update with identical values).
    pub fn is_noop(&self) -> bool {
        match self {
            Command::Add(v) => v.is_empty(),
            Command::Insert(v) => v.is_empty(),
            Command::Remove(v) => v.is_empty(),
            Command::Update { before, after } => before == after,
            Command::Reorder { before, after } => before == after,
            Command::SetBeautify { before, after } => before == after,
            Command::SetImageTransform { before, after } => before == after,
            Command::Batch(v) => v.iter().all(Command::is_noop),
        }
    }
}

/// Errors from applying a command. On error the document is left unchanged
/// (batches are rolled back).
#[derive(Debug, Clone, PartialEq, Error)]
pub enum HistoryError {
    #[error("annotation {0} not found")]
    NotFound(AnnotationId),
    #[error("annotation {0} already exists")]
    DuplicateId(AnnotationId),
    #[error("index {0} out of bounds")]
    IndexOutOfBounds(usize),
    #[error("reorder ids are not a permutation of the document's annotations")]
    InvalidReorder,
    #[error("update changes the annotation id")]
    IdMismatch,
}

/// Apply `cmd` to `doc` and return its exact inverse. Low-level; prefer
/// [`History::apply`]. On error `doc` is unchanged.
pub fn apply_command(doc: &mut Document, cmd: &Command) -> Result<Command, HistoryError> {
    match cmd {
        Command::Add(items) => {
            for a in items {
                if doc.index_of(a.id).is_some() {
                    return Err(HistoryError::DuplicateId(a.id));
                }
            }
            doc.annotations.extend(items.iter().cloned());
            Ok(Command::Remove(items.iter().map(|a| a.id).collect()))
        }
        Command::Insert(items) => {
            let mut sorted = items.clone();
            sorted.sort_by_key(|(i, _)| *i);
            for (_, a) in &sorted {
                if doc.index_of(a.id).is_some() {
                    return Err(HistoryError::DuplicateId(a.id));
                }
            }
            let mut next = doc.annotations.clone();
            for (i, a) in &sorted {
                if *i > next.len() {
                    return Err(HistoryError::IndexOutOfBounds(*i));
                }
                next.insert(*i, a.clone());
            }
            doc.annotations = next;
            Ok(Command::Remove(sorted.iter().map(|(_, a)| a.id).collect()))
        }
        Command::Remove(ids) => {
            let mut removed = Vec::with_capacity(ids.len());
            for id in ids {
                let i = doc.index_of(*id).ok_or(HistoryError::NotFound(*id))?;
                removed.push((i, doc.annotations[i].clone()));
            }
            // Indices are positions in the original list; ascending insertion
            // of original indices restores the original order exactly.
            removed.sort_by_key(|(i, _)| *i);
            doc.annotations.retain(|a| !ids.contains(&a.id));
            Ok(Command::Insert(removed))
        }
        Command::Update { before, after } => {
            if before.id != after.id {
                return Err(HistoryError::IdMismatch);
            }
            let slot = doc.get_mut(after.id).ok_or(HistoryError::NotFound(after.id))?;
            let current = std::mem::replace(slot, (**after).clone());
            Ok(Command::Update { before: after.clone(), after: Box::new(current) })
        }
        Command::Reorder { before: _, after } => {
            let current: Vec<AnnotationId> = doc.annotations.iter().map(|a| a.id).collect();
            if after.len() != current.len() {
                return Err(HistoryError::InvalidReorder);
            }
            let mut next = Vec::with_capacity(after.len());
            for id in after {
                let a = doc.get(*id).ok_or(HistoryError::InvalidReorder)?;
                if next.iter().any(|x: &Annotation| x.id == *id) {
                    return Err(HistoryError::InvalidReorder);
                }
                next.push(a.clone());
            }
            doc.annotations = next;
            Ok(Command::Reorder { before: after.clone(), after: current })
        }
        Command::SetBeautify { before: _, after } => {
            let current = std::mem::replace(&mut doc.beautify, (**after).clone());
            Ok(Command::SetBeautify { before: after.clone(), after: Box::new(current) })
        }
        Command::SetImageTransform { before: _, after } => {
            let current = doc.image_transform;
            crate::document::remap_annotations(
                &mut doc.annotations,
                &current,
                after,
                doc.base_image.width,
                doc.base_image.height,
            );
            doc.image_transform = *after;
            Ok(Command::SetImageTransform { before: *after, after: current })
        }
        Command::Batch(cmds) => {
            let mut inverses = Vec::with_capacity(cmds.len());
            for c in cmds {
                match apply_command(doc, c) {
                    Ok(inv) => inverses.push(inv),
                    Err(e) => {
                        // Roll back what was applied.
                        for inv in inverses.iter().rev() {
                            let _ = apply_command(doc, inv);
                        }
                        return Err(e);
                    }
                }
            }
            inverses.reverse();
            Ok(Command::Batch(inverses))
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    forward: Command,
    inverse: Command,
    coalesce_key: Option<String>,
}

/// Unlimited undo/redo stack. One `History` per open Document.
#[derive(Debug, Clone, Default)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    /// Undo-stack depth at the last save (for dirty tracking).
    saved_at: Option<usize>,
    /// Bumped on every change; lets views cheaply detect changes.
    revision: u64,
}

impl History {
    pub fn new() -> Self {
        Self { saved_at: Some(0), ..Default::default() }
    }

    /// Apply a command as a new undo step. Clears the redo stack.
    /// No-op commands are ignored (Ok, nothing recorded).
    pub fn apply(&mut self, doc: &mut Document, cmd: Command) -> Result<(), HistoryError> {
        self.apply_inner(doc, cmd, None)
    }

    /// Like [`apply`](Self::apply) but merges into the previous step if that
    /// step was applied with the same `key` and nothing was undone since.
    /// Use for continuous interactions (slider drags, arrow-key nudges,
    /// typing): e.g. key = `"opacity:<id>"`. The merged step undoes to the
    /// state before the *first* command of the run.
    pub fn apply_coalescing(
        &mut self,
        doc: &mut Document,
        cmd: Command,
        key: impl Into<String>,
    ) -> Result<(), HistoryError> {
        self.apply_inner(doc, cmd, Some(key.into()))
    }

    /// End the current coalescing run so the next `apply_coalescing` with the
    /// same key starts a new undo step (call on mouse-up / focus change).
    pub fn break_coalescing(&mut self) {
        if let Some(e) = self.undo.last_mut() {
            e.coalesce_key = None;
        }
    }

    fn apply_inner(&mut self, doc: &mut Document, cmd: Command, key: Option<String>) -> Result<(), HistoryError> {
        if cmd.is_noop() {
            return Ok(());
        }
        let inverse = apply_command(doc, &cmd)?;
        self.redo.clear();
        self.revision += 1;
        let merge = matches!((&key, self.undo.last()), (Some(k), Some(e)) if e.coalesce_key.as_ref() == Some(k))
            && self.saved_at != Some(self.undo.len());
        if merge {
            let last = self.undo.last_mut().expect("checked above");
            // forward = old forward then new; inverse = new inverse then old.
            last.forward = Command::Batch(vec![last.forward.clone(), cmd]);
            last.inverse = Command::Batch(vec![inverse, last.inverse.clone()]);
        } else {
            if let Some(s) = self.saved_at
                && s > self.undo.len() {
                    self.saved_at = None;
                }
            self.undo.push(Entry { forward: cmd, inverse, coalesce_key: key });
        }
        Ok(())
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undo the last step. Returns `Ok(false)` if there was nothing to undo.
    pub fn undo(&mut self, doc: &mut Document) -> Result<bool, HistoryError> {
        let Some(mut e) = self.undo.pop() else { return Ok(false) };
        match apply_command(doc, &e.inverse) {
            Ok(_) => {
                e.coalesce_key = None;
                self.redo.push(e);
                self.revision += 1;
                Ok(true)
            }
            Err(err) => {
                self.undo.push(e);
                Err(err)
            }
        }
    }

    /// Redo the last undone step. Returns `Ok(false)` if nothing to redo.
    pub fn redo(&mut self, doc: &mut Document) -> Result<bool, HistoryError> {
        let Some(e) = self.redo.pop() else { return Ok(false) };
        match apply_command(doc, &e.forward) {
            Ok(inv) => {
                self.undo.push(Entry { forward: e.forward, inverse: inv, coalesce_key: None });
                self.revision += 1;
                Ok(true)
            }
            Err(err) => {
                self.redo.push(e);
                Err(err)
            }
        }
    }

    /// Drop all history (e.g. after loading a project).
    pub fn clear(&mut self) {
        *self = History::new();
        self.revision = self.revision.wrapping_add(1);
    }

    /// Mark the current state as saved.
    pub fn mark_saved(&mut self) {
        self.saved_at = Some(self.undo.len());
        self.break_coalescing();
    }

    /// True if the document changed since the last `mark_saved` (or creation).
    pub fn is_dirty(&self) -> bool {
        self.saved_at != Some(self.undo.len())
    }

    /// Monotonic change counter (changes on apply/undo/redo/clear).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }
}

/// Z-order helpers. Each returns a [`Command::Reorder`] for the given
/// selection (no-op command if nothing moves). Relative order within the
/// selection is preserved.
pub mod zorder {
    use super::*;

    fn ids(doc: &Document) -> Vec<AnnotationId> {
        doc.annotations.iter().map(|a| a.id).collect()
    }

    fn reorder(doc: &Document, after: Vec<AnnotationId>) -> Command {
        Command::Reorder { before: ids(doc), after }
    }

    /// Move selection to the top.
    pub fn bring_to_front(doc: &Document, sel: &[AnnotationId]) -> Command {
        let (mut rest, picked): (Vec<_>, Vec<_>) = ids(doc).into_iter().partition(|i| !sel.contains(i));
        rest.extend(picked);
        reorder(doc, rest)
    }

    /// Move selection to the bottom.
    pub fn send_to_back(doc: &Document, sel: &[AnnotationId]) -> Command {
        let (rest, mut picked): (Vec<_>, Vec<_>) = ids(doc).into_iter().partition(|i| !sel.contains(i));
        picked.extend(rest);
        reorder(doc, picked)
    }

    /// Move each selected item one step up (past one unselected item).
    pub fn bring_forward(doc: &Document, sel: &[AnnotationId]) -> Command {
        let mut v = ids(doc);
        for i in (0..v.len().saturating_sub(1)).rev() {
            if sel.contains(&v[i]) && !sel.contains(&v[i + 1]) {
                v.swap(i, i + 1);
            }
        }
        reorder(doc, v)
    }

    /// Move each selected item one step down.
    pub fn send_backward(doc: &Document, sel: &[AnnotationId]) -> Command {
        let mut v = ids(doc);
        for i in 1..v.len() {
            if sel.contains(&v[i]) && !sel.contains(&v[i - 1]) {
                v.swap(i, i - 1);
            }
        }
        reorder(doc, v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn doc() -> Document {
        Document::new(RgbaImage::filled(100, 50, [255, 255, 255, 255]), DocumentMetadata::default())
    }

    fn rect(x: f32) -> Annotation {
        ToolStyle::default().create(ToolKind::Rect, Point::new(x, 0.0), Point::new(x + 10.0, 10.0)).unwrap()
    }

    #[test]
    fn add_undo_redo() {
        let mut d = doc();
        let mut h = History::new();
        assert!(!h.can_undo() && !h.can_redo() && !h.is_dirty());
        let a = rect(0.0);
        h.apply(&mut d, Command::Add(vec![a.clone()])).unwrap();
        assert_eq!(d.annotations.len(), 1);
        assert!(h.can_undo() && h.is_dirty());
        assert!(h.undo(&mut d).unwrap());
        assert!(d.annotations.is_empty());
        assert!(h.can_redo() && !h.is_dirty());
        assert!(h.redo(&mut d).unwrap());
        assert_eq!(d.annotations, vec![a]);
        // Nothing to redo / undo on a fresh history.
        let mut fresh = History::new();
        assert!(!fresh.redo(&mut d).unwrap());
        assert!(!fresh.undo(&mut d).unwrap());
    }

    #[test]
    fn remove_multi_restores_order() {
        let mut d = doc();
        let mut h = History::new();
        let items: Vec<_> = (0..5).map(|i| rect(i as f32 * 10.0)).collect();
        h.apply(&mut d, Command::Add(items.clone())).unwrap();
        h.apply(&mut d, Command::Remove(vec![items[3].id, items[0].id, items[2].id])).unwrap();
        assert_eq!(d.annotations, vec![items[1].clone(), items[4].clone()]);
        h.undo(&mut d).unwrap();
        assert_eq!(d.annotations, items);
        h.redo(&mut d).unwrap();
        assert_eq!(d.annotations.len(), 2);
    }

    #[test]
    fn update_and_coalescing() {
        let mut d = doc();
        let mut h = History::new();
        let a = rect(0.0);
        h.apply(&mut d, Command::Add(vec![a.clone()])).unwrap();
        let mut cur = a.clone();
        for i in 1..=5 {
            let mut next = cur.clone();
            next.translate(1.0, 0.0);
            h.apply_coalescing(&mut d, Command::update(cur.clone(), next.clone()), "move").unwrap();
            cur = next;
            assert_eq!(d.annotations[0].geometric_bounds().x, i as f32);
        }
        assert_eq!(h.undo_len(), 2);
        assert_eq!(d.annotations[0].geometric_bounds().x, 5.0);
        h.undo(&mut d).unwrap();
        assert_eq!(d.annotations[0], a);
        h.redo(&mut d).unwrap();
        assert_eq!(d.annotations[0].geometric_bounds().x, 5.0);
        // Break → a new step.
        h.break_coalescing();
        let mut next = cur.clone();
        next.opacity = 0.5;
        h.apply_coalescing(&mut d, Command::update(cur.clone(), next), "move").unwrap();
        assert_eq!(h.undo_len(), 3);
    }

    #[test]
    fn update_id_mismatch_and_not_found() {
        let mut d = doc();
        let mut h = History::new();
        let a = rect(0.0);
        let b = rect(1.0);
        assert_eq!(h.apply(&mut d, Command::update(a.clone(), b.clone())), Err(HistoryError::IdMismatch));
        let mut a2 = a.clone();
        a2.opacity = 0.3;
        assert_eq!(h.apply(&mut d, Command::update(a.clone(), a2)), Err(HistoryError::NotFound(a.id)));
        assert!(!h.can_undo());
    }

    #[test]
    fn batch_rolls_back_on_error() {
        let mut d = doc();
        let mut h = History::new();
        let a = rect(0.0);
        let bad = rect(1.0);
        let res = h.apply(
            &mut d,
            Command::Batch(vec![Command::Add(vec![a.clone()]), Command::Remove(vec![bad.id])]),
        );
        assert!(res.is_err());
        assert!(d.annotations.is_empty());
        h.apply(
            &mut d,
            Command::Batch(vec![Command::Add(vec![a.clone()]), Command::Add(vec![bad.clone()])]),
        )
        .unwrap();
        assert_eq!(d.annotations.len(), 2);
        h.undo(&mut d).unwrap();
        assert!(d.annotations.is_empty());
    }

    #[test]
    fn zorder_ops() {
        let mut d = doc();
        let mut h = History::new();
        let items: Vec<_> = (0..4).map(|i| rect(i as f32)).collect();
        let ids: Vec<_> = items.iter().map(|a| a.id).collect();
        h.apply(&mut d, Command::Add(items)).unwrap();
        let order = |d: &Document| d.annotations.iter().map(|a| a.id).collect::<Vec<_>>();

        let c = zorder::bring_to_front(&d, &[ids[0]]);
        h.apply(&mut d, c).unwrap();
        assert_eq!(order(&d), vec![ids[1], ids[2], ids[3], ids[0]]);
        let c = zorder::send_to_back(&d, &[ids[0]]);
        h.apply(&mut d, c).unwrap();
        assert_eq!(order(&d), ids);
        let c = zorder::bring_forward(&d, &[ids[1]]);
        h.apply(&mut d, c).unwrap();
        assert_eq!(order(&d), vec![ids[0], ids[2], ids[1], ids[3]]);
        let c = zorder::send_backward(&d, &[ids[1]]);
        h.apply(&mut d, c).unwrap();
        assert_eq!(order(&d), ids);
        // Already at top → noop, not recorded.
        let before = h.undo_len();
        let c = zorder::bring_to_front(&d, &[ids[3]]);
        h.apply(&mut d, c).unwrap();
        assert_eq!(h.undo_len(), before);
        h.undo(&mut d).unwrap();
        assert_eq!(order(&d), vec![ids[0], ids[2], ids[1], ids[3]]);
    }

    #[test]
    fn beautify_and_transform() {
        let mut d = doc();
        let mut h = History::new();
        let a = rect(10.0);
        h.apply(&mut d, Command::Add(vec![a.clone()])).unwrap();
        let mut b = d.beautify.clone();
        b.corner_radius = 3.0;
        let c = Command::set_beautify(d.beautify.clone(), b.clone());
        h.apply(&mut d, c).unwrap();
        assert_eq!(d.beautify, b);

        let t = ImageTransform { crop: Some(IRect::new(5, 0, 50, 50)), rotation: Rotation::Deg90 };
        let c = Command::SetImageTransform { before: d.image_transform, after: t };
        h.apply(&mut d, c).unwrap();
        assert_eq!(d.content_size(), Size::new(50.0, 50.0));
        assert_ne!(d.annotations[0], a);
        h.undo(&mut d).unwrap();
        let r = d.annotations[0].geometric_bounds();
        let r0 = a.geometric_bounds();
        assert!((r.x - r0.x).abs() < 1e-3 && (r.y - r0.y).abs() < 1e-3 && (r.width - r0.width).abs() < 1e-3);
        h.undo(&mut d).unwrap();
        assert_eq!(d.beautify, BeautifySettings::default());
        assert_eq!(h.undo_len(), 1);
    }

    #[test]
    fn dirty_tracking() {
        let mut d = doc();
        let mut h = History::new();
        h.apply(&mut d, Command::Add(vec![rect(0.0)])).unwrap();
        h.mark_saved();
        assert!(!h.is_dirty());
        h.undo(&mut d).unwrap();
        assert!(h.is_dirty());
        h.redo(&mut d).unwrap();
        assert!(!h.is_dirty());
        h.undo(&mut d).unwrap();
        h.apply(&mut d, Command::Add(vec![rect(1.0)])).unwrap();
        assert!(h.is_dirty());
    }
}
