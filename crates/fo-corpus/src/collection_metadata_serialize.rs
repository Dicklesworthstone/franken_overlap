use serde::ser::{Serialize, SerializeStruct, Serializer};

use crate::collection::CollectionMetadataRow;

impl Serialize for CollectionMetadataRow {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("CollectionMetadataRow", 15)?;
        state.serialize_field("source_path", &self.source_path)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("title", &self.title)?;
        state.serialize_field("family_id", &self.family_id)?;
        state.serialize_field("version_id", &self.version_id)?;
        state.serialize_field("document_type", &self.document_type)?;
        state.serialize_field("effective_date", &self.effective_date)?;
        state.serialize_field("executed_date", &self.executed_date)?;
        state.serialize_field("parties", &self.parties)?;
        state.serialize_field("tags", &self.tags)?;
        state.serialize_field("metadata", &self.metadata)?;
        state.serialize_field("previous_version_id", &self.previous_version_id)?;
        state.serialize_field("amends_id", &self.amends_id)?;
        state.serialize_field("supersedes_id", &self.supersedes_id)?;
        state.serialize_field("related_ids", &self.related_ids)?;
        state.end()
    }
}
