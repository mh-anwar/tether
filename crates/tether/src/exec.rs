//! Running helper programs and downloading files, shared by phone apps and updates. Downloads use
//! curl or wget from the system, so Tether needs no HTTP or TLS stack of its own.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use tokio::process::Command;

pub fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

pub fn which(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| is_executable(p))
}

/// Runs a helper to completion; returns whether it succeeded and stdout + stderr together.
pub async fn run(prog: &Path, args: &[&str], timeout: Duration) -> Result<(bool, String)> {
    run_input(prog, args, None, timeout).await
}

/// `run` with extra environment variables (scrcpy is told which adb to use this way).
pub async fn run_env(
    prog: &Path,
    args: &[&str],
    env: &[(&str, &std::ffi::OsStr)],
    timeout: Duration,
) -> Result<(bool, String)> {
    run_full(prog, args, env, None, timeout).await
}

/// `input` goes to the helper's stdin: secrets go there, never into arguments (visible in /proc).
pub async fn run_input(
    prog: &Path,
    args: &[&str],
    input: Option<&str>,
    timeout: Duration,
) -> Result<(bool, String)> {
    run_full(prog, args, &[], input, timeout).await
}

async fn run_full(
    prog: &Path,
    args: &[&str],
    env: &[(&str, &std::ffi::OsStr)],
    input: Option<&str>,
    timeout: Duration,
) -> Result<(bool, String)> {
    use tokio::io::AsyncWriteExt;
    let mut child = Command::new(prog)
        .args(args)
        .envs(env.iter().copied())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("running {}", prog.display()))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        let _ = stdin.write_all(format!("{text}\n").as_bytes()).await;
    }
    let out = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| anyhow!("{} took too long", prog.display()))?
        .with_context(|| format!("running {}", prog.display()))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text))
}

/// The command line that fetches `url` to `dest` (`-` for stdout), with curl or else wget.
fn fetch_cmd(url: &str, dest: &str) -> Result<(PathBuf, Vec<String>)> {
    if let Some(curl) = which("curl") {
        let args = ["-fsSL", "-o", dest, url];
        return Ok((curl, args.map(String::from).to_vec()));
    }
    if let Some(wget) = which("wget") {
        let args = ["-q", "-O", dest, url];
        return Ok((wget, args.map(String::from).to_vec()));
    }
    bail!("this computer has neither curl nor wget")
}

/// Downloads `url` to `dest`. A failed download leaves no file behind.
pub async fn download(url: &str, dest: &Path, timeout: Duration) -> Result<()> {
    let (prog, args) = fetch_cmd(url, &dest.to_string_lossy())?;
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (ok, _) = run(&prog, &args, timeout).await?;
    if !ok {
        let _ = std::fs::remove_file(dest);
        bail!("check the internet connection");
    }
    Ok(())
}

/// Fetches a small text document such as a JSON API reply. None when the server answers with an
/// error (404, for example a private repository); Err when it can't be reached at all.
pub async fn fetch_text(url: &str, timeout: Duration) -> Result<Option<String>> {
    let (prog, args) = fetch_cmd(url, "-")?;
    let out = tokio::time::timeout(
        timeout,
        Command::new(&prog)
            .args(&args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| anyhow!("{} took too long", prog.display()))??;
    if out.status.success() {
        return Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()));
    }
    // curl -f exits 22 and wget 8 when the server answered with an error status.
    let is_curl = prog.file_name().is_some_and(|n| n == "curl");
    match out.status.code() {
        Some(22) if is_curl => Ok(None),
        Some(8) if !is_curl => Ok(None),
        _ => bail!("check the internet connection"),
    }
}
