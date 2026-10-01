use crate::database::feature::FeatureRepository;
use crate::grpc::map_features_to_full;

/// Maps a feature to the `pb::FeatureFull` of a live `FeatureUpdate`, with the
/// same batched mapping as the stream snapshot. Like the snapshot, only
/// Contextual features carry variants.
pub async fn map_db_feature_to_full_for_broadcast(
    repo: &dyn FeatureRepository,
    f: crate::database::entity::Feature,
) -> Result<crate::grpc::pb::FeatureFull, crate::Error> {
    let mut mapped = map_features_to_full(repo, vec![f]).await?;
    Ok(mapped.remove(0))
}
