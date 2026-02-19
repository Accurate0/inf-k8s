use clap::{Parser, Subcommand};
use comfy_table::Table;
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

#[derive(clap::ValueEnum, Clone, Debug)]
enum OutputFormat {
    Json,
    Yaml,
    Binary,
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
        /// Output format
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Json)]
        format: OutputFormat,
        /// Output file
        #[arg(long)]
        file: Option<PathBuf>,
    },
    List {
        #[arg(short, long)]
        namespace: String,
    },
    Delete {
        #[arg(short, long)]
        namespace: String,
        #[arg(short, long)]
        object: String,
        /// Optional version query parameter
        #[arg(long)]
        version: Option<String>,
    },
    Namespaces,
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
        #[arg(long, required = true)]
        audience: String,
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
        #[arg(long, required = true)]
        audience: String,
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
            labels,
        } => {
            let path = PathBuf::from(file);
            let file_contents = read_to_string(path).await?;

            let (private_pem, kid) = {
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
                    permitted_methods: vec!["object:put".to_string()],
                    created_at: chrono::Utc::now(),
                    ttl: Some(ttl),
                };

                km.add_key(details).await?;
                (private_pem, kid)
            };

            let api = object_registry::ApiClient::new(private_pem, kid, "object-registry-cli");

            let labels_map: std::collections::HashMap<String, String> =
                labels.into_iter().collect();

            api.put_object(
                &namespace,
                &object,
                version.as_deref(),
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
            format,
            file,
        } => {
            let (private_pem, kid) = {
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
                    permitted_methods: vec!["object:get".to_string()],
                    created_at: chrono::Utc::now(),
                    ttl: Some(ttl),
                };

                km.add_key(details).await?;
                (private_pem, kid)
            };

            let api = object_registry::ApiClient::new(private_pem, kid, "object-registry-cli");

            match format {
                OutputFormat::Binary => {
                    let response: object_registry::types::ObjectResponse<Vec<u8>> =
                        api.get_object(&namespace, &object, version.as_deref()).await?;
                    if let Some(path) = file {
                        tokio::fs::write(path, response.payload).await?;
                    } else {
                        use std::io::Write;
                        std::io::stdout().write_all(&response.payload)?;
                    }
                }
                OutputFormat::Json => {
                    let response: object_registry::types::ObjectResponse<serde_json::Value> =
                        api.get_object(&namespace, &object, version.as_deref()).await?;
                    let output = serde_json::to_string_pretty(&response)?;
                    if let Some(path) = file {
                        tokio::fs::write(path, output).await?;
                    } else {
                        println!("{output}");
                    }
                }
                OutputFormat::Yaml => {
                    let response: object_registry::types::ObjectResponse<serde_json::Value> =
                        api.get_object(&namespace, &object, version.as_deref()).await?;
                    let output = serde_yaml::to_string(&response)?;
                    if let Some(path) = file {
                        tokio::fs::write(path, output).await?;
                    } else {
                        println!("{output}");
                    }
                }
            }
        }
        Commands::List { namespace } => {
            let (private_pem, kid) = {
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
                    permitted_methods: vec!["object:get".to_string()],
                    created_at: chrono::Utc::now(),
                    ttl: Some(ttl),
                };

                km.add_key(details).await?;
                (private_pem, kid)
            };

            let api = object_registry::ApiClient::new(private_pem, kid, "object-registry-cli");

            let response = api.list_objects(&namespace).await?;
            let mut table = Table::new();
            table.set_header(vec![
                "Key",
                "Version",
                "Content-Type",
                "Size",
                "Created At",
                "Created By",
            ]);

            for obj in response.objects {
                table.add_row(vec![
                    obj.key,
                    obj.metadata.version,
                    obj.metadata.content_type,
                    obj.metadata.size.to_string(),
                    obj.metadata.created_at,
                    obj.metadata.created_by,
                ]);
            }
            println!("{table}");
        }
        Commands::Delete {
            namespace,
            object,
            version,
        } => {
            let (private_pem, kid) = {
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
                    permitted_methods: vec!["object:delete".to_string()],
                    created_at: chrono::Utc::now(),
                    ttl: Some(ttl),
                };

                km.add_key(details).await?;
                (private_pem, kid)
            };

            let api = object_registry::ApiClient::new(private_pem, kid, "object-registry-cli");

            api.delete_object(&namespace, &object, version.as_deref())
                .await?;

            println!("deleted {}/{}", namespace, object);
        }
        Commands::Namespaces => {
            let (private_pem, kid) = {
                let rsa = openssl::rsa::Rsa::generate(4096)?;
                let private_pem = rsa.private_key_to_pem()?;
                let public_pem = rsa.public_key_to_pem()?;
                let kid = uuid::Uuid::new_v4().to_string();

                let ttl = chrono::Utc::now().timestamp() + 300;

                let km = object_registry::key_manager::KeyManager::new(&config);
                let details = object_registry::key_manager::KeyDetails {
                    key_id: kid.clone(),
                    public_key: String::from_utf8(public_pem.clone())?,
                    permitted_namespaces: vec!["*".to_string()],
                    permitted_methods: vec!["namespace:list".to_string()],
                    created_at: chrono::Utc::now(),
                    ttl: Some(ttl),
                };

                km.add_key(details).await?;
                (private_pem, kid)
            };

            let api = object_registry::ApiClient::new(private_pem, kid, "object-registry-cli");

            let namespaces = api.list_namespaces().await?;
            for ns in namespaces {
                println!("{ns}");
            }
        }
        Commands::Events { command } => match command {
            EventsCommand::Create {
                namespace,
                keys,
                notify_type,
                notify_method,
                notify_urls,
                audience,
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
                    permitted_methods: vec!["event:post".to_string()],
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
                    audience,
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
                    permitted_methods: vec!["event:get".to_string()],
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
                let mut table = Table::new();
                table.set_header(vec![
                    "ID",
                    "Keys",
                    "Notify Details",
                    "Created At",
                ]);

                for event in events {
                    let notify_details = format!(
                        "Type: {}\nMethod: {}\nURLs: {}",
                        event.notify.r#type,
                        event.notify.method,
                        event.notify.urls.join(", ")
                    );
                    table.add_row(vec![
                        event.id,
                        event.keys.join(", "),
                        notify_details,
                        event.created_at,
                    ]);
                }
                println!("{table}");
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
                    permitted_methods: vec!["event:delete".to_string()],
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
                audience,
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
                    permitted_methods: vec!["event:put".to_string()],
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
                    audience,
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
