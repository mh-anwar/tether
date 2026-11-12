//! `tether daemon` runs the background service; the other subcommands talk to it.

mod api;
mod apps;
mod autostart;
mod client;
mod clipsync;
mod config;
mod daemon;
mod mprisd;
mod notify;

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
    /// Control what plays on the phone: play, pause, next, previous, or no argument to show it.
    Media { action: Option<MediaAction> },
    /// Open a phone app in its own window, by name. No name lists the apps; `screen` shows the whole
    /// phone screen. Set up once from the Apps page in the window.
    Apps { app: Option<String> },
    /// Silence a ringing call.
    Mute,
    /// Look for phones now.
    Refresh,
    /// Reach a phone by address, for networks where it can't be found automatically.
    Connect { address: String },
    /// Print events as they happen.
    Watch,
    /// Start Tether when you log in: `on`, `off`, or no argument to check.
    Autostart { state: Option<Toggle> },
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
    client::run(cli.device, cli.cmd).await
}

pub fn socket_path() -> PathBuf {
    let base = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
    base.join("tether").join("tether.sock")
}
