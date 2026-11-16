use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};
use tether_core::sim::{Result, Simulator};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

pub const WAIT: Duration = Duration::from_secs(8);
pub struct Daemon {
    pub dir: PathBuf,
    child: Child,
    pub port: u16,
}
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
pub async fn start_daemon() -> Result<Daemon> {
    let dir = PathBuf::from(format!(
        "/tmp/ts-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..10]
    ));
    for part in ["cfg/tether", "data", "run", "downloads"] {
        std::fs::create_dir_all(dir.join(part))?;
    }
    std::fs::write(dir.join("cfg/tether/config.json"), json!({"device_name":"Integration test", "clipboard_sync":false, "mirror_notifications":false, "auto_update":false, "downloads_dir":dir.join("downloads")}).to_string())?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let log = std::fs::File::create(dir.join("daemon.log"))?;
    let child = Command::new(env!("CARGO_BIN_EXE_tether"))
        .arg("daemon")
        .env("XDG_CONFIG_HOME", dir.join("cfg"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .env("TETHER_PORT", port.to_string())
        .env("TOKIO_WORKER_THREADS", "2")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
    let daemon = Daemon { dir, child, port };
    tokio::time::timeout(WAIT, async {
        loop {
            if daemon.call("identity", json!({})).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .map_err(|e| {
        format!(
            "daemon startup: {e}: {}",
            std::fs::read_to_string(daemon.dir.join("daemon.log")).unwrap_or_default()
        )
    })?;
    Ok(daemon)
}
impl Daemon {
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        tokio::time::timeout(WAIT, async {
            let mut stream = UnixStream::connect(self.dir.join("run/tether/tether.sock")).await?;
            stream
                .write_all(
                    format!("{}\n", json!({"id":1,"method":method,"params":params})).as_bytes(),
                )
                .await?;
            let mut lines = BufReader::new(stream).lines();
            while let Some(line) = lines.next_line().await? {
                let reply: Value = serde_json::from_str(&line)?;
                if reply["id"] == 1 {
                    if let Some(error) = reply.get("error") {
                        return Err(format!("{method}: {error}").into());
                    }
                    return Ok(reply["result"].clone());
                }
            }
            Err("API closed".into())
        })
        .await?
    }
    pub async fn device_call(
        &self,
        sim: &Simulator,
        method: &str,
        mut params: Value,
    ) -> Result<Value> {
        params["device"] = json!(sim.identity().device_id);
        self.call(method, params).await
    }
    pub async fn until(
        &self,
        method: &str,
        params: Value,
        check: impl Fn(&Value) -> bool,
    ) -> Result<Value> {
        tokio::time::timeout(WAIT, async {
            loop {
                let result = self.call(method, params.clone()).await?;
                if check(&result) {
                    return Ok(result);
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?
    }
    pub async fn pair_state(&self, sim: &Simulator, state: &str) -> Result<Value> {
        self.until("devices", json!({}), |v| {
            v.as_array().is_some_and(|a| {
                a.iter()
                    .any(|d| d["id"] == sim.identity().device_id && d["pair"] == state)
            })
        })
        .await
    }
    pub async fn sim(&self) -> Result<Simulator> {
        let sim = Simulator::new(self.dir.join("phone")).await?;
        sim.connect(([127, 0, 0, 1], self.port).into()).await?;
        self.pair_state(&sim, "unpaired").await?;
        Ok(sim)
    }
    pub async fn paired(&self) -> Result<Simulator> {
        let sim = self.sim().await?;
        sim.pair().await?;
        self.pair_state(&sim, "requested_by_peer").await?;
        self.device_call(&sim, "accept", json!({})).await?;
        self.pair_state(&sim, "paired").await?;
        Ok(sim)
    }
}
