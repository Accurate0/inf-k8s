/// Example showing CRUD operations against the object-registry S3-compatible API
/// using the rust-s3 client library.
///
/// Expects:
///   AWS_ACCESS_KEY_ID     - access key generated via `cli generate-s3-key`
///   AWS_SECRET_ACCESS_KEY - secret generated via `cli generate-s3-key`
///
/// Run:
///   AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=... \
///     cargo run --example s3_crud -p object-registry-api
use s3::{Bucket, Region, creds::Credentials};

const ENDPOINT: &str = "https://s3.object-registry.inf-k8s.net";
const BUCKET: &str = "test-namespace";
const REGION: &str = "object-registry.inf-k8s.net";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let access_key = std::env::var("AWS_ACCESS_KEY_ID").expect("AWS_ACCESS_KEY_ID must be set");
    let secret_key =
        std::env::var("AWS_SECRET_ACCESS_KEY").expect("AWS_SECRET_ACCESS_KEY must be set");

    let credentials = Credentials::new(Some(&access_key), Some(&secret_key), None, None, None)?;

    let region = Region::Custom {
        region: REGION.to_string(),
        endpoint: ENDPOINT.to_string(),
    };

    let bucket = Bucket::new(BUCKET, region, credentials)?.with_path_style();

    // ── PUT ──────────────────────────────────────────────────────────────────
    println!("PUT example-object ...");
    let body = b"hello from rust-s3";
    let response = bucket.put_object("example-object", body).await?;
    println!("  status: {}", response.status_code());
    assert_eq!(response.status_code(), 200);

    // ── GET ──────────────────────────────────────────────────────────────────
    println!("GET example-object ...");
    let response = bucket.get_object("example-object").await?;
    println!("  status: {}", response.status_code());
    assert_eq!(response.status_code(), 200);
    println!("  body:   {}", std::str::from_utf8(response.bytes())?);
    assert_eq!(response.bytes().as_ref(), body.as_ref());

    // ── PUT (update) ─────────────────────────────────────────────────────────
    println!("PUT example-object (update) ...");
    let updated_body = b"updated content";
    let response = bucket.put_object("example-object", updated_body).await?;
    println!("  status: {}", response.status_code());
    assert_eq!(response.status_code(), 200);

    let response = bucket.get_object("example-object").await?;
    assert_eq!(response.bytes().as_ref(), updated_body.as_ref());
    println!(
        "  body after update: {}",
        std::str::from_utf8(response.bytes())?
    );

    // ── LIST ─────────────────────────────────────────────────────────────────
    println!("LIST {} ...", BUCKET);
    let pages = bucket.list(String::new(), None).await?;
    for page in &pages {
        for obj in &page.contents {
            println!("  - {} ({} bytes)", obj.key, obj.size);
        }
    }

    // ── DELETE ───────────────────────────────────────────────────────────────
    println!("DELETE example-object ...");
    let response = bucket.delete_object("example-object").await?;
    println!("  status: {}", response.status_code());
    assert_eq!(response.status_code(), 204);

    println!("\nAll CRUD operations succeeded.");
    Ok(())
}
