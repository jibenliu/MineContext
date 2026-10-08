//! 实体类别的**存储名**必须稳定。
//!
//! `activity_entities.kind` 列里存的是字符串。这个字符串一旦改名，
//! 旧的实体关联就会全部查不出来（而且不会报错，只是「线索变少了」）。
//! 因此它和错误码一样，是一份对外契约。

use mc_memory::entity::EntityKind;

#[test]
fn entity_kind_storage_names_are_stable() {
    assert_eq!(EntityKind::Issue.as_str(), "issue");
    assert_eq!(EntityKind::FileNote.as_str(), "file_note");
}

#[test]
fn entity_kind_round_trips_from_storage() {
    for kind in [EntityKind::Issue, EntityKind::FileNote] {
        assert_eq!(EntityKind::from_storage(kind.as_str()), Some(kind));
    }
    assert_eq!(EntityKind::from_storage("nope"), None);
}
