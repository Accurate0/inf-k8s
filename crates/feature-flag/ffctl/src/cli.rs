use crate::output::Format;
use clap::{Parser, Subcommand, ValueEnum};
use feature_flag_proto as pb;

#[derive(Parser)]
#[command(
    name = "ffctl",
    version,
    about = "feature-flags admin CLI and TUI",
    long_about = "feature-flags admin CLI and TUI. Run without a subcommand to open the TUI."
)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        env = "FFCTL_URL",
        default_value = "https://feature-flags.inf-k8s.net",
        help = "feature-flags gRPC endpoint"
    )]
    pub url: String,

    #[arg(
        long,
        global = true,
        env = "FFCTL_OIDC_ISSUER",
        default_value = "https://idm.anurag.sh/oauth2/openid/feature-flags",
        help = "OIDC issuer used to log in"
    )]
    pub issuer: String,

    #[arg(
        long,
        global = true,
        env = "FFCTL_OIDC_CLIENT_ID",
        default_value = "feature-flags",
        help = "OAuth client id"
    )]
    pub client_id: String,

    #[arg(
        long,
        global = true,
        env = "FFCTL_TOKEN",
        hide_env_values = true,
        help = "Use this bearer token instead of the stored login"
    )]
    pub token: Option<String>,

    #[arg(
        long,
        global = true,
        env = "FFCTL_NO_AUTH",
        help = "Send no token, for a local server running without OIDC"
    )]
    pub no_auth: bool,

    #[arg(
        long,
        global = true,
        env = "FFCTL_ACTOR",
        help = "Audit identity sent with --no-auth; defaults to `git config user.email`"
    )]
    pub actor: Option<String>,

    #[arg(
        long,
        short,
        global = true,
        value_enum,
        default_value = "json",
        help = "Output format"
    )]
    pub output: Format,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    #[command(about = "Open the interactive terminal UI")]
    Tui,

    #[command(about = "Log in through the browser and store the session")]
    Login,

    #[command(about = "Forget the stored session")]
    Logout,

    #[command(about = "Show the logged in user")]
    Whoami,

    #[command(about = "Manage flags")]
    Flag {
        #[command(subcommand)]
        action: FlagAction,
    },

    #[command(about = "Manage a flag's variants")]
    Variant {
        #[command(subcommand)]
        action: VariantAction,
    },

    #[command(about = "Manage targeting segments")]
    Segment {
        #[command(subcommand)]
        action: SegmentAction,
    },

    #[command(about = "Replace the ordered targeting rules for a flag")]
    Rules {
        #[command(subcommand)]
        action: RulesAction,
    },

    #[command(about = "Declarative, Terraform-style management from a config directory")]
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    #[command(about = "Show the audit log of admin changes, newest first")]
    Audit {
        #[arg(long, default_value = "", help = "Only this kind: flag or segment")]
        kind: String,

        #[arg(long, default_value = "", help = "Only this flag or segment key")]
        key: String,

        #[arg(long, default_value_t = 50, help = "Maximum rows to return")]
        limit: u32,
    },

    #[command(about = "Evaluate flags for a targeting key and attributes")]
    Eval {
        #[arg(help = "Targeting key used for bucketing")]
        targeting_key: String,

        #[arg(long, help = "Evaluate only this flag instead of all flags")]
        flag: Option<String>,

        #[arg(
            long = "attr",
            help = "A context attribute as key=json (bare strings allowed); repeatable"
        )]
        attrs: Vec<String>,

        #[arg(long, help = "All context attributes as one JSON object")]
        attributes: Option<String>,
    },

    #[command(about = "Stream config changes as they happen")]
    Watch,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    #[command(about = "Show the diff between the config directory and the live service")]
    Plan {
        #[arg(
            long,
            default_value = "config",
            help = "Directory holding flags/*.yaml and segments.yaml"
        )]
        dir: String,
    },

    #[command(
        about = "Reconcile the live service to the config directory",
        long_about = "Reconcile the live service to the config directory. Flags and segments absent from the config are deleted. Prints the plan and asks to confirm."
    )]
    Apply {
        #[arg(long, default_value = "config")]
        dir: String,

        #[arg(long, help = "Skip the interactive confirmation prompt")]
        auto_approve: bool,
    },

    #[command(about = "Dump the live flags and segments into the config directory layout")]
    Export {
        #[arg(long, default_value = "config")]
        dir: String,
    },

    #[command(about = "Write the JSON Schemas for the config files to <dir>/schema")]
    Schema {
        #[arg(long, default_value = "config")]
        dir: String,
    },
}

#[derive(Subcommand)]
pub enum FlagAction {
    #[command(
        about = "Create a flag",
        long_about = "Create a flag. Variants are key=value pairs; values are parsed as JSON, falling back to a bare string (e.g. on=true, model=gpt-4)."
    )]
    Create {
        key: String,

        #[arg(long, value_enum)]
        r#type: FlagType,

        #[arg(
            long,
            help = "Enable targeting; otherwise the default variant always serves"
        )]
        enabled: bool,

        #[arg(
            long,
            help = "Variant served when no rule matches or targeting is disabled"
        )]
        default: String,

        #[arg(long = "variant", help = "A variant as key=json; repeatable")]
        variants: Vec<String>,
    },

    #[command(about = "Fetch a single flag")]
    Get { key: String },

    #[command(about = "List flags")]
    List {
        #[arg(long, help = "Include archived flags")]
        archived: bool,
    },

    #[command(about = "Set a flag's enabled state and default variant")]
    Update {
        key: String,

        #[arg(long)]
        enabled: bool,

        #[arg(long)]
        default: String,
    },

    #[command(about = "Enable targeting for a flag")]
    Enable { key: String },

    #[command(about = "Disable targeting for a flag")]
    Disable { key: String },

    #[command(about = "Edit a flag as YAML in $EDITOR, then review and apply the diff")]
    Edit {
        key: String,

        #[arg(long, short, help = "Apply without asking")]
        yes: bool,
    },

    #[command(about = "Archive a flag, or restore it with --restore")]
    Archive {
        key: String,

        #[arg(long)]
        restore: bool,
    },

    #[command(about = "Permanently delete a flag")]
    Delete { key: String },
}

#[derive(Subcommand)]
pub enum VariantAction {
    #[command(about = "Add or update a variant; the value is JSON or a bare string")]
    Set {
        flag_key: String,
        variant_key: String,
        value: String,
    },

    #[command(about = "Remove a variant from a flag")]
    Delete {
        flag_key: String,
        variant_key: String,
    },
}

#[derive(Subcommand)]
pub enum SegmentAction {
    #[command(
        about = "Create or update a segment from a JSON document",
        long_about = "Create or update a segment from a JSON document, e.g. {\"key\":\"beta\",\"name\":\"Beta\",\"constraints\":[{\"attribute\":\"plan\",\"operator\":\"IN\",\"values\":[\"pro\"]}]}. Operators are the short names from the proto (EQ, IN, STARTS_WITH, ...)."
    )]
    Set { json: String },

    #[command(about = "Fetch a single segment")]
    Get { key: String },

    #[command(about = "List segments")]
    List,

    #[command(about = "Edit a segment as YAML in $EDITOR, then review and apply the diff")]
    Edit {
        key: String,

        #[arg(long, short, help = "Apply without asking")]
        yes: bool,
    },

    #[command(about = "Delete a segment")]
    Delete { key: String },
}

#[derive(Subcommand)]
pub enum RulesAction {
    #[command(
        about = "Replace a flag's rules from a JSON array ordered by priority",
        long_about = "Replace a flag's rules from a JSON array ordered by priority, e.g. [{\"segment_key\":\"beta\",\"variant_key\":\"on\"},{\"constraint_groups\":[[{\"attribute\":\"country\",\"operator\":\"IN\",\"values\":[\"AU\",\"NZ\"]}],[{\"attribute\":\"plan\",\"operator\":\"EQ\",\"values\":[\"pro\"]}]],\"variant_key\":\"on\"}]. constraint_groups is CNF: groups are AND-combined, constraints within a group OR-combined; constraints (a flat array) is sugar for plain AND. A FLAG_MATCHES operator depends on another flag: attribute is the flag key and values the variant keys it must resolve to."
    )]
    Set { flag_key: String, json: String },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum FlagType {
    Bool,
    String,
    Int,
    Float,
    Object,
}

impl From<FlagType> for pb::ValueType {
    fn from(flag_type: FlagType) -> Self {
        match flag_type {
            FlagType::Bool => pb::ValueType::Boolean,
            FlagType::String => pb::ValueType::String,
            FlagType::Int => pb::ValueType::Integer,
            FlagType::Float => pb::ValueType::Float,
            FlagType::Object => pb::ValueType::Object,
        }
    }
}
