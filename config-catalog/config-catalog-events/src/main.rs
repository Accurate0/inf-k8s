use aws_lambda_events::event::s3::S3Event;
use lambda_runtime::{Error, LambdaEvent, run, service_fn, tracing};

async fn s3_event_handler(event: LambdaEvent<S3Event>) -> Result<(), Error> {
    tracing::info!("got event: {event:?}");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing::init_default_subscriber();
    run(service_fn(s3_event_handler)).await
}
