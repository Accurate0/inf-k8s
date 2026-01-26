use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tokio::fs::read_to_string;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    GenerateKeyPair {
        /// Optional key id; if omitted a UUID will be generated
        #[arg(short, long)]
        key_id: Option<String>,
        /// Path to write the public cert (PEM)
        #[arg(short = 'o', long = "cert-output", required = true)]
        cert_output: PathBuf,
        /// Permitted namespace(s)
        #[arg(short = 'n', long = "namespace", required = true, num_args = 1..)]
        namespaces: Vec<String>,
        /// Permitted method(s)
        #[arg(short = 'm', long = "method", required = true, num_args = 1..)]
        methods: Vec<String>,
    },
    Store {
        #[arg(short, long)]
        namespace: String,
        #[arg(short, long)]
        object: String,
        #[arg(short, long)]
        file: String,
        /// Optional version query parameter
        #[arg(long)]
        version: Option<String>,
    },
    Get {
        #[arg(short, long)]
        namespace: String,
        #[arg(short, long)]
        object: String,
        /// Optional version query parameter
        #[arg(long)]
        version: Option<String>,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let config = aws_config::load_from_env().await;

    match args.command {
        Commands::GenerateKeyPair {
            key_id,
            cert_output,
            namespaces,
            methods,
        } => {
            // generate RSA keypair
            let rsa = openssl::rsa::Rsa::generate(4096)?;
            let private_pem = rsa.private_key_to_pem()?; // keep private PEM to write to disk
            let public_pem = rsa.public_key_to_pem()?;

            // Do NOT print the private key. Write the public PEM to `cert_output`.

            // build key details and save public key via KeyManager
            let km = object_registry::key_manager::KeyManager::new(&config);
            let id = key_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

            // keep a UTF-8 string of the public PEM for storage, but retain the bytes
            let public_pem_str = String::from_utf8(public_pem.clone())?;
            let details = object_registry::key_manager::KeyDetails {
                key_id: id.clone(),
                public_key: public_pem_str,
                permitted_namespaces: namespaces,
                permitted_methods: methods,
                created_at: chrono::Utc::now(),
                ttl: None,
            };

            km.add_key(details).await?;
            println!("saved public key with id: {id}");

            // write private key PEM to required path (do not print private contents)
            tokio::fs::write(&cert_output, private_pem).await?;
            println!("wrote private key to: {}", cert_output.display());
        }
        Commands::Store {
            namespace,
            object,
            file,
            version,
        } => {
            // read file
            let path = PathBuf::from(file);
            let file_contents = read_to_string(path).await?;

            // generate temporary keypair (in-memory) and register public key with TTL 5 minutes
            let rsa = openssl::rsa::Rsa::generate(4096)?;
            let private_pem = rsa.private_key_to_pem()?;
            let public_pem = rsa.public_key_to_pem()?;
            let kid = uuid::Uuid::new_v4().to_string();

            let ttl = chrono::Utc::now().timestamp() + 300; // 5 minutes

            let km = object_registry::key_manager::KeyManager::new(&config);
            let details = object_registry::key_manager::KeyDetails {
                key_id: kid.clone(),
                public_key: String::from_utf8(public_pem.clone())?,
                permitted_namespaces: vec![namespace.clone()],
                permitted_methods: vec!["PUT".to_string()],
                created_at: chrono::Utc::now(),
                ttl: Some(ttl),
            };

            km.add_key(details).await?;

            // construct ApiClient using the private key (in-memory)
            let api = object_registry::ApiClient::new(
                private_pem.clone(),
                kid.clone(),
                "object-registry-cli",
            );

            // perform put_object; include source header via API client's post_json/get helpers is not available for raw body, so call put_object directly
            api.put_object(
                &namespace,
                &object,
                version.as_deref(),
                file_contents.as_bytes(),
            )
            .await?;

            println!("stored {}/{}", namespace, object);
        }
        Commands::Get {
            namespace,
            object,
            version,
        } => {
            // generate temporary keypair (in-memory) and register public key with TTL 5 minutes
            let rsa = openssl::rsa::Rsa::generate(4096)?;
            let private_pem = rsa.private_key_to_pem()?;
            let public_pem = rsa.public_key_to_pem()?;
            let kid = uuid::Uuid::new_v4().to_string();

            let ttl = chrono::Utc::now().timestamp() + 300; // 5 minutes

            let km = object_registry::key_manager::KeyManager::new(&config);
            let details = object_registry::key_manager::KeyDetails {
                key_id: kid.clone(),
                public_key: String::from_utf8(public_pem.clone())?,
                permitted_namespaces: vec![namespace.clone()],
                permitted_methods: vec!["GET".to_string()],
                created_at: chrono::Utc::now(),
                ttl: Some(ttl),
            };

            km.add_key(details).await?;

            // construct ApiClient using the private key (in-memory)
            let api = object_registry::ApiClient::new(
                private_pem.clone(),
                kid.clone(),
                "config-catalog-cli",
            );

            // fetch object as raw string
            let body: String = api
                .get_object(&namespace, &object, version.as_deref())
                .await?;
            println!("{}", body);
        }
    }

    Ok(())
}
