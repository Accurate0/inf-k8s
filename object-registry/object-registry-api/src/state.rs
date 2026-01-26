use crate::permissions::PermissionsManager;
use object_registry::event_manager::EventManager;
use object_registry::key_manager::KeyManager;

#[derive(Clone)]
pub struct AppState {
    pub s3_client: aws_sdk_s3::Client,

    pub event_manager: EventManager,
    pub key_manager: KeyManager,
    pub permissions_manager: PermissionsManager,
}
