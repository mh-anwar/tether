//! Stand-in phone. Optional third argument: JSON-lines control socket.
use std::{path::PathBuf, time::Duration};
use tether_core::sim::Simulator;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args().skip(1);
    let addr = args
        .next()
        .unwrap_or_else(|| "127.0.0.1:1716".into())
        .parse()?;
    let dir = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "/tmp/tether-fake-phone".into()),
    );
    let sim = Simulator::new(&dir).await?;
    if let Some(socket) = args.next() {
        sim.control_socket(socket).await?;
    }
    loop {
        if sim.connect(addr).await.is_ok() {
            if !dir.join("paired").exists() {
                sim.pair().await?;
                println!("asked to pair; run `tether accept`");
            }
            while sim.connected() {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}
