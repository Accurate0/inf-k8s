use crate::model::{Rule, Segment, ValueType, Variant};
use crate::pb;
use crate::pb::admin_server::Admin;
use crate::snapshot::SnapshotManager;
use crate::store::Store;
use std::sync::Arc;
use tonic::{Request, Response, Status};

pub struct AdminService {
    store: Store,
    mgr: Arc<SnapshotManager>,
}

impl AdminService {
    pub fn new(store: Store, mgr: Arc<SnapshotManager>) -> Self {
        Self { store, mgr }
    }

    /// Refresh the in-memory snapshot immediately after a write rather than waiting
    /// for the LISTEN/NOTIFY round-trip, so read-your-writes holds on this replica.
    async fn refresh(&self) {
        if let Err(e) = self.mgr.reload().await {
            tracing::error!("post-write snapshot reload failed: {e}");
        }
    }
}

#[tonic::async_trait]
impl Admin for AdminService {
    async fn create_flag(
        &self,
        request: Request<pb::CreateFlagRequest>,
    ) -> Result<Response<pb::Flag>, Status> {
        let req = request.into_inner();
        let value_type = ValueType::try_from(req.value_type())
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        let variants: Vec<_> = req.variants.iter().map(Variant::from).collect();
        let flag = self
            .store
            .create_flag(&req.key, value_type, req.enabled, &req.default_variant_key, &variants)
            .await?;
        self.refresh().await;
        Ok(Response::new(pb::Flag::from(&flag)))
    }

    async fn get_flag(
        &self,
        request: Request<pb::GetFlagRequest>,
    ) -> Result<Response<pb::Flag>, Status> {
        let flag = self.store.get_flag(&request.into_inner().key).await?;
        Ok(Response::new(pb::Flag::from(&flag)))
    }

    async fn list_flags(
        &self,
        request: Request<pb::ListFlagsRequest>,
    ) -> Result<Response<pb::ListFlagsResponse>, Status> {
        let flags = self.store.list_flags(request.into_inner().include_archived).await?;
        Ok(Response::new(pb::ListFlagsResponse {
            flags: flags.iter().map(pb::Flag::from).collect(),
        }))
    }

    async fn update_flag(
        &self,
        request: Request<pb::UpdateFlagRequest>,
    ) -> Result<Response<pb::Flag>, Status> {
        let req = request.into_inner();
        let flag = self
            .store
            .update_flag(&req.key, req.enabled, &req.default_variant_key)
            .await?;
        self.refresh().await;
        Ok(Response::new(pb::Flag::from(&flag)))
    }

    async fn archive_flag(
        &self,
        request: Request<pb::ArchiveFlagRequest>,
    ) -> Result<Response<pb::Flag>, Status> {
        let req = request.into_inner();
        let flag = self.store.archive_flag(&req.key, req.archived).await?;
        self.refresh().await;
        Ok(Response::new(pb::Flag::from(&flag)))
    }

    async fn delete_flag(
        &self,
        request: Request<pb::DeleteFlagRequest>,
    ) -> Result<Response<pb::DeleteFlagResponse>, Status> {
        self.store.delete_flag(&request.into_inner().key).await?;
        self.refresh().await;
        Ok(Response::new(pb::DeleteFlagResponse {}))
    }

    async fn upsert_variant(
        &self,
        request: Request<pb::UpsertVariantRequest>,
    ) -> Result<Response<pb::Flag>, Status> {
        let req = request.into_inner();
        let variant = req
            .variant
            .as_ref()
            .map(Variant::from)
            .ok_or_else(|| Status::invalid_argument("variant is required"))?;
        let flag = self.store.upsert_variant(&req.flag_key, &variant).await?;
        self.refresh().await;
        Ok(Response::new(pb::Flag::from(&flag)))
    }

    async fn delete_variant(
        &self,
        request: Request<pb::DeleteVariantRequest>,
    ) -> Result<Response<pb::Flag>, Status> {
        let req = request.into_inner();
        let flag = self.store.delete_variant(&req.flag_key, &req.variant_key).await?;
        self.refresh().await;
        Ok(Response::new(pb::Flag::from(&flag)))
    }

    async fn create_segment(
        &self,
        request: Request<pb::CreateSegmentRequest>,
    ) -> Result<Response<pb::Segment>, Status> {
        self.upsert_segment_inner(request.into_inner().segment).await
    }

    async fn get_segment(
        &self,
        request: Request<pb::GetSegmentRequest>,
    ) -> Result<Response<pb::Segment>, Status> {
        let segment = self.store.get_segment(&request.into_inner().key).await?;
        Ok(Response::new(pb::Segment::from(&segment)))
    }

    async fn list_segments(
        &self,
        _request: Request<pb::ListSegmentsRequest>,
    ) -> Result<Response<pb::ListSegmentsResponse>, Status> {
        let segments = self.store.list_segments().await?;
        Ok(Response::new(pb::ListSegmentsResponse {
            segments: segments.iter().map(pb::Segment::from).collect(),
        }))
    }

    async fn update_segment(
        &self,
        request: Request<pb::UpdateSegmentRequest>,
    ) -> Result<Response<pb::Segment>, Status> {
        self.upsert_segment_inner(request.into_inner().segment).await
    }

    async fn delete_segment(
        &self,
        request: Request<pb::DeleteSegmentRequest>,
    ) -> Result<Response<pb::DeleteSegmentResponse>, Status> {
        self.store.delete_segment(&request.into_inner().key).await?;
        self.refresh().await;
        Ok(Response::new(pb::DeleteSegmentResponse {}))
    }

    async fn set_flag_rules(
        &self,
        request: Request<pb::SetFlagRulesRequest>,
    ) -> Result<Response<pb::Flag>, Status> {
        let req = request.into_inner();
        let rules: Vec<_> = req.rules.iter().map(Rule::from).collect();
        let flag = self.store.set_flag_rules(&req.flag_key, &rules).await?;
        self.refresh().await;
        Ok(Response::new(pb::Flag::from(&flag)))
    }
}

impl AdminService {
    async fn upsert_segment_inner(
        &self,
        segment: Option<pb::Segment>,
    ) -> Result<Response<pb::Segment>, Status> {
        let proto = segment.ok_or_else(|| Status::invalid_argument("segment is required"))?;
        let domain = Segment::try_from(&proto).map_err(|e| Status::invalid_argument(e.to_string()))?;
        let saved = self.store.upsert_segment(&domain).await?;
        self.refresh().await;
        Ok(Response::new(pb::Segment::from(&saved)))
    }
}
