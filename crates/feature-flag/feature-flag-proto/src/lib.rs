//! Generated tonic types for the `featureflag.v1` gRPC contract. Shared by the
//! backend service, the API client, and the OpenFeature provider so the wire schema
//! has a single source of truth.

tonic::include_proto!("featureflag.v1");

/// Reflection descriptor so `grpcurl`/clients can introspect the API.
pub const FILE_DESCRIPTOR_SET: &[u8] =
    tonic::include_file_descriptor_set!("featureflag_descriptor");
