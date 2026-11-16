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
    media_test_control(sim.clone());
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

/// Opt-in file control for testing/media: each `*.json` packet dropped into `$FAKE_PHONE_CONTROL`
/// is sent (with the file at `testPayloadPath` as its payload), every received packet is appended
/// to `received.jsonl`, and the newest received payload is written to `payload.bin`.
fn media_test_control(sim: Simulator) {
    let Some(dir) = std::env::var_os("FAKE_PHONE_CONTROL").map(PathBuf::from) else {
        return;
    };
    tokio::spawn(async move {
        use std::io::Write;
        let mut logged = 0;
        loop {
            let mut paths = std::fs::read_dir(&dir)
                .map(|entries| {
                    entries
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| p.extension().is_some_and(|e| e == "json"))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            paths.sort();
            for path in paths {
                let Ok(bytes) = std::fs::read(&path) else {
                    continue;
                };
                let _ = std::fs::remove_file(&path);
                let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                    continue;
                };
                let payload_path = value
                    .as_object_mut()
                    .and_then(|v| v.remove("testPayloadPath"))
                    .and_then(|v| v.as_str().map(String::from));
                let Ok(packet) = serde_json::from_value::<Packet>(value) else {
                    continue;
                };
                match payload_path.map(std::fs::read) {
                    Some(Ok(data)) => {
                        let _ = sim.send_payload(packet, data).await;
                    }
                    Some(Err(_)) => {}
                    None => {
                        let _ = sim.send(packet).await;
                    }
                }
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
                        if !r.payload.is_empty() {
                            let _ = std::fs::write(dir.join("payload.bin"), &r.payload);
                        }
                    }
                }
                logged = received.len();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
}
