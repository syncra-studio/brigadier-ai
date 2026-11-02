use super::bindings::AXUIElementRef;
use super::tree::AXNode;
use core_foundation::base::{CFEqual, CFRelease, CFRetain, CFTypeRef};
use cua_driver_core::snapshot_store::{SnapshotPayload, SnapshotStore};

pub struct RetainedElement(usize);

impl RetainedElement {
    pub fn as_ptr(&self) -> usize {
        self.0
    }

    /// Take a +1 reference on an AX element pointer (0 is kept as null).
    ///
    /// # Safety
    ///
    /// A nonzero `ptr` must be a live `AXUIElementRef`.
    pub unsafe fn retain(ptr: usize) -> Self {
        if ptr != 0 {
            unsafe { CFRetain(ptr as AXUIElementRef as CFTypeRef) };
        }
        Self(ptr)
    }
}

impl Clone for RetainedElement {
    fn clone(&self) -> Self {
        unsafe { Self::retain(self.0) }
    }
}

impl Drop for RetainedElement {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe { CFRelease(self.0 as AXUIElementRef as CFTypeRef) };
        }
    }
}

pub struct AxSnapshot {
    pub elements: Vec<usize>,
    /// Per row, a hash of what the read saw: role, title, value, description,
    /// identifier, depth, place among its siblings and its parent's hash.
    /// `None` when the role was unreadable, so the row is never carried.
    pub identities: Vec<Option<u64>>,
}

impl AxSnapshot {
    pub fn from_nodes(nodes: &[AXNode]) -> Self {
        use std::collections::HashMap;
        use std::hash::{Hash, Hasher};
        let mut by_index: HashMap<usize, Option<u64>> = HashMap::new();
        let mut siblings: HashMap<Option<usize>, usize> = HashMap::new();
        let mut elements = Vec::new();
        let mut identities = Vec::new();
        for node in nodes {
            let Some(index) = node.element_index else {
                continue;
            };
            let place = siblings.entry(node.parent_element_index).or_default();
            let parent = match node.parent_element_index {
                Some(parent) => by_index.get(&parent).copied().flatten(),
                None => Some(0),
            };
            let identity = parent.filter(|_| !node.role.is_empty()).map(|parent| {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                (
                    &node.role,
                    &node.title,
                    &node.value,
                    &node.description,
                    &node.identifier,
                    node.depth,
                    *place,
                    parent,
                )
                    .hash(&mut hasher);
                hasher.finish()
            });
            *place += 1;
            by_index.insert(index, identity);
            elements.push(node.element_ptr);
            identities.push(identity);
        }
        Self {
            elements,
            identities,
        }
    }
}

impl SnapshotPayload for AxSnapshot {
    type Element = RetainedElement;
    fn len(&self) -> usize {
        self.elements.len()
    }
    fn retain(&self, index: usize) -> Option<RetainedElement> {
        self.elements
            .get(index)
            .map(|ptr| unsafe { RetainedElement::retain(*ptr) })
    }

    const CARRIES_ROWS: bool = true;

    /// The one row of this read that is the same AX element (CFEqual) with
    /// the same identity hash as `older`'s row: unchanged content, same place
    /// under the same unchanged parents. Anything else stays stale.
    fn carried_row(&self, older: &Self, row: usize) -> Option<usize> {
        let identity = (*older.identities.get(row)?)?;
        let element = *older.elements.get(row)?;
        if element == 0 {
            return None;
        }
        let mut matches = self.identities.iter().enumerate().filter(|&(index, id)| {
            *id == Some(identity) && {
                let now = self.elements[index];
                now != 0 && unsafe { CFEqual(now as CFTypeRef, element as CFTypeRef) != 0 }
            }
        });
        let (found, _) = matches.next()?;
        matches.next().is_none().then_some(found)
    }
}

impl Drop for AxSnapshot {
    fn drop(&mut self) {
        for ptr in &self.elements {
            if *ptr != 0 {
                unsafe { CFRelease(*ptr as AXUIElementRef as CFTypeRef) };
            }
        }
    }
}

pub type Snapshots = SnapshotStore<AxSnapshot>;

#[cfg(test)]
mod tests {
    use super::*;
    use core_foundation::base::{CFGetRetainCount, TCFType};
    use core_foundation::string::CFString;
    use cua_driver_core::element_token::{token_for, ResolvedElement};

    fn resolve(cache: &Snapshots, snapshot: u32, index: usize) -> Option<RetainedElement> {
        match cache
            .resolve(
                1,
                &serde_json::json!({ "element_token": token_for(snapshot, index) }),
            )
            .ok()?
        {
            ResolvedElement::Element { element, .. } => Some(element),
            _ => None,
        }
    }

    fn payload(ptr: usize) -> AxSnapshot {
        unsafe { CFRetain(ptr as CFTypeRef) };
        AxSnapshot {
            elements: vec![ptr],
            identities: vec![None],
        }
    }

    #[test]
    fn retained_element_survives_concurrent_snapshot_replace() {
        let value = CFString::new("cua-driver-uaf-test-element-placeholder");
        let ptr = value.as_concrete_TypeRef() as usize;
        let base = unsafe { CFGetRetainCount(ptr as CFTypeRef) };
        let cache = Snapshots::new();
        let snapshot = cache.publish(1, 2, payload(ptr));
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base + 1);
        let guard = resolve(&cache, snapshot, 0).unwrap();
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base + 2);
        cache.publish(1, 2, AxSnapshot::from_nodes(&[]));
        assert!(resolve(&cache, snapshot, 0).is_none());
        // The replaced read is kept (for unchanged rows) until it ages out.
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base + 2);
        cache.clear();
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base + 1);
        drop(guard);
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base);
    }

    #[test]
    fn admitted_element_survives_cache_destruction_until_native_work_finishes() {
        let value = CFString::new("cua-driver-invariant-admitted-native-work");
        let ptr = value.as_concrete_TypeRef() as usize;
        let base = unsafe { CFGetRetainCount(ptr as CFTypeRef) };
        let cache = Snapshots::new();
        let snapshot = cache.publish(1, 2, payload(ptr));
        let guard = resolve(&cache, snapshot, 0).unwrap();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            finish_rx.recv().unwrap();
            assert_eq!(guard.as_ptr(), ptr);
            drop(guard);
        });
        drop(cache);
        let retained = unsafe { CFGetRetainCount(ptr as CFTypeRef) };
        finish_tx.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(retained, base + 1);
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base);
    }

    #[test]
    fn missing_index_returns_none() {
        let cache = Snapshots::new();
        assert!(resolve(&cache, 0, 0).is_none());
        let snapshot = cache.publish(1, 2, AxSnapshot::from_nodes(&[]));
        assert!(resolve(&cache, snapshot, 0).is_none());
        assert!(resolve(&cache, snapshot, 5).is_none());
    }

    #[test]
    fn abandoned_preparation_releases_native_payload_without_replacing_snapshot() {
        let original = CFString::new("cua-driver-original-published-native-work");
        let replacement = CFString::new("cua-driver-abandoned-prepared-native-work");
        let original_ptr = original.as_concrete_TypeRef() as usize;
        let replacement_ptr = replacement.as_concrete_TypeRef() as usize;
        let base = unsafe { CFGetRetainCount(replacement_ptr as CFTypeRef) };
        let cache = Snapshots::new();
        let snapshot = cache.publish(1, 2, payload(original_ptr));
        let prepared = payload(replacement_ptr);
        assert_eq!(resolve(&cache, snapshot, 0).unwrap().as_ptr(), original_ptr);
        drop(prepared);
        assert_eq!(
            unsafe { CFGetRetainCount(replacement_ptr as CFTypeRef) },
            base
        );
        assert_eq!(resolve(&cache, snapshot, 0).unwrap().as_ptr(), original_ptr);
    }

    /// One indexed row: (element, role, title, value, parent row).
    type Row<'a> = (&'a CFString, &'a str, &'a str, &'a str, Option<usize>);

    fn read(rows: &[Row]) -> AxSnapshot {
        let nodes: Vec<AXNode> = rows
            .iter()
            .enumerate()
            .map(|(index, (element, role, title, value, parent))| {
                let ptr = element.as_concrete_TypeRef() as usize;
                unsafe { CFRetain(ptr as CFTypeRef) };
                AXNode {
                    element_index: Some(index),
                    role: (*role).into(),
                    title: Some((*title).into()),
                    value: Some((*value).into()),
                    description: None,
                    identifier: None,
                    help: None,
                    actions: Vec::new(),
                    element_ptr: ptr,
                    depth: usize::from(parent.is_some()),
                    parent_element_index: *parent,
                    frame: None,
                    value_state: None,
                    value_description: None,
                    min_value: None,
                    max_value: None,
                    enabled: None,
                    selected: None,
                    in_web_content: false,
                }
            })
            .collect();
        AxSnapshot::from_nodes(&nodes)
    }

    fn resolved_row(cache: &Snapshots, snapshot: u32, index: usize) -> Option<(usize, usize)> {
        match cache
            .resolve(
                1,
                &serde_json::json!({ "element_token": token_for(snapshot, index) }),
            )
            .ok()?
        {
            ResolvedElement::Element {
                element_index,
                element,
                ..
            } => Some((element_index, element.as_ptr())),
            _ => None,
        }
    }

    fn elements() -> [CFString; 4] {
        ["w05-window", "w05-row-a", "w05-row-b", "w05-field"].map(CFString::new)
    }

    #[test]
    fn an_unchanged_element_keeps_its_token_across_reads() {
        let [window, a, b, field] = elements();
        let cache = Snapshots::new();
        let first = cache.publish(
            1,
            2,
            read(&[
                (&window, "AXWindow", "Doc", "", None),
                (&a, "AXRow", "a", "", Some(0)),
            ]),
        );
        // A new element after it changes nothing about row [1].
        cache.publish(
            1,
            2,
            read(&[
                (&window, "AXWindow", "Doc", "", None),
                (&a, "AXRow", "a", "", Some(0)),
                (&b, "AXRow", "b", "", Some(0)),
                (&field, "AXTextField", "", "x", Some(0)),
            ]),
        );
        let a_ptr = a.as_concrete_TypeRef() as usize;
        assert_eq!(resolved_row(&cache, first, 1), Some((1, a_ptr)));
    }

    #[test]
    fn a_changed_value_stales_the_token() {
        let [window, _, _, field] = elements();
        let cache = Snapshots::new();
        let rows = |value| {
            read(&[
                (&window, "AXWindow", "Doc", "", None),
                (&field, "AXTextField", "", value, Some(0)),
            ])
        };
        let first = cache.publish(1, 2, rows("before"));
        cache.publish(1, 2, rows("after"));
        assert_eq!(resolved_row(&cache, first, 1), None);
        // The window row itself did not change and still resolves.
        assert!(resolved_row(&cache, first, 0).is_some());
    }

    #[test]
    fn the_same_object_with_new_content_is_stale() {
        let [window, a, _, _] = elements();
        let cache = Snapshots::new();
        let first = cache.publish(
            1,
            2,
            read(&[
                (&window, "AXWindow", "Doc", "", None),
                (&a, "AXRow", "a", "", Some(0)),
            ]),
        );
        cache.publish(
            1,
            2,
            read(&[
                (&window, "AXWindow", "Doc", "", None),
                (&a, "AXRow", "renamed", "", Some(0)),
            ]),
        );
        assert_eq!(resolved_row(&cache, first, 1), None);
    }

    #[test]
    fn a_moved_row_is_stale() {
        let [window, a, b, _] = elements();
        let cache = Snapshots::new();
        let first = cache.publish(
            1,
            2,
            read(&[
                (&window, "AXWindow", "Doc", "", None),
                (&a, "AXRow", "a", "", Some(0)),
                (&b, "AXRow", "b", "", Some(0)),
            ]),
        );
        cache.publish(
            1,
            2,
            read(&[
                (&window, "AXWindow", "Doc", "", None),
                (&b, "AXRow", "b", "", Some(0)),
                (&a, "AXRow", "a", "", Some(0)),
            ]),
        );
        assert_eq!(resolved_row(&cache, first, 1), None);
        assert_eq!(resolved_row(&cache, first, 2), None);
    }

    #[test]
    fn a_removed_element_is_stale() {
        let [window, a, _, _] = elements();
        let cache = Snapshots::new();
        let first = cache.publish(
            1,
            2,
            read(&[
                (&window, "AXWindow", "Doc", "", None),
                (&a, "AXRow", "a", "", Some(0)),
            ]),
        );
        cache.publish(1, 2, read(&[(&window, "AXWindow", "Doc", "", None)]));
        assert_eq!(resolved_row(&cache, first, 1), None);
    }

    #[test]
    fn tokens_reach_back_only_four_reads() {
        let [window, _, _, _] = elements();
        let cache = Snapshots::new();
        let publish = || cache.publish(1, 2, read(&[(&window, "AXWindow", "Doc", "", None)]));
        let first = publish();
        let second = publish();
        publish();
        publish();
        assert!(resolved_row(&cache, first, 0).is_some());
        publish();
        assert_eq!(resolved_row(&cache, first, 0), None);
        assert!(resolved_row(&cache, second, 0).is_some());
        // Removing the window drops its kept reads too.
        cache.remove(1, 2);
        assert_eq!(resolved_row(&cache, second, 0), None);
    }
}
