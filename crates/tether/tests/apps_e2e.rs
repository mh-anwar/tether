//! Phone apps in their own windows, through the real daemon's API with the stand-in adb and scrcpy
//! from `testing/apps` (no phone, no display). The stand-ins keep their state as files in a folder and
//! log every call, so the test checks both what the window is told and what scrcpy was asked to do.
mod support;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use support::{start_daemon_with, Daemon};
use tether_core::sim::Result;

const CALC: &str = "com.sec.android.app.popupcalculator";

struct Fake {
    dir: PathBuf,
}
impl Fake {
    fn new() -> Result<Self> {
        let dir = std::env::temp_dir().join(format!(
            "tether-apps-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..10]
        ));
        std::fs::create_dir_all(&dir)?;
        Ok(Fake { dir })
    }
    fn calls(&self) -> String {
        std::fs::read_to_string(self.dir.join("calls.log")).unwrap_or_default()
    }
    fn count(&self, needle: &str) -> usize {
        self.calls().lines().filter(|l| l.contains(needle)).count()
    }
}
impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn stand_in(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testing/apps")
        .join(name)
}

async fn daemon(fake: &Fake, adb: &Path, scrcpy: &Path) -> Result<Daemon> {
    start_daemon_with(
        false,
        &[
            ("FAKE_APPS_DIR", fake.dir.as_os_str()),
            ("TETHER_ADB", adb.as_os_str()),
            ("TETHER_SCRCPY", scrcpy.as_os_str()),
        ],
    )
    .await
}

async fn status(api: &Daemon, check: impl Fn(&Value) -> bool) -> Result<Value> {
    api.until("apps_status", json!({}), check).await
}

#[tokio::test]
async fn app_windows_over_wireless_debugging() -> Result<()> {
    let fake = Fake::new()?;
    let adb = stand_in("adb");
    let api = daemon(&fake, &adb, &stand_in("scrcpy")).await?;

    let st = api.call("apps_status", json!({})).await?;
    assert_eq!(st["adb"], true);
    assert_eq!(st["scrcpy"], true);
    assert_eq!(st["scrcpyTooOld"], false);
    assert!(st["device"].is_null());
    assert_eq!(st["windows"], json!([]));
    assert!(api
        .call("apps_open", json!({"package": CALC}))
        .await
        .is_err());

    // Pairing with the code from the phone's "Pair device with pairing code" screen.
    let addr = "127.0.0.1:41234";
    let wrong = api
        .call("apps_pair_code", json!({"address": addr, "code": "000000"}))
        .await;
    assert!(format!("{wrong:?}").contains("didn't match"), "{wrong:?}");
    api.call("apps_pair_code", json!({"address": addr, "code": "123456"}))
        .await?;
    let st = status(&api, |s| !s["device"].is_null()).await?;
    assert_eq!(st["device"]["serial"], "127.0.0.1:37199");
    assert_eq!(st["device"]["android"], "15");
    assert_eq!(st["ownWindows"], true);

    let list = api.call("apps_list", json!({})).await?;
    assert!(list.as_array().is_some_and(|l| l
        .iter()
        .any(|a| a["package"] == CALC && a["name"] == "Calculator")));

    // One app, one window of its own on a virtual display; scrcpy uses Tether's adb.
    let r = api
        .call("apps_open", json!({"package": CALC, "title": "Calculator"}))
        .await?;
    assert_eq!(r["opened"], true);
    assert_eq!(r["sharedScreen"], false);
    assert_eq!(
        fake.count(&format!(
            "--window-title=Calculator --new-display --start-app={CALC}"
        )),
        1
    );
    assert!(fake
        .calls()
        .contains(&format!("scrcpy-adb {}", adb.display())));
    let r = api
        .call("apps_open", json!({"package": CALC, "title": "Calculator"}))
        .await?;
    assert_eq!(r["alreadyOpen"], true);
    assert_eq!(fake.count(&format!("--start-app={CALC}")), 1);
    let st = api.call("apps_status", json!({})).await?;
    assert_eq!(
        st["windows"],
        json!([{"package": CALC, "title": "Calculator"}])
    );

    // A phone that can't make a virtual display: the error says why, and no window is left behind.
    let broken = api
        .call(
            "apps_open",
            json!({"package": "com.example.broken", "title": "Broken"}),
        )
        .await;
    assert!(format!("{broken:?}").contains("Android 10"), "{broken:?}");
    assert_eq!(
        api.call("apps_windows", json!({}))
            .await?
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    assert_eq!(
        api.call("apps_close", json!({"package": CALC})).await?["closed"],
        1
    );
    status(&api, |s| s["windows"] == json!([])).await?;

    // The phone goes away and comes back on Android 9: no virtual displays, so apps start on the
    // phone's own screen and share the one "Phone screen" window.
    std::fs::write(fake.dir.join("sdk"), "28")?;
    std::fs::remove_file(fake.dir.join("connected"))?;
    status(&api, |s| s["device"].is_null()).await?;
    std::fs::write(fake.dir.join("connected"), "")?;
    let st = status(&api, |s| !s["device"].is_null()).await?;
    assert_eq!(st["device"]["android"], "9");
    assert_eq!(st["ownWindows"], false);

    let r = api
        .call("apps_open", json!({"package": CALC, "title": "Calculator"}))
        .await?;
    assert_eq!(r["sharedScreen"], true);
    let calls = fake.calls();
    let last = calls
        .lines()
        .rev()
        .find(|l| l.starts_with("scrcpy -s"))
        .unwrap_or_default();
    assert!(last.contains("--window-title=Phone screen"), "{last}");
    assert!(last.contains(&format!("--start-app={CALC}")), "{last}");
    assert!(!last.contains("--new-display"), "{last}");
    let clock = "com.sec.android.app.clockpackage";
    let r = api
        .call("apps_open", json!({"package": clock, "title": "Clock"}))
        .await?;
    assert_eq!(r["sharedScreen"], true);
    assert_eq!(fake.count(&format!("shell monkey -p {clock}")), 1);
    assert_eq!(fake.count("--start-app="), 3);
    assert_eq!(
        api.call("apps_windows", json!({})).await?,
        json!([{"package": null, "title": "Phone screen"}])
    );
    assert_eq!(api.call("apps_close", json!({})).await?["closed"], 1);
    status(&api, |s| s["windows"] == json!([])).await?;
    Ok(())
}

#[tokio::test]
async fn missing_or_old_helpers_degrade() -> Result<()> {
    let fake = Fake::new()?;
    let missing = fake.dir.join("not-installed");

    // No scrcpy: the window offers the download or the install command, nothing tries to run it.
    let api = daemon(&fake, &stand_in("adb"), &missing).await?;
    let st = api.call("apps_status", json!({})).await?;
    assert_eq!(st["scrcpy"], false);
    assert_eq!(st["adb"], true);
    assert!(st.get("installHint").is_some());
    assert!(st.get("canDownload").is_some());
    assert!(api.call("apps_list", json!({})).await.is_err());
    assert_eq!(fake.count("scrcpy"), 0);
    drop(api);

    // No adb either: still a clean status, no phone.
    let api = daemon(&fake, &missing, &missing).await?;
    let st = api.call("apps_status", json!({})).await?;
    assert_eq!(
        (st["adb"].clone(), st["scrcpy"].clone()),
        (json!(false), json!(false))
    );
    assert!(st["device"].is_null());
    drop(api);

    // A distro's scrcpy 2.x mirrors the screen but can't open apps in their own windows.
    std::fs::write(fake.dir.join("version"), "2.4")?;
    let api = daemon(&fake, &stand_in("adb"), &stand_in("scrcpy")).await?;
    let st = api.call("apps_status", json!({})).await?;
    assert_eq!(st["scrcpy"], true);
    assert_eq!(st["scrcpyTooOld"], true);
    Ok(())
}
