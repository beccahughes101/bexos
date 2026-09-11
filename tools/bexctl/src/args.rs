//! The complete, offline CLI grammar. Keep transport work out of this module.
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "bexctl",
    version = "0.1.0",
    about = "Inspect and control a BexOS developer target",
    after_help = "Example: bexctl --socket /tmp/debugd.sock ps\nUse `bexctl help COMMAND` for command details."
)]
pub struct Cli {
    /// QEMU debugd Unix socket (required for target commands)
    #[arg(long)]
    pub socket: Option<PathBuf>,
    /// Structured output format; place before the command
    #[arg(long, value_enum, default_value = "table")]
    pub format: Format,
    #[command(subcommand)]
    pub command: Command,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Format {
    Table,
    Json,
    Tsv,
}
#[derive(Subcommand)]
pub enum Command {
    /// Check target health
    Health,
    /// List kernel processes and resource groups
    Ps {
        #[arg(long)]
        wide: bool,
    },
    /// List installed applications
    Apps,
    /// Install a local application bundle
    Install { bundle: PathBuf },
    /// Install an application from HTTPS
    InstallUrl {
        #[arg(value_parser=https)]
        url: String,
    },
    /// Refresh domain association metadata
    ReloadWellKnown {
        #[arg(value_parser=domain)]
        domain: String,
    },
    /// Uninstall an application
    Uninstall { package: String },
    /// Launch an application process
    Launch {
        package: String,
        process: String,
        #[arg(default_value = "0")]
        arg0: u64,
        #[arg(long, default_value = "0")]
        uid: u64,
    },
    /// Inspect app progress
    App {
        #[command(subcommand)]
        command: AppCommand,
    },
    /// Read or change component configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Read or change per-user preferences
    Prefs {
        #[command(subcommand)]
        command: PreferencesCommand,
    },
    /// Manage users and password authentication
    Users {
        #[command(subcommand)]
        command: UserCommand,
    },
    /// Manage trusted applications and TEE diagnostics
    Tee {
        #[command(subcommand)]
        command: TeeCommand,
    },
    /// Record and export traces
    Trace {
        #[command(subcommand)]
        command: TraceCommand,
    },
    /// Check, stage, apply, or inspect updates
    Update {
        #[command(subcommand)]
        command: UpdateCommand,
    },
    /// Inspect kernel update state
    Kernel {
        #[command(subcommand)]
        command: KernelCommand,
    },
    /// Install or launch development test applications
    TestApp {
        #[command(subcommand)]
        command: TestAppCommand,
    },
    /// Run an allowlisted diagnostic (not an arbitrary shell command)
    Exec {
        component: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Open an authenticated terminal (requires an installed shell provider)
    Shell(ShellArgs),
    /// Generate host shell completions without a target connection
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}
#[derive(Subcommand)]
pub enum AppCommand {
    /// Report process progress
    Progress {
        #[arg(default_value = "bexos.platform.storage_verify")]
        package: String,
    },
}
#[derive(Subcommand)]
pub enum KernelCommand {
    /// Report platform update status
    UpdateStatus,
}
#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Explicitly lock MDM-lockable fields
    Lock {
        package: String,
        expected_generation: u64,
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Remove runtime locks (product locks remain authoritative)
    Unlock {
        package: String,
        expected_generation: u64,
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Inspect explicit product/runtime lock state
    Policy { package: String },
    /// Get config metadata and optionally save the compiled config
    Get {
        package: String,
        #[arg(short = 'o', long = "out", alias = "output")]
        output: Option<PathBuf>,
    },
    /// Set typed fields using optimistic generation checking
    Set {
        package: String,
        expected_generation: u64,
        #[arg(required=true,value_parser=assignment)]
        assignments: Vec<String>,
    },
    /// Restore default configuration
    Reset {
        package: String,
        expected_generation: u64,
    },
}
#[derive(Subcommand)]
pub enum PreferencesCommand {
    Get {
        package: String,
        #[arg(long)]
        uid: u64,
        #[arg(short = 'o', long = "out", alias = "output")]
        output: Option<PathBuf>,
    },
    Set {
        package: String,
        expected_generation: u64,
        #[arg(long)]
        uid: u64,
        #[arg(required=true,value_parser=assignment)]
        assignments: Vec<String>,
    },
    Reset {
        package: String,
        expected_generation: u64,
        #[arg(long)]
        uid: u64,
    },
}
#[derive(Subcommand)]
pub enum UserCommand {
    /// List users
    List,
    /// Inspect one user
    Get { uid: u64 },
    /// Create a password-authenticated user
    Create {
        #[arg(long)]
        uid: u64,
        #[arg(long)]
        name: String,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        password: String,
    },
    /// Update identity fields or replace a password
    Update {
        #[arg(long)]
        uid: u64,
        #[arg(long)]
        name: String,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        disabled: bool,
        #[arg(long, requires = "new_password")]
        current_password: Option<String>,
        #[arg(long, requires = "current_password")]
        new_password: Option<String>,
    },
    /// Delete a user
    Delete { uid: u64 },
    /// Verify the password and unlock a user vault
    Unlock {
        uid: u64,
        #[arg(long)]
        password: String,
    },
    /// Lock a user vault
    Lock { uid: u64 },
}
#[derive(Subcommand)]
pub enum TeeCommand {
    /// Inspect the TEE
    Info,
    /// List trusted apps
    Apps,
    /// List package-managed trusted apps
    ListPackages,
    /// Install a trusted app
    InstallApp { file: PathBuf },
    /// Uninstall a trusted app
    UninstallApp {
        #[arg(value_parser=uuid)]
        uuid: String,
    },
    /// Open a trusted-app session
    OpenSession {
        #[arg(value_parser=uuid)]
        uuid: String,
    },
    /// Close a trusted-app session
    CloseSession { session_id: u64 },
    /// Invoke a trusted-app command; write raw response bytes to stdout
    Invoke {
        session_id: u64,
        command_id: u32,
        payload: Option<PathBuf>,
    },
    /// Apply a signed TEE image update
    UpdateCore { manifest: PathBuf, image: PathBuf },
    /// Submit the low-level bounded TEE core RPC (image must fit one debug frame)
    UpdateCoreRaw {
        generation: u64,
        target: String,
        image: PathBuf,
    },
    /// Inspect a TEE update
    UpdateStatus,
    /// Run a target diagnostic
    Diagnostic {
        #[arg(value_enum)]
        diagnostic: TeeDiagnostic,
    },
}
#[derive(Clone, Copy, ValueEnum)]
pub enum TeeDiagnostic {
    Keymint,
    Orchestrator,
    Authmgr,
    Concurrent,
}
#[derive(Subcommand)]
pub enum TraceCommand {
    /// Start tracing
    Start(TraceOptions),
    /// Inspect tracing
    Status,
    /// Stop tracing and export the result
    Stop {
        #[arg(short = 'o', long = "output")]
        output: PathBuf,
    },
    /// Record for a duration and export
    Record {
        #[command(flatten)]
        options: TraceOptions,
        #[arg(short = 'o', long = "output")]
        output: PathBuf,
        #[arg(long, default_value = "5000")]
        duration_ms: u64,
    },
}
#[derive(Args)]
pub struct TraceOptions {
    #[arg(long,default_value="all",value_parser=categories)]
    pub categories: String,
    #[arg(long,default_value="2048",value_parser=clap::value_parser!(u32).range(1..))]
    pub buffer_size_kb: u32,
    #[arg(long,default_value="circular",value_parser=["circular","oneshot"])]
    pub buffer_mode: String,
    /// Trace file format (independent of the root structured output format)
    #[arg(long,default_value="perfetto",value_parser=["perfetto","pftrace","fxt","legacy","legacy-bexos-fxt"])]
    pub format: String,
}
#[derive(Subcommand)]
pub enum UpdateCommand {
    /// Check the configured feed; apply implies stage
    Check {
        #[arg(conflicts_with = "all")]
        target: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        stage: bool,
        #[arg(long)]
        apply: bool,
        /// Explicit activation for a TEE or HYPERVISOR feed update
        #[arg(long, value_enum, requires = "apply")]
        activation: Option<FirmwareActivation>,
    },
    /// Upload and apply an app update
    App {
        manifest: PathBuf,
        artifact: PathBuf,
    },
    /// Upload and transplant a service
    Service {
        manifest: PathBuf,
        artifact: PathBuf,
    },
    /// Upload and apply a kernel, TEE, or x86 hypervisor image update
    Platform {
        manifest: PathBuf,
        artifact: PathBuf,
    },
    /// Upload signed monitor or Trusty firmware with explicit activation
    Firmware {
        manifest: PathBuf,
        artifact: PathBuf,
        #[arg(long, value_enum)]
        activation: FirmwareActivation,
    },
    /// Inspect the current update
    Status,
    /// Apply an already staged update
    Apply {
        #[arg(value_enum)]
        kind: UpdateKind,
    },
    /// Transplant a service from the archive store
    StoredService {
        target: String,
        generation: u64,
        archive_id: Option<String>,
    },
    /// Inspect service migration state
    ServiceStatus {
        #[arg(default_value = "bexos.platform.appd")]
        target: String,
    },
}
#[derive(Clone, Copy, ValueEnum)]
pub enum UpdateKind {
    App,
    Service,
    Platform,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum FirmwareActivation {
    Live,
    OnReboot,
}
#[derive(Subcommand)]
pub enum TestAppCommand {
    /// Upload manifest and executable streams
    Install {
        package: String,
        manifest: PathBuf,
        artifact: PathBuf,
    },
    /// Launch a previously installed test app
    Launch {
        package: String,
        process: String,
        #[arg(default_value = "0")]
        arg0: u64,
    },
}
#[derive(Args)]
#[group(skip)]
#[command(group(clap::ArgGroup::new("identity").required(true).multiple(false).args(["system", "user", "uid"])))]
pub struct ShellArgs {
    /// Open the system's preferred shell without user authentication
    #[arg(long,conflicts_with_all=["password","password_stdin"])]
    pub system: bool,
    /// Authenticate by exact user name
    #[arg(long, value_parser = login_name)]
    pub user: Option<String>,
    /// Authenticate by numeric user ID (zero requires --system)
    #[arg(long,value_parser=clap::value_parser!(u64).range(1..))]
    pub uid: Option<u64>,
    /// Password; omitted passwords are prompted without echo
    #[arg(long, conflicts_with = "password_stdin", value_parser = shell_password)]
    pub password: Option<String>,
    /// Read one password line from stdin before terminal data
    #[arg(long)]
    pub password_stdin: bool,
}
fn https(s: &str) -> Result<String, String> {
    if s.starts_with("https://") && s.len() > 8 {
        Ok(s.into())
    } else {
        Err("expected an https:// URL".into())
    }
}
fn domain(s: &str) -> Result<String, String> {
    if !s.is_empty() && !s.contains('/') {
        Ok(s.into())
    } else {
        Err("expected a domain, not a URL or path".into())
    }
}
fn categories(s: &str) -> Result<String, String> {
    bexos_trace::parse_category_list(s)
        .map(|_| s.into())
        .ok_or("unknown trace category".into())
}
fn assignment(s: &str) -> Result<String, String> {
    super::input::parse_config_assignment(s).map(|_| s.into())
}
fn uuid(s: &str) -> Result<String, String> {
    super::input::parse_uuid(s).map(|_| s.into())
}

fn login_name(s: &str) -> Result<String, String> {
    if s.is_empty() || s.len() > 64 || s.chars().any(char::is_control) {
        Err("login name must contain 1–64 bytes without control characters".into())
    } else {
        Ok(s.into())
    }
}
fn shell_password(s: &str) -> Result<String, String> {
    if s.len() > 256 {
        Err("password exceeds 256 bytes".into())
    } else {
        Ok(s.into())
    }
}
