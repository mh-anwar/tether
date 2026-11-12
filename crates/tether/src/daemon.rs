use anyhow::{Context, Result};
use tether_core::Service;
use tracing::info;

use crate::config::Config;

pub async fn run(cfg: Config) -> Result<()> {
    // Two daemons would share one identity and fight over the phone; the lock is held for the
    // process lifetime and released by the OS even if we crash.
    std::fs::create_dir_all(&cfg.data_dir)?;
    let lock =
        std::fs::File::create(cfg.data_dir.join("daemon.lock")).context("creating lock file")?;
    if lock.try_lock().is_err() {
        // Exit cleanly: a login service set to restart on failure would otherwise retry forever
        // while the window's copy runs.
        eprintln!("Tether is already running.");
        return Ok(());
    }
    let service = Service::start(cfg.core()).await?;
    info!("running as \"{}\"", service.identity().device_name);
    if cfg.mirror_notifications {
        tokio::spawn(crate::notify::mirror(service.clone()));
    }
    tokio::spawn(crate::mprisd::run(service.clone()));
    if cfg.clipboard_sync {
        crate::clipsync::start(service.clone());
    }
    crate::apps::refresh_menu_entries();
    crate::update::start(cfg.update_url.clone(), cfg.auto_update);
    let path = crate::socket_path();
    tokio::select! {
        r = crate::api::serve(service, &path) => r?,
        _ = tokio::signal::ctrl_c() => {}
    }
    let _ = std::fs::remove_file(path);
    Ok(())
}
