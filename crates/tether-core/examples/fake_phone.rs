//! Stand-in phone. Optional third argument: JSON-lines control socket.
use std::{path::PathBuf, time::Duration};
use tether_core::{sim::Simulator, Packet};

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
    file_control(sim.clone());
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

/// Opt-in file control for testing/computer/test_computer.py: packets appended to
/// `$FAKE_COMPUTER_DIR/send.jsonl` are sent, and every packet the phone receives is appended to
/// `received.jsonl`. The JSON control socket does the same for tests that can hold a socket.
fn file_control(sim: Simulator) {
    let Some(dir) = std::env::var_os("FAKE_COMPUTER_DIR").map(PathBuf::from) else {
        return;
    };
    tokio::spawn(async move {
        use std::io::Write;
        let (mut sent, mut logged) = (0, 0);
        loop {
            if let Ok(text) = std::fs::read_to_string(dir.join("send.jsonl")) {
                for line in text.lines().skip(sent) {
                    if let Ok(packet) = Packet::from_line(line.as_bytes()) {
                        let _ = sim.send(packet).await;
                    }
                }
                sent = text.lines().count();
            }
            let received = sim.received();
            logged = logged.min(received.len());
            if received.len() > logged {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dir.join("received.jsonl"))
                {
                    for r in &received[logged..] {
                        let _ = writeln!(
                            f,
                            "{}",
                            serde_json::to_string(&r.packet).unwrap_or_default()
                        );
                    }
                }
                logged = received.len();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
}
