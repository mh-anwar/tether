//! `tether daemon` runs the background service; the other subcommands talk to it.

mod api;
mod apps;
mod autostart;
mod calls;
mod client;
mod clipsync;
mod computer;
mod config;
mod daemon;
mod desktopmedia;
mod exec;
mod flatpak;
mod mdns_probe;
mod mprisd;
mod notify;
mod phonefiles;
mod received;
mod remoteinput;
mod schedule;
mod sendnotifications;
mod tray;
mod update;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "tether", version, about = "Your phone, on your Linux desktop")]
struct Cli {
    /// Device id or the start of its name. Defaults to the only paired device.
    #[arg(short, long, global = true)]
    device: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the background service.
    Daemon,
    /// List phones that are nearby or paired.
    Devices,
    /// Ask a phone to pair. Confirm the code on both screens.
    Pair,
    /// Accept a pairing request from a phone.
    Accept,
    /// Decline a pairing request.
    Reject,
    /// Forget a paired phone.
    Unpair,
    /// Send a ping.
    Ping,
    /// Make the phone ring at full volume.
    Ring,
    /// Show battery, signal and notifications.
    Status,
    /// List text conversations.
    Threads,
    /// Show one conversation.
    Thread { thread_id: i64 },
    /// Send a text message.
    Sms {
        /// Phone number; repeat for a group message.
        #[arg(short, long = "to", required = true)]
        to: Vec<String>,
        message: String,
    },
    /// Send a file or a link to the phone.
    Share { target: String },
    /// Copy text to the phone's clipboard.
    Clip { text: String },
    /// Type into the active text field on the phone.
    Type { text: String },
    /// Control what plays on the phone: play, pause, next, previous, or no argument to show it.
    Media { action: Option<MediaAction> },
    /// Open a phone app in its own window, by name. No name lists the apps; `screen` shows the whole
    /// phone screen; `doctor` explains why the phone isn't found. Set up once from the Apps page.
    Apps { app: Option<String> },
    /// Silence a ringing call.
    Mute,
    /// Calls through this computer over Bluetooth. No argument shows the call. `answer`, `decline`,
    /// `hangup`, `pickup` (answer if ringing, else hang up: one key for both), `mute` (the
    /// microphone off, or back on), `here` (sound on this computer), `keys 1#` (keypad), or a number
    /// to call it.
    #[command(verbatim_doc_comment)]
    Call { args: Vec<String> },
    /// Look for phones now.
    Refresh,
    /// Reach a phone by address, for networks where it can't be found automatically.
    Connect { address: String },
    /// Print events as they happen.
    Watch,
    /// Start Tether when you log in: `on`, `off`, or no argument to check.
    Autostart { state: Option<Toggle> },
    /// Check for a newer Tether and install it.
    Update,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum MediaAction {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Toggle {
    On,
    Off,
}

fn main() -> Result<()> {
    phonefiles::answer_askpass();
    let cli = Cli::parse();
    if let Cmd::Daemon = cli.cmd {
        clipsync::guess_display_env();
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(cli))
}

async fn run(cli: Cli) -> Result<()> {
    if let Cmd::Daemon = cli.cmd {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "info".into()),
            )
            .init();
        return daemon::run(config::Config::load()?).await;
    }
    if let Cmd::Autostart { state } = cli.cmd {
        match state {
            Some(Toggle::On) => {
                autostart::enable()?;
                println!("Tether will start when you log in.");
            }
            Some(Toggle::Off) => {
                autostart::disable()?;
                println!("Tether won't start at login any more.");
            }
            None if autostart::is_enabled() => println!("On: Tether starts when you log in."),
            None => println!("Off. Turn it on with `tether autostart on`."),
        }
        return Ok(());
    }
    // Launched from a phone-app menu entry there is no terminal to print to; say it on screen.
    let from_menu = matches!(cli.cmd, Cmd::Apps { .. })
        && !std::io::IsTerminal::is_terminal(&std::io::stdout());
    let result = client::run(cli.device, cli.cmd).await;
    if let (true, Err(e)) = (from_menu, &result) {
        let _ = notify_rust::Notification::new()
            .appname("Tether")
            .summary("Couldn't open the phone app")
            .body(&format!("{e}"))
            .show_async()
            .await;
    }
    result
}

/// Each `flatpak run` gets a private `/run/user/<uid>`; only `app/<app id>` inside it is shared
/// between instances, so the window and a daemon started at login meet there. tether-app mirrors this.
pub fn socket_path() -> PathBuf {
    let base = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
    match std::env::var("FLATPAK_ID").ok().filter(|id| !id.is_empty()) {
        Some(id) => base.join("app").join(id).join("tether.sock"),
        None => base.join("tether").join("tether.sock"),
    }
}
