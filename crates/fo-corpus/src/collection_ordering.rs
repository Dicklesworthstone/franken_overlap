use std::cmp::Ordering;

use crate::collection::CollectionRelationKind;

impl PartialOrd for CollectionRelationKind {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CollectionRelationKind {
    fn cmp(&self, other: &Self) -> Ordering {
        relation_rank(*self).cmp(&relation_rank(*other))
    }
}

const fn relation_rank(kind: CollectionRelationKind) -> u8 {
    match kind {
        CollectionRelationKind::PreviousVersion => 0,
        CollectionRelationKind::AmendmentOf => 1,
        CollectionRelationKind::RestatementOf => 2,
        CollectionRelationKind::Supersedes => 3,
        CollectionRelationKind::ExhibitTo => 4,
        CollectionRelationKind::IncorporatesByReference => 5,
        CollectionRelationKind::Governs => 6,
        CollectionRelationKind::TemplateFor => 7,
        CollectionRelationKind::RelatedTo => 8,
    }
}
