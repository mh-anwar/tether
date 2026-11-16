//! Real daemon, isolated filesystem and real TLS peer; no desktop session or phone required.
mod support;
use serde_json::{json, Value};
use support::{start_daemon, WAIT};
use tether_core::{
    sim::{Result, Simulator},
    Packet,
};

#[tokio::test]
async fn pairing_trust_and_reconnect() -> Result<()> {
    let api = start_daemon().await?;
    let sim = api.sim().await?;
    sim.send(Packet::new(
        "kdeconnect.battery",
        json!({"currentCharge":13}),
    ))
    .await?;
    // A reply to our ping gives an ordering barrier after the injected untrusted packet.
    assert!(api.device_call(&sim, "ping", json!({})).await.is_err());
    assert!(api.device_call(&sim, "device_data", json!({})).await?["battery"].is_null());
    sim.pair().await?;
    api.pair_state(&sim, "requested_by_peer").await?;
    api.device_call(&sim, "reject", json!({})).await?;
    sim.wait_for("kdeconnect.pair", json!({"pair":false}), 0, WAIT)
        .await?;
    api.pair_state(&sim, "unpaired").await?;
    api.device_call(&sim, "pair", json!({})).await?;
    sim.wait_for("kdeconnect.pair", json!({"pair":true}), 0, WAIT)
        .await?;
    sim.reject().await?;
    api.pair_state(&sim, "unpaired").await?;
    api.device_call(&sim, "pair", json!({})).await?;
    sim.accept().await?;
    api.pair_state(&sim, "paired").await?;
    sim.disconnect();
    api.until("devices", json!({}), |v| v[0]["connected"] == false)
        .await?;
    sim.reconnect().await?;
    api.until("devices", json!({}), |v| {
        v[0]["connected"] == true && v[0]["pair"] == "paired"
    })
    .await?;
    api.device_call(&sim, "unpair", json!({})).await?;
    api.pair_state(&sim, "unpaired").await?;
    sim.pair().await?;
    api.pair_state(&sim, "requested_by_peer").await?;
    api.device_call(&sim, "accept", json!({})).await?;
    api.pair_state(&sim, "paired").await?;
    sim.unpair().await?;
    api.pair_state(&sim, "unpaired").await?;
    Ok(())
}

#[tokio::test]
async fn discovery_and_pair_timeout() -> Result<()> {
    let api = start_daemon().await?;
    let identity = api.call("identity", json!({})).await?;
    assert_eq!(identity["tcpPort"], api.port);
    assert_eq!(identity["discoveryListening"], true);
    let sim = Simulator::new(api.dir.join("phone")).await?;
    // Unicast delivery of a broadcast-format identity is deterministic even on hosts that
    // don't loop back broadcast. The listener takes the daemon's outgoing TLS role.
    sim.discover(([127, 0, 0, 1], api.port).into()).await?;
    api.pair_state(&sim, "unpaired").await?;
    api.device_call(&sim, "pair", json!({})).await?;
    api.pair_state(&sim, "requested_by_us").await?;
    tokio::time::sleep(std::time::Duration::from_secs(36)).await;
    api.pair_state(&sim, "unpaired").await?;
    // connect asks the remote peer to dial us by sending our identity over UDP.
    let udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
    api.call("connect", json!({"address":udp.local_addr()?.to_string()}))
        .await?;
    let mut buf = [0; 65536];
    let (n, _) = tokio::time::timeout(WAIT, udp.recv_from(&mut buf)).await??;
    assert_eq!(Packet::from_line(&buf[..n])?.kind, "kdeconnect.identity");
    Ok(())
}

#[tokio::test]
async fn plugins_and_payloads() -> Result<()> {
    let api = start_daemon().await?;
    let sim = api.paired().await?;
    let device = json!({"device":sim.identity().device_id});
    api.until("device_data", device.clone(), |v| {
        v["battery"]["charge"] == 82 && v["signal"]["1"]["network_type"] == "5G"
    })
    .await?;
    api.until("contacts", device.clone(), |v| {
        v.as_array().is_some_and(|a| !a.is_empty())
    })
    .await?;
    let threads = api
        .until("threads", device.clone(), |v| {
            v.as_array().is_some_and(|a| !a.is_empty())
        })
        .await?;
    let tid = threads[0]["thread_id"].clone();
    api.device_call(&sim, "request_thread", json!({"thread_id":tid}))
        .await?;
    api.until(
        "thread",
        json!({"device":sim.identity().device_id,"thread_id":tid}),
        |v| v.as_array().is_some_and(|a| !a.is_empty()),
    )
    .await?;
    for (method, params, kind, body) in [
        ("ping", json!({}), "kdeconnect.ping", json!({})),
        (
            "ring",
            json!({}),
            "kdeconnect.findmyphone.request",
            json!({}),
        ),
        (
            "mute",
            json!({}),
            "kdeconnect.telephony.request_mute",
            json!({}),
        ),
        (
            "clipboard",
            json!({"content":"desktop clip"}),
            "kdeconnect.clipboard",
            json!({"content":"desktop clip"}),
        ),
        (
            "share_text",
            json!({"text":"hello"}),
            "kdeconnect.share.request",
            json!({"text":"hello"}),
        ),
        (
            "share_url",
            json!({"url":"https://example.org"}),
            "kdeconnect.share.request",
            json!({"url":"https://example.org"}),
        ),
        (
            "notification_reply",
            json!({"reply_id":"reply1","message":"yes"}),
            "kdeconnect.notification.reply",
            json!({"message":"yes"}),
        ),
        (
            "notification_action",
            json!({"id":"note1","action":"Open"}),
            "kdeconnect.notification.action",
            json!({"key":"note1","action":"Open"}),
        ),
        (
            "notification_dismiss",
            json!({"id":"note1"}),
            "kdeconnect.notification.request",
            json!({"cancel":"note1"}),
        ),
        (
            "sms_send",
            json!({"addresses":["+15555550101"],"body":"test SMS"}),
            "kdeconnect.sms.request",
            json!({"messageBody":"test SMS"}),
        ),
        (
            "media_action",
            json!({"player":"Spotify","action":"Next"}),
            "kdeconnect.mpris.request",
            json!({"action":"Next"}),
        ),
        (
            "media_volume",
            json!({"player":"Spotify","volume":35}),
            "kdeconnect.mpris.request",
            json!({"setVolume":35}),
        ),
        (
            "media_seek",
            json!({"player":"Spotify","position_ms":1000}),
            "kdeconnect.mpris.request",
            json!({"SetPosition":1000}),
        ),
    ] {
        let after = sim.received().len();
        api.device_call(&sim, method, params).await?;
        sim.wait_for(kind, body, after, WAIT).await?;
    }
    api.until("threads", device.clone(), |v| {
        v.as_array()
            .is_some_and(|a| a.iter().any(|m| m["body"] == "test SMS" && m["type"] == 2))
    })
    .await?;
    let notes = api
        .until("device_data", device.clone(), |v| {
            v["notifications"]
                .as_object()
                .is_some_and(|a| !a.is_empty())
        })
        .await?;
    let id = notes["notifications"]
        .as_object()
        .and_then(|m| m.keys().next())
        .ok_or("no notification")?;
    api.device_call(&sim, "notification_dismiss", json!({"id":id}))
        .await?;
    api.until("device_data", device.clone(), |v| {
        v["notifications"].get(id).is_none()
    })
    .await?;
    sim.send(Packet::new(
        "kdeconnect.telephony",
        json!({"event":"ringing","phoneNumber":"1234"}),
    ))
    .await?;
    api.until("device_data", device.clone(), |v| {
        v["call"]["state"] == "ringing"
    })
    .await?;
    sim.send(Packet::new(
        "kdeconnect.telephony",
        json!({"event":"ringing","isCancel":"true"}),
    ))
    .await?;
    api.until("device_data", device.clone(), |v| v["call"].is_null())
        .await?;
    api.until("device_data", device.clone(), |v| {
        v["notifications"]
            .as_object()
            .is_some_and(|a| a.values().any(|n| n["icon_path"].is_string()))
    })
    .await?;
    api.until("device_data", device.clone(), |v| {
        v["media"]["Spotify"]["art_path"].is_string()
    })
    .await?;
    eprintln!("stage: attachment");
    let attachment = json!({"device":sim.identity().device_id,"part_id":9001,"unique_identifier":"PART_1731300000_dog.jpg"});
    let attachment = api
        .until("sms_attachment", attachment, |v| v["path"].is_string())
        .await?;
    assert_eq!(
        std::fs::read(attachment["path"].as_str().ok_or("attachment path")?)?,
        include_bytes!("../../../testing/fake-art/photo-dog.jpg")
    );
    eprintln!("stage: desktop file");
    let source = api.dir.join("upload.txt");
    std::fs::write(&source, b"desktop payload")?;
    api.device_call(&sim, "share_file", json!({"path":source}))
        .await?;
    let received = sim
        .wait_for(
            "kdeconnect.share.request",
            json!({"filename":"upload.txt"}),
            0,
            WAIT,
        )
        .await?;
    assert_eq!(received.payload, b"desktop payload");
    assert!(received.payload_error.is_none());
    eprintln!("stage: phone file");
    sim.send_payload(
        Packet::new(
            "kdeconnect.share.request",
            json!({"filename":"download.txt"}),
        ),
        b"phone payload".to_vec(),
    )
    .await?;
    tokio::time::timeout(WAIT, async {
        loop {
            if std::fs::read(api.dir.join("downloads/download.txt"))
                .ok()
                .as_deref()
                == Some(b"phone payload")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await?;
    // Watch one API connection to assert ephemeral incoming updates, including clipboard with
    // clipboard_sync disabled (the API event remains the public contract).
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let stream = tokio::net::UnixStream::connect(api.dir.join("run/tether/tether.sock")).await?;
    let (rd, mut wr) = stream.into_split();
    wr.write_all(b"{\"id\":9,\"method\":\"identity\",\"params\":{}}\n")
        .await?;
    let mut events = BufReader::new(rd).lines();
    loop {
        let line = tokio::time::timeout(WAIT, events.next_line())
            .await??
            .ok_or("API closed")?;
        let event: Value = serde_json::from_str(&line)?;
        if event["id"] == 9 {
            break;
        }
    }
    for (kind, body, update) in [
        (
            "kdeconnect.clipboard",
            json!({"content":"phone clip"}),
            "clipboard",
        ),
        (
            "kdeconnect.share.request",
            json!({"text":"phone text"}),
            "shared_text",
        ),
        (
            "kdeconnect.share.request",
            json!({"url":"https://example.org/phone"}),
            "shared_url",
        ),
        ("kdeconnect.ping", json!({"message":"phone ping"}), "ping"),
    ] {
        eprintln!("stage: incoming {update}");
        sim.send(Packet::new(kind, body)).await?;
        tokio::time::timeout(WAIT, async {
            loop {
                let line = events.next_line().await?.ok_or("event stream closed")?;
                let event: Value = serde_json::from_str(&line)?;
                if event["update"]["kind"] == update {
                    return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(());
                }
            }
        })
        .await??;
    }
    Ok(())
}

#[tokio::test]
async fn pinned_certificate_mismatch_refused() -> Result<()> {
    let api = start_daemon().await?;
    let sim = api.paired().await?;
    sim.disconnect();
    api.until("devices", json!({}), |v| v[0]["connected"] == false)
        .await?;
    let cert = tether_core::cert::LocalCert::generate(&sim.identity().device_id)?;
    std::fs::write(api.dir.join("phone/cert.der"), cert.cert_der)?;
    std::fs::write(api.dir.join("phone/key.der"), cert.key_der)?;
    let imposter = Simulator::new(api.dir.join("phone")).await?;
    imposter.connect(([127, 0, 0, 1], api.port).into()).await?;
    // The main TLS exchange accepts self-signed certificates; Service applies the pin afterward.
    tokio::time::timeout(WAIT, async {
        while imposter.connected() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await?;
    let devices = api.call("devices", json!({})).await?;
    assert_eq!(devices[0]["connected"], false);
    assert_eq!(devices[0]["pair"], "paired");
    Ok(())
}

#[tokio::test]
async fn sftp_phone_files_when_available() -> Result<()> {
    use std::process::{Command, Stdio};
    if !std::path::Path::new("/usr/sbin/sshd").exists()
        || Command::new("sftp").arg("-h").output().is_err()
    {
        eprintln!("SKIP sftp_phone_files: sshd and sftp are required");
        return Ok(());
    }
    struct Server(std::process::Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let api = start_daemon().await?;
    let sim = api.paired().await?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut server = Server(
        Command::new("bash")
            .arg(root.join("testing/phone-files/serve.sh"))
            .arg(&api.dir)
            .arg(api.dir.join("data"))
            .arg(port.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    // The script prints its configuration before exec. Read it without blocking the async worker.
    let stdout = server.0.stdout.take().ok_or("missing sshd stdout")?;
    let config = tokio::task::spawn_blocking(move || {
        use std::io::BufRead;
        std::io::BufReader::new(stdout).lines().next().transpose()
    })
    .await??
    .ok_or("sshd script produced no config")?;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    if server.0.try_wait()?.is_some() {
        eprintln!(
            "SKIP sftp_phone_files: user-level sshd unavailable: {}",
            std::fs::read_to_string(api.dir.join("sftp/sshd.log")).unwrap_or_default()
        );
        return Ok(());
    }
    let fields: Vec<_> = config.splitn(4, ':').collect();
    assert_eq!(fields.len(), 4);
    let response = async {
        sim.wait_for("kdeconnect.sftp.request", json!({}), 0, WAIT)
            .await?;
        sim.send(Packet::new("kdeconnect.sftp",json!({"ip":fields[0],"port":port,"user":fields[2],"password":"fake-one-time","path":fields[3],"multiPaths":[fields[3]],"pathNames":["Internal storage"]}))).await
    };
    let (photos, reply) = tokio::join!(api.device_call(&sim, "photos_list", json!({})), response);
    reply?;
    let photos = photos?;
    assert!(photos["folders"].as_array().is_some_and(|folders| folders
        .iter()
        .any(|f| f["items"].as_array().is_some_and(|a| a.len() == 5))));
    // Also exercise an actual SFTP download, rather than only listing metadata.
    let path = photos["folders"]
        .as_array()
        .and_then(|a| {
            a.iter()
                .flat_map(|f| f["items"].as_array().into_iter().flatten())
                .find_map(Value::as_str)
        })
        .ok_or("no photo")?;
    let photo = api
        .device_call(&sim, "photo_get", json!({"path":path}))
        .await?;
    assert!(std::fs::metadata(photo["path"].as_str().ok_or("no local photo")?)?.len() > 1000);
    Ok(())
}

#[tokio::test]
async fn simulator_json_control() -> Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let api = start_daemon().await?;
    let sim = api.paired().await?;
    let socket = api.dir.join("control.sock");
    sim.control_socket(&socket).await?;
    let stream = tokio::net::UnixStream::connect(&socket).await?;
    let (rd, mut wr) = stream.into_split();
    let mut lines = BufReader::new(rd).lines();
    for command in [
        json!({"command":"send","packet":{"type":"kdeconnect.battery","body":{"currentCharge":47,"isCharging":true}}}),
        json!({"command":"received"}),
        json!({"command":"wait_for","type":"never.sent","timeout":5}),
    ] {
        wr.write_all(format!("{command}\n").as_bytes()).await?;
        let reply = tokio::time::timeout(WAIT, lines.next_line())
            .await??
            .ok_or("control closed")?;
        let reply: Value = serde_json::from_str(&reply)?;
        if command["command"] == "wait_for" {
            assert!(reply["error"].is_string());
        } else {
            assert!(reply.get("result").is_some());
        }
    }
    api.until(
        "device_data",
        json!({"device":sim.identity().device_id}),
        |v| v["battery"]["charge"] == 47,
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn mdns_discovery_uses_tls_and_pairing() -> Result<()> {
    let api = support::start_daemon_with_mdns(true).await?;
    let udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
    let query = tether_core::dns::query(&[(tether_core::mdns::SERVICE, 12)], true);
    let mut buf = [0; 9000];
    // Startup and the first membership refresh run independently of API startup.
    let response = tokio::time::timeout(WAIT, async {
        loop {
            udp.send_to(&query, "127.0.0.1:5353").await?;
            if let Ok(Ok((n, _))) = tokio::time::timeout(
                std::time::Duration::from_millis(200),
                udp.recv_from(&mut buf),
            )
            .await
            {
                if let Some(m) = tether_core::mdns::parse(&buf[..n]) {
                    if m.response {
                        return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(m);
                    }
                }
            }
        }
    })
    .await??;
    assert!(response
        .records
        .iter()
        .any(|r| matches!(r.data,tether_core::mdns::Data::Srv(port,_) if port == api.port)));
    let sim = Simulator::new(api.dir.join("mdns-phone")).await?;
    sim.discover_mdns("127.0.0.1:5353".parse()?).await?;
    api.pair_state(&sim, "unpaired").await?;
    sim.pair().await?;
    api.pair_state(&sim, "requested_by_peer").await?;
    api.device_call(&sim, "accept", json!({})).await?;
    api.pair_state(&sim, "paired").await?;
    assert_eq!(api.call("settings_get", json!({})).await?["mdns"], true);
    assert_eq!(
        api.call("settings_set", json!({"mdns":false})).await?["restart"],
        true
    );
    let saved: Value =
        serde_json::from_slice(&std::fs::read(api.dir.join("cfg/tether/config.json"))?)?;
    assert_eq!(saved["mdns"], false);
    Ok(())
}

#[tokio::test]
async fn unauthenticated_connections_are_limited_and_expire() -> Result<()> {
    use tokio::io::AsyncReadExt;
    let api = start_daemon().await?;
    let mut held = Vec::new();
    for _ in 0..8 {
        held.push(tokio::net::TcpStream::connect(("127.0.0.1", api.port)).await?);
    }
    // An accepted connection keeps its slot throughout plaintext identity, TLS and secure identity.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let mut extra = tokio::net::TcpStream::connect(("127.0.0.1", api.port)).await?;
    let mut b = [0];
    assert_eq!(tokio::time::timeout(WAIT, extra.read(&mut b)).await??, 0);
    // All eight stalled identity exchanges must close at the ten-second deadline.
    for tcp in &mut held {
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(12), tcp.read(&mut b)).await??,
            0
        );
    }
    let sim = api.sim().await?;
    api.pair_state(&sim, "unpaired").await?;
    Ok(())
}

#[tokio::test]
async fn oversized_api_request_closes_only_its_connection() -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let api = start_daemon().await?;
    let mut stream =
        tokio::net::UnixStream::connect(api.dir.join("run/tether/tether.sock")).await?;
    let bytes = vec![b'x'; 1024 * 1024 + 1];
    let _ = stream.write_all(&bytes).await;
    let mut b = [0];
    assert_eq!(tokio::time::timeout(WAIT, stream.read(&mut b)).await??, 0);
    api.call("identity", json!({})).await?;
    Ok(())
}

#[tokio::test]
async fn phone_without_sms_is_not_ready_via_both_apis() -> Result<()> {
    let api = start_daemon().await?;
    let sim = Simulator::with_capabilities(
        api.dir.join("phone"),
        tether_core::plugins::outgoing_capabilities(),
        tether_core::plugins::incoming_capabilities()
            .into_iter()
            .filter(|c| c != "kdeconnect.sms.messages")
            .collect(),
    )
    .await?;
    sim.connect(([127, 0, 0, 1], api.port).into()).await?;
    api.pair_state(&sim, "unpaired").await?;
    sim.pair().await?;
    api.pair_state(&sim, "requested_by_peer").await?;
    api.device_call(&sim, "accept", json!({})).await?;
    api.pair_state(&sim, "paired").await?;
    let data = api.device_call(&sim, "device_data", json!({})).await?;
    let devices = api.call("devices", json!({})).await?;
    for data in [&data, &devices[0]] {
        let texts = data["readiness"]
            .as_array()
            .ok_or("no checklist")?
            .iter()
            .find(|f| f["id"] == "texts")
            .ok_or("no texts")?;
        assert_eq!(texts["ready"], false);
        assert!(!data["outgoing_capabilities"]
            .as_array()
            .ok_or("no capabilities")?
            .contains(&json!("kdeconnect.sms.messages")));
    }
    sim.disconnect();
    Ok(())
}
