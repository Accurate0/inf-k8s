use clap::{Parser, Subcommand};
use std::error::Error;
use std::path::PathBuf;
use tokio::fs::read_to_string;

fn parse_key_val<T, U>(s: &str) -> Result<(T, U), Box<dyn Error + Send + Sync + 'static>>
where
    T: std::str::FromStr,
    T::Err: Error + Send + Sync + 'static,
    U: std::str::FromStr,
    U::Err: Error + Send + Sync + 'static,
{
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{}`", s))?;
    Ok((s[..pos].parse()?, s[pos + 1..].parse()?))
}

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
        /// Publicly accessible object
        #[arg(long)]
        public: bool,
        /// Optional labels (key=value)
        #[arg(short = 'l', long = "label", value_parser = parse_key_val::<String, String>)]
        labels: Vec<(String, String)>,
    },
    Get {
        #[arg(short, long)]
        namespace: String,
        #[arg(short, long)]
        object: String,
        /// Optional version query parameter
        #[arg(long)]
        version: Option<String>,
        /// Publicly accessible object
        #[arg(long)]
        public: bool,
    },
    Events {
        #[command(subcommand)]
        command: EventsCommand,
    },
}

#[derive(Subcommand, Debug)]
enum EventsCommand {
    Create {
        #[arg(short, long)]
        namespace: String,
        #[arg(long, required = true, num_args = 1..)]
        keys: Vec<String>,
        #[arg(long, default_value = "HTTP")]
        notify_type: String,
        #[arg(long, default_value = "POST")]
        notify_method: String,
        #[arg(long, required = true, num_args = 1..)]
        notify_urls: Vec<String>,
    },
    List {
        #[arg(short, long)]
        namespace: String,
    },
    Delete {
        #[arg(short, long)]
        namespace: String,
        #[arg(long)]
        id: String,
    },
    Update {
        #[arg(short, long)]
        namespace: String,
        #[arg(long)]
        id: String,
        #[arg(long, required = true, num_args = 1..)]
        keys: Vec<String>,
        #[arg(long, default_value = "HTTP")]
        notify_type: String,
        #[arg(long, default_value = "POST")]
        notify_method: String,
        #[arg(long, required = true, num_args = 1..)]
        notify_urls: Vec<String>,
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
            let rsa = openssl::rsa::Rsa::generate(4096)?;
            let private_pem = rsa.private_key_to_pem()?;
            let public_pem = rsa.public_key_to_pem()?;

            let km = object_registry::key_manager::KeyManager::new(&config);
            let id = key_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

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

            tokio::fs::write(&cert_output, private_pem).await?;
            println!("wrote private key to: {}", cert_output.display());
        }
        Commands::Store {
            namespace,
            object,
            file,
            version,
            public,
            labels,
        } => {
            let path = PathBuf::from(file);
            let file_contents = read_to_string(path).await?;

            let (private_pem, kid) = if !public {
                let rsa = openssl::rsa::Rsa::generate(4096)?;
                let private_pem = rsa.private_key_to_pem()?;
                let public_pem = rsa.public_key_to_pem()?;
                let kid = uuid::Uuid::new_v4().to_string();

                let ttl = chrono::Utc::now().timestamp() + 300;

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
                (private_pem, kid)
            } else {
                (vec![], "".to_string())
            };

            let api = object_registry::ApiClient::new(
                private_pem,
                kid,
                "object-registry-cli",
            );

            let labels_map: std::collections::HashMap<String, String> = labels.into_iter().collect();

            api.put_object(
                &namespace,
                &object,
                version.as_deref(),
                public,
                file_contents.as_bytes(),
                if labels_map.is_empty() {
                    None
                } else {
                    Some(labels_map)
                },
            )
            .await?;

            println!("stored {}/{}", namespace, object);
        }
        Commands::Get {
            namespace,
            object,
            version,
            public,
        } => {
            let (private_pem, kid) = if !public {
                let rsa = openssl::rsa::Rsa::generate(4096)?;
                let private_pem = rsa.private_key_to_pem()?;
                let public_pem = rsa.public_key_to_pem()?;
                let kid = uuid::Uuid::new_v4().to_string();

                let ttl = chrono::Utc::now().timestamp() + 300;

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
                (private_pem, kid)
            } else {
                (vec![], "".to_string())
            };

            let api = object_registry::ApiClient::new(private_pem, kid, "config-catalog-cli");

            let body: serde_json::Value = api
                .get_object(&namespace, &object, version.as_deref(), public)
                .await?;
            println!("{}", body);
        }
        Commands::Events { command } => match command {
            EventsCommand::Create {
                namespace,
                keys,
                notify_type,
                notify_method,
                notify_urls,
            } => {
                let rsa = openssl::rsa::Rsa::generate(4096)?;
                let private_pem = rsa.private_key_to_pem()?;
                let public_pem = rsa.public_key_to_pem()?;
                let kid = uuid::Uuid::new_v4().to_string();

                let ttl = chrono::Utc::now().timestamp() + 300;

                let km = object_registry::key_manager::KeyManager::new(&config);
                let details = object_registry::key_manager::KeyDetails {
                    key_id: kid.clone(),
                    public_key: String::from_utf8(public_pem.clone())?,
                    permitted_namespaces: vec![namespace.clone()],
                    permitted_methods: vec!["POST".to_string()],
                    created_at: chrono::Utc::now(),
                    ttl: Some(ttl),
                };

                km.add_key(details).await?;

                let api = object_registry::ApiClient::new(
                    private_pem.clone(),
                    kid.clone(),
                    "object-registry-cli",
                );

                let event_req = object_registry::types::EventRequest {
                    keys,
                    notify: object_registry::types::NotifyRequest {
                        r#type: notify_type,
                        method: notify_method,
                        urls: notify_urls,
                    },
                    created_at: None,
                };

                let created = api.post_event(&namespace, &event_req).await?;
                println!("created event with id: {}", created.id);
            }
            EventsCommand::List { namespace } => {
                let rsa = openssl::rsa::Rsa::generate(4096)?;
                let private_pem = rsa.private_key_to_pem()?;
                let public_pem = rsa.public_key_to_pem()?;
                let kid = uuid::Uuid::new_v4().to_string();

                let ttl = chrono::Utc::now().timestamp() + 300;

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

                let api = object_registry::ApiClient::new(
                    private_pem.clone(),
                    kid.clone(),
                    "object-registry-cli",
                );

                let events = api.list_events(&namespace).await?;
                println!("{}", serde_json::to_string_pretty(&events)?);
            }
            EventsCommand::Delete { namespace, id } => {
                let rsa = openssl::rsa::Rsa::generate(4096)?;
                let private_pem = rsa.private_key_to_pem()?;
                let public_pem = rsa.public_key_to_pem()?;
                let kid = uuid::Uuid::new_v4().to_string();

                let ttl = chrono::Utc::now().timestamp() + 300;

                let km = object_registry::key_manager::KeyManager::new(&config);
                let details = object_registry::key_manager::KeyDetails {
                    key_id: kid.clone(),
                    public_key: String::from_utf8(public_pem.clone())?,
                    permitted_namespaces: vec![namespace.clone()],
                    permitted_methods: vec!["DELETE".to_string()],
                    created_at: chrono::Utc::now(),
                    ttl: Some(ttl),
                };

                km.add_key(details).await?;

                let api = object_registry::ApiClient::new(
                    private_pem.clone(),
                    kid.clone(),
                    "object-registry-cli",
                );

                api.delete_event(&namespace, &id).await?;
                println!("deleted event with id: {}", id);
            }
            EventsCommand::Update {
                namespace,
                id,
                keys,
                notify_type,
                notify_method,
                notify_urls,
            } => {
                let rsa = openssl::rsa::Rsa::generate(4096)?;
                let private_pem = rsa.private_key_to_pem()?;
                let public_pem = rsa.public_key_to_pem()?;
                let kid = uuid::Uuid::new_v4().to_string();

                let ttl = chrono::Utc::now().timestamp() + 300;

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

                let api = object_registry::ApiClient::new(
                    private_pem.clone(),
                    kid.clone(),
                    "object-registry-cli",
                );

                let event_req = object_registry::types::EventRequest {
                    keys,
                    notify: object_registry::types::NotifyRequest {
                        r#type: notify_type,
                        method: notify_method,
                        urls: notify_urls,
                    },
                    created_at: None,
                };

                let updated = api.put_event(&namespace, &id, &event_req).await?;
                println!("updated event with id: {}", updated.id);
            }
        },
    }

    Ok(())
}
