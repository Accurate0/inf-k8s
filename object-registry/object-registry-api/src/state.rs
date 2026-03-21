use crate::permissions::PermissionsManager;
use object_registry_foundations::audit_manager::AuditManager;
use object_registry_foundations::event_manager::EventManager;
use object_registry_foundations::key_manager::KeyManager;
use object_registry_foundations::object_manager::ObjectManager;
use object_registry_foundations::s3_key_manager::S3KeyManager;

#[derive(Clone)]
pub struct AppState {
    pub object_manager: ObjectManager,
    pub s3_client: aws_sdk_s3::Client,
    pub event_manager: EventManager,
    pub key_manager: KeyManager,
    pub s3_key_manager: S3KeyManager,
    pub permissions_manager: PermissionsManager,
    pub audit_manager: AuditManager,
}
