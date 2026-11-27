use chrono::{DateTime, Utc};

#[cynic::schema("graph")]
mod schema {}

cynic::impl_scalar!(DateTime<Utc>, schema::DateTime);
cynic::impl_scalar!(serde_json::Value, schema::JSON);

#[derive(cynic::Enum, Clone, Copy, Debug, PartialEq, Eq)]
#[cynic(graphql_type = "RegistryTarget")]
pub enum Target {
    #[cynic(rename = "DARWIN_ARM_64")]
    DarwinArm64,
    #[cynic(rename = "LINUX_AMD_64")]
    LinuxAmd64,
    Wasm,
}

#[derive(cynic::Enum, Clone, Copy, Debug, PartialEq, Eq)]
#[cynic(graphql_type = "RegistryHarborAction")]
pub enum HarborAction {
    Pull,
    Push,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RegistryWhoAmI")]
pub struct WhoAmI {
    pub subject: String,
    pub admin: bool,
    pub groups: Vec<String>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RegistryCatalogRef")]
pub struct CatalogRef {
    pub seq: i32,
    pub digest: Option<String>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RegistryArtifact")]
pub struct Artifact {
    pub target: Target,
    pub sha256: String,
    pub size: i32,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RegistryYanked")]
pub struct Yanked {
    pub reason: Option<String>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RegistryVersion")]
pub struct Version {
    pub version: String,
    pub status: String,
    pub artifacts: Vec<Artifact>,
    pub yanked: Option<Yanked>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RegistryPackage")]
pub struct Package {
    pub versions: Vec<Version>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RegistryPackage")]
pub struct PackageName {
    pub name: String,
    pub binary_name: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RegistryTrustedPublisher")]
pub struct TrustedPublisher {
    pub id: cynic::Id,
    pub ecosystem: String,
    pub package: Option<PackageName>,
    pub repository: String,
    pub ref_pattern: Option<String>,
    pub workflow: Option<String>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RegistryScope")]
pub struct Scope {
    pub ecosystem: String,
    pub package: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RegistryPublishToken")]
pub struct PublishToken {
    pub id: cynic::Id,
    pub name: String,
    pub prefix: String,
    pub scopes: Vec<Scope>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RegistryCreatedPublishToken")]
pub struct CreatedPublishToken {
    pub secret: String,
    pub token: PublishToken,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RegistryAuditEntry")]
pub struct AuditEntry {
    pub at: DateTime<Utc>,
    pub actor: String,
    pub action: String,
    pub package_name: Option<String>,
    pub version: Option<String>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RegistryEnvelope")]
pub struct Envelope {
    pub payload: String,
    pub signature: String,
    pub key_id: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RegistryHarborAccess")]
pub struct HarborAccess {
    pub project: String,
    pub actions: Vec<HarborAction>,
    pub package: Option<String>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RegistryHarborCredential")]
pub struct HarborCredential {
    pub registry: String,
    pub username: String,
    pub password: String,
    pub expires_at: DateTime<Utc>,
    pub access: Vec<HarborAccess>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RegistryImageSignature")]
pub struct ImageSignature {
    pub repository: String,
    pub digest: String,
    pub tag: String,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "RegistryHarborAccessInput")]
pub struct HarborAccessInput {
    pub project: String,
    pub actions: Vec<HarborAction>,
    pub package: Option<String>,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "RegistryEnvelopeInput")]
pub struct EnvelopeInput {
    pub payload: String,
    pub signature: String,
    pub key_id: String,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "RegistryPackageAuthInput")]
pub struct PackageAuthInput {
    pub issuer: String,
    pub client_id: String,
    pub scopes: String,
    pub token_env: String,
    pub lazy: bool,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "RegistryCreateVersionInput")]
pub struct CreateVersionInput {
    pub package: String,
    pub version: String,
    pub targets: Vec<Target>,
    pub description: Option<String>,
    pub binary_name: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub auth: Option<PackageAuthInput>,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "RegistryCreatePackageInput")]
pub struct CreatePackageInput {
    pub name: String,
    pub description: String,
    pub binary_name: Option<String>,
    pub aliases: Vec<String>,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "RegistryUpdatePackageInput")]
pub struct UpdatePackageInput {
    pub description: Option<String>,
    pub binary_name: Option<String>,
    pub aliases: Option<Vec<String>>,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "RegistryCreateTrustedPublisherInput")]
pub struct CreateTrustedPublisherInput {
    pub package: String,
    pub repository: String,
    pub ref_pattern: Option<String>,
    pub workflow: Option<String>,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "RegistryCreatePublishTokenInput")]
pub struct CreatePublishTokenInput {
    pub name: String,
    pub packages: Vec<String>,
    pub expires_in_days: Option<i32>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query")]
pub struct WhoAmIQuery {
    pub registry_whoami: Option<WhoAmI>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query")]
pub struct CatalogQuery {
    pub registry_catalog: Option<CatalogRef>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct VersionVariables {
    pub package: String,
    pub version: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "VersionVariables")]
pub struct VersionQuery {
    #[arguments(package: $package, version: $version)]
    pub registry_version: Option<Version>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct NameVariables {
    pub name: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "NameVariables")]
pub struct PackageQuery {
    #[arguments(name: $name)]
    pub registry_package: Option<Package>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct PackageFilterVariables {
    pub package: Option<String>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "PackageFilterVariables")]
pub struct TrustedPublishersQuery {
    #[arguments(package: $package)]
    pub registry_trusted_publishers: Option<Vec<TrustedPublisher>>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query")]
pub struct PublishTokensQuery {
    pub registry_publish_tokens: Option<Vec<PublishToken>>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct AuditVariables {
    pub package: Option<String>,
    pub limit: i32,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "AuditVariables")]
pub struct AuditQuery {
    #[arguments(package: $package, limit: $limit)]
    pub registry_audit: Option<Vec<AuditEntry>>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct HarborCredentialVariables {
    pub access: Vec<HarborAccessInput>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "HarborCredentialVariables")]
pub struct CreateHarborCredential {
    #[arguments(access: $access)]
    pub registry_create_harbor_credential: HarborCredential,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct UsernameVariables {
    pub username: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "UsernameVariables")]
pub struct RevokeHarborCredential {
    #[arguments(username: $username)]
    pub registry_revoke_harbor_credential: String,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct SignImageVariables {
    pub repository: String,
    pub digest: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "SignImageVariables")]
pub struct SignImage {
    #[arguments(repository: $repository, digest: $digest)]
    pub registry_sign_image: ImageSignature,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct CreateVersionVariables {
    pub input: CreateVersionInput,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "CreateVersionVariables")]
pub struct CreateVersion {
    #[arguments(input: $input)]
    pub registry_create_version: Version,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct RegisterArtifactVariables {
    pub package: String,
    pub version: String,
    pub target: Target,
    pub manifest: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "RegisterArtifactVariables")]
pub struct RegisterArtifact {
    #[arguments(package: $package, version: $version, target: $target, manifest: $manifest)]
    pub registry_register_artifact: Artifact,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct CertificateVariables {
    pub package: String,
    pub public_key: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "CertificateVariables")]
pub struct IssueSigningCertificate {
    #[arguments(package: $package, publicKey: $public_key)]
    pub registry_issue_signing_certificate: Envelope,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct PublishVariables {
    pub package: String,
    pub version: String,
    pub statement: EnvelopeInput,
    pub certificate: EnvelopeInput,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "PublishVariables")]
pub struct PublishVersion {
    #[arguments(package: $package, version: $version, statement: $statement, certificate: $certificate)]
    pub registry_publish_version: Version,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct CreatePackageVariables {
    pub input: CreatePackageInput,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "CreatePackageVariables")]
pub struct CreatePackage {
    #[arguments(input: $input)]
    pub registry_create_package: PackageName,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct UpdatePackageVariables {
    pub name: String,
    pub input: UpdatePackageInput,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "UpdatePackageVariables")]
pub struct UpdatePackage {
    #[arguments(name: $name, input: $input)]
    pub registry_update_package: PackageName,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "NameVariables")]
pub struct ArchivePackage {
    #[arguments(name: $name)]
    pub registry_archive_package: PackageName,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "NameVariables")]
pub struct UnarchivePackage {
    #[arguments(name: $name)]
    pub registry_unarchive_package: PackageName,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "NameVariables")]
pub struct DeletePackage {
    #[arguments(name: $name)]
    pub registry_delete_package: cynic::Id,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct CreateTrustedPublisherVariables {
    pub input: CreateTrustedPublisherInput,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(
    graphql_type = "Mutation",
    variables = "CreateTrustedPublisherVariables"
)]
pub struct CreateTrustedPublisher {
    #[arguments(input: $input)]
    pub registry_create_trusted_publisher: TrustedPublisher,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct IdVariables {
    pub id: cynic::Id,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "IdVariables")]
pub struct DeleteTrustedPublisher {
    #[arguments(id: $id)]
    pub registry_delete_trusted_publisher: cynic::Id,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct CreatePublishTokenVariables {
    pub input: CreatePublishTokenInput,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "CreatePublishTokenVariables")]
pub struct CreatePublishToken {
    #[arguments(input: $input)]
    pub registry_create_publish_token: CreatedPublishToken,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "IdVariables")]
pub struct RevokePublishToken {
    #[arguments(id: $id)]
    pub registry_revoke_publish_token: PublishToken,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct YankVariables {
    pub id: cynic::Id,
    pub reason: Option<String>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "YankVariables")]
pub struct YankVersion {
    #[arguments(id: $id, reason: $reason)]
    pub registry_yank_version: Version,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "IdVariables")]
pub struct UnyankVersion {
    #[arguments(id: $id)]
    pub registry_unyank_version: Version,
}
