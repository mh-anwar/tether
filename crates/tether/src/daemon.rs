use anyhow::Result;
use tether_core::Service;
use tracing::info;

use crate::config::Config;

pub async fn run(cfg: Config) -> Result<()> {
    let service = Service::start(cfg.core()).await?;
    info!("running as \"{}\"", service.identity().device_name);
    if cfg.mirror_notifications {
        tokio::spawn(crate::notify::mirror(service.clone()));
    }
    let path = crate::socket_path();
    tokio::select! {
        r = crate::api::serve(service, &path) => r?,
        _ = tokio::signal::ctrl_c() => {}
    }
    let _ = std::fs::remove_file(path);
    Ok(())
}
