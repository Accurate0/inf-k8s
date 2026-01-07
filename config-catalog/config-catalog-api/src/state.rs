#[derive(Clone)]
pub struct AppState {
    pub s3_client: aws_sdk_s3::Client,
    pub secrets_client: aws_sdk_secretsmanager::Client,
}
