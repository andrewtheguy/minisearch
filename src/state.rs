#[derive(Clone)]
pub struct AppState {
    pub s3_client: aws_sdk_s3::Client,
    pub bucket_name: String,
}
