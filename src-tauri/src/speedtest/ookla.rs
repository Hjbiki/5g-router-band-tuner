//! Optional provider: the official Ookla Speedtest CLI.
//!
//! Nothing is downloaded until the user picks Ookla and accepts its terms in the app. The app
//! never ships the CLI: on first use it downloads Ookla's own Windows x64 zip, checks it against
//! the pinned SHA-256, and extracts `speedtest.exe` into `<app data>/tools/ookla/`. The exe is
//! hashed again before every run. See docs/SPEEDTEST.md for how to update the pin.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::watch;

use super::{Live, SpeedPhase};

pub const VERSION: &str = "1.2.0";
pub const ZIP_URL: &str = "https://install.speedtest.net/app/cli/ookla-speedtest-1.2.0-win64.zip";
pub const ZIP_SHA256: &str = "13e3d888b845d301a556419e31f14ab9bff57e3f06089ef2fd3bdc9ba6841efa";
pub const EXE_SHA256: &str = "c73c4ac2a7cdc48829fe0f9d7ec91ca41c8b434490d4eb3205aad69c7aa3b921";
/// The pinned zip is under 1 MB; anything far bigger is not it.
const MAX_ZIP_BYTES: usize = 20 * 1024 * 1024;
const MAX_EXE_BYTES: u64 = 50 * 1024 * 1024;
pub const RUN_TIMEOUT: Duration = Duration::from_secs(120);

pub const EULA_URL: &str = "https://www.speedtest.net/about/eula";
pub const PRIVACY_URL: &str = "https://www.speedtest.net/about/privacy";

/// What the CLI measured, in the app's units.
#[derive(Debug, Clone, PartialEq)]
pub struct OoklaMeasured {
    pub idle_ping_ms: Option<f64>,
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    pub ping_down_ms: Option<f64>,
    pub ping_up_ms: Option<f64>,
    pub bytes_used: u64,
    pub server: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum OoklaError {
    Cancelled,
    /// Download, hash or extraction failed.
    Install(String),
    /// The CLI couldn't be started at all, so it measured nothing.
    NotStarted(String),
    /// The CLI ran (and may have used data) but gave no result.
    Failed(String),
    TimedOut,
}

pub fn exe_path(data_dir: &Path) -> PathBuf {
    data_dir.join("tools").join("ookla").join("speedtest.exe")
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The installed exe is the pinned build.
pub fn is_installed(exe: &Path, exe_sha: &str) -> bool {
    std::fs::read(exe).is_ok_and(|b| sha256_hex(&b) == exe_sha)
}

/// The installed exe, checked and held open so nothing can replace it before it starts. Windows
/// lets other programs read it meanwhile, which is what starting it needs.
fn open_pinned(exe: &Path, exe_sha: &str) -> Option<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 1;
        options.share_mode(FILE_SHARE_READ);
    }
    let mut file = options.open(exe).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    (sha256_hex(&bytes) == exe_sha).then_some(file)
}

/// Windows loads DLLs from the exe's own folder first, and the hash only covers the exe. Nothing
/// the app puts in that folder has these kinds.
fn has_planted_file(dir: &Path, exe: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        let path = e.path();
        let ext = path
            .extension()
            .map(|x| x.to_string_lossy().to_ascii_lowercase());
        path != exe && matches!(ext.as_deref(), Some("dll" | "exe" | "local" | "manifest"))
    })
}

/// Pulls `speedtest.exe` out of the zip, checking the zip's and the exe's hashes.
pub fn extract(zip_bytes: &[u8], zip_sha: &str, exe_sha: &str) -> Result<Vec<u8>, String> {
    let got = sha256_hex(zip_bytes);
    if got != zip_sha {
        return Err(format!("download hash mismatch ({got})"));
    }
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| e.to_string())?;
    let mut f = z.by_name("speedtest.exe").map_err(|e| e.to_string())?;
    let mut exe = Vec::new();
    f.by_ref()
        .take(MAX_EXE_BYTES)
        .read_to_end(&mut exe)
        .map_err(|e| e.to_string())?;
    if sha256_hex(&exe) != exe_sha {
        return Err("speedtest.exe hash mismatch".into());
    }
    Ok(exe)
}

/// Downloads, verifies and installs the CLI. Returns the exe path.
pub async fn install(
    url: &str,
    zip_sha: &str,
    exe_sha: &str,
    exe: &Path,
    mut cancel: watch::Receiver<bool>,
) -> Result<(), OoklaError> {
    let c = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| OoklaError::Install(e.to_string()))?;
    let fetch = async {
        let mut r = c
            .get(url)
            .send()
            .await
            .map_err(|e| e.without_url().to_string())?;
        if !r.status().is_success() {
            return Err(format!("HTTP {}", r.status().as_u16()));
        }
        let mut buf = Vec::new();
        while let Some(chunk) = r.chunk().await.map_err(|e| e.without_url().to_string())? {
            buf.extend_from_slice(&chunk);
            if buf.len() > MAX_ZIP_BYTES {
                return Err("download is too large".to_string());
            }
        }
        Ok(buf)
    };
    let zip_bytes = tokio::select! {
        r = fetch => r.map_err(OoklaError::Install)?,
        _ = cancel.wait_for(|c| *c) => return Err(OoklaError::Cancelled),
    };
    let bytes = extract(&zip_bytes, zip_sha, exe_sha).map_err(OoklaError::Install)?;
    let dir = exe.parent().expect("exe has a folder");
    std::fs::create_dir_all(dir).map_err(|e| OoklaError::Install(e.to_string()))?;
    let tmp = exe.with_extension("exe.part");
    std::fs::write(&tmp, &bytes).map_err(|e| OoklaError::Install(e.to_string()))?;
    std::fs::rename(&tmp, exe).map_err(|e| OoklaError::Install(e.to_string()))?;
    Ok(())
}

#[derive(Deserialize)]
struct Latency {
    iqm: Option<f64>,
}

#[derive(Deserialize)]
struct Ping {
    latency: Option<f64>,
    progress: Option<f64>,
}

#[derive(Deserialize)]
struct Transfer {
    /// Bytes per second.
    bandwidth: Option<f64>,
    bytes: Option<u64>,
    latency: Option<Latency>,
    progress: Option<f64>,
}

#[derive(Deserialize)]
struct Server {
    name: Option<String>,
    location: Option<String>,
}

#[derive(Deserialize)]
struct Output {
    #[serde(rename = "type")]
    kind: Option<String>,
    ping: Option<Ping>,
    download: Option<Transfer>,
    upload: Option<Transfer>,
    server: Option<Server>,
    message: Option<String>,
    level: Option<String>,
    error: Option<String>,
}

/// Why a run gave no result: the CLI's own error lines (JSON log lines on either stream, or
/// plain text on stderr) and the test it had reached, e.g. "during the upload test: Cannot
/// read from socket".
pub fn failure_detail(stdout: &str, stderr: &str) -> String {
    let mut errors: Vec<String> = Vec::new();
    let mut plain: Vec<String> = Vec::new();
    let mut reached = None;
    for line in stdout.lines().chain(stderr.lines()) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Output>(line) {
            Ok(o) => {
                if let Some(k @ ("ping" | "download" | "upload")) = o.kind.as_deref() {
                    reached = Some(k.to_string());
                }
                let is_error = o.error_level() || o.kind.as_deref() == Some("error");
                let msg = o.error.or(o.message);
                if let (true, Some(m)) = (is_error, msg) {
                    if !errors.contains(&m) {
                        errors.push(m);
                    }
                }
            }
            Err(_) => {
                if stderr.contains(line) && plain.len() < 3 {
                    plain.push(line.to_string());
                }
            }
        }
    }
    let cause = if !errors.is_empty() {
        errors.join("; ")
    } else if !plain.is_empty() {
        plain.join(" ")
    } else {
        "no result in the CLI output".to_string()
    };
    match reached {
        Some(k) => format!("during the {k} test: {cause}"),
        None => cause,
    }
}

impl Output {
    fn error_level(&self) -> bool {
        matches!(self.level.as_deref(), Some("error") | Some("fatal"))
    }
}

/// Folds the CLI's interim lines into what the progress bars and live numbers show.
#[derive(Debug, Default)]
pub struct Tracker {
    live: Live,
    down_bytes: u64,
    up_bytes: u64,
}

impl Tracker {
    /// `None` for lines that say nothing about progress, including the final result.
    pub fn line(&mut self, line: &str) -> Option<(SpeedPhase, f64, u64, Live)> {
        let o = serde_json::from_str::<Output>(line.trim()).ok()?;
        let (phase, fraction) = match o.kind.as_deref()? {
            "ping" => {
                let p = o.ping?;
                self.live.idle_ping_ms = p.latency.or(self.live.idle_ping_ms);
                (SpeedPhase::IdlePing, p.progress)
            }
            "download" => {
                let d = o.download?;
                self.live.download_mbps = mbps(d.bandwidth).or(self.live.download_mbps);
                let lat = d.latency.as_ref().and_then(|l| l.iqm);
                self.live.ping_down_ms = lat.or(self.live.ping_down_ms);
                self.down_bytes = d.bytes.unwrap_or(self.down_bytes);
                (SpeedPhase::Download, d.progress)
            }
            "upload" => {
                let u = o.upload?;
                self.live.upload_mbps = mbps(u.bandwidth).or(self.live.upload_mbps);
                let lat = u.latency.as_ref().and_then(|l| l.iqm);
                self.live.ping_up_ms = lat.or(self.live.ping_up_ms);
                self.up_bytes = u.bytes.unwrap_or(self.up_bytes);
                (SpeedPhase::Upload, u.progress)
            }
            _ => return None,
        };
        Some((
            phase,
            fraction.unwrap_or(0.0).clamp(0.0, 1.0),
            self.down_bytes + self.up_bytes,
            self.live.clone(),
        ))
    }
}

fn mbps(bytes_per_s: Option<f64>) -> Option<f64> {
    bytes_per_s.filter(|b| *b > 0.0).map(|b| b / 125_000.0)
}

/// Parses `--format=json` output. Only the measured values are kept: the ISP, IP addresses and
/// the result URL in the output are dropped here.
pub fn parse(stdout: &str) -> Result<OoklaMeasured, String> {
    // The result is the last JSON object line; earlier lines can be log messages.
    let mut last_msg = None;
    for line in stdout.lines().rev() {
        let Ok(o) = serde_json::from_str::<Output>(line.trim()) else {
            continue;
        };
        if o.kind.as_deref() == Some("result") {
            let d = o.download.as_ref();
            let u = o.upload.as_ref();
            let server = o.server.and_then(|s| match (s.name, s.location) {
                (Some(n), Some(l)) if !l.is_empty() => Some(format!("{n}, {l}")),
                (n, _) => n,
            });
            return Ok(OoklaMeasured {
                idle_ping_ms: o.ping.and_then(|p| p.latency),
                download_mbps: mbps(d.and_then(|d| d.bandwidth)),
                upload_mbps: mbps(u.and_then(|u| u.bandwidth)),
                ping_down_ms: d.and_then(|d| d.latency.as_ref()?.iqm),
                ping_up_ms: u.and_then(|u| u.latency.as_ref()?.iqm),
                bytes_used: d.and_then(|d| d.bytes).unwrap_or(0)
                    + u.and_then(|u| u.bytes).unwrap_or(0),
                server,
            });
        }
        if last_msg.is_none() {
            last_msg = o.message;
        }
    }
    Err(last_msg.unwrap_or_else(|| "no result in the CLI output".into()))
}

/// Runs the CLI with no console window, a timeout and cancel. Interim values go to `progress` as
/// the CLI reports them.
pub async fn run(
    exe: &Path,
    exe_sha: &str,
    timeout: Duration,
    mut cancel: watch::Receiver<bool>,
    progress: &(dyn Fn(SpeedPhase, f64, u64, &Live) + Send + Sync),
) -> Result<OoklaMeasured, OoklaError> {
    let exe_owned = exe.to_path_buf();
    let sha = exe_sha.to_string();
    let pinned = tokio::task::spawn_blocking(move || open_pinned(&exe_owned, &sha))
        .await
        .unwrap_or(None);
    if pinned.is_none() {
        return Err(OoklaError::Install(
            "speedtest.exe is missing or changed".into(),
        ));
    }
    let folder = exe.parent().expect("exe has a folder");
    if has_planted_file(folder, exe) {
        return Err(OoklaError::Install(
            "an unexpected file is next to speedtest.exe".into(),
        ));
    }
    let mut cmd = tokio::process::Command::new(exe);
    cmd.current_dir(folder);
    // jsonl: one line per event, so the numbers can be shown live and a run that stops halfway
    // still says where and why.
    cmd.args([
        "--accept-license",
        "--accept-gdpr",
        "--format=jsonl",
        "--progress=yes",
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| OoklaError::NotStarted(e.to_string()))?;
    drop(pinned);
    let out_pipe = child.stdout.take().expect("stdout is piped");
    let mut err_pipe = child.stderr.take().expect("stderr is piped");
    let read = async {
        let mut stdout = String::new();
        let mut stderr = Vec::new();
        let mut tracker = Tracker::default();
        let lines = async {
            let mut split = BufReader::new(out_pipe).split(b'\n');
            while let Some(raw) = split.next_segment().await.transpose() {
                let Ok(raw) = raw else { break };
                let line = String::from_utf8_lossy(&raw);
                if let Some((phase, fraction, bytes, live)) = tracker.line(&line) {
                    progress(phase, fraction, bytes, &live);
                }
                stdout.push_str(&line);
                stdout.push('\n');
            }
        };
        let (_, _) = tokio::join!(lines, err_pipe.read_to_end(&mut stderr));
        let status = child.wait().await;
        (stdout, stderr, status)
    };
    // Dropping the future kills the process (kill_on_drop).
    let (stdout, stderr, status) = tokio::select! {
        r = tokio::time::timeout(timeout, read) => match r {
            Err(_) => return Err(OoklaError::TimedOut),
            Ok(r) => r,
        },
        _ = cancel.wait_for(|c| *c) => return Err(OoklaError::Cancelled),
    };
    let status = status.map_err(|e| OoklaError::Failed(e.to_string()))?;
    match parse(&stdout) {
        Ok(m) => Ok(m),
        Err(_) => {
            let stderr = String::from_utf8_lossy(&stderr);
            let msg = failure_detail(&stdout, &stderr);
            let code = status
                .code()
                .map(|c| format!(" (exit {c})"))
                .unwrap_or_default();
            Err(OoklaError::Failed(
                format!("{msg}{code}").chars().take(200).collect(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hold on the exe has to stop a swap, and still let it start.
    #[cfg(windows)]
    #[test]
    fn the_pinned_exe_can_start_but_not_be_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("speedtest.exe");
        let system = std::env::var("SystemRoot").unwrap();
        std::fs::copy(format!("{system}/System32/whoami.exe"), &exe).unwrap();
        let sha = sha256_hex(&std::fs::read(&exe).unwrap());

        assert!(open_pinned(&exe, "wrong").is_none());
        let held = open_pinned(&exe, &sha).unwrap();
        assert!(std::fs::write(&exe, b"swapped").is_err());
        assert!(std::fs::rename(&exe, dir.path().join("moved.exe")).is_err());
        let run = std::process::Command::new(&exe).output().unwrap();
        assert!(run.status.success());
        drop(held);
    }

    #[test]
    fn a_dll_next_to_the_exe_is_noticed() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("speedtest.exe");
        std::fs::write(&exe, b"x").unwrap();
        std::fs::write(dir.path().join("speedtest.exe.part"), b"x").unwrap();
        assert!(!has_planted_file(dir.path(), &exe));
        std::fs::write(dir.path().join("version.dll"), b"x").unwrap();
        assert!(has_planted_file(dir.path(), &exe));
    }
    use std::io::Write;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Shaped like the CLI's real output; values and names are synthetic.
    const SAMPLE: &str = r#"{"type":"result","timestamp":"2026-10-30T10:00:00Z","ping":{"jitter":2.1,"latency":41.2,"low":38.0,"high":60.0},"download":{"bandwidth":13500000,"bytes":120000000,"elapsed":9000,"latency":{"iqm":610.5,"low":40.0,"high":900.0,"jitter":30.0}},"upload":{"bandwidth":162500,"bytes":2000000,"elapsed":12000,"latency":{"iqm":1450.0,"low":50.0,"high":2000.0,"jitter":80.0}},"packetLoss":0,"isp":"Example ISP","interface":{"internalIp":"192.0.2.10","externalIp":"198.51.100.7"},"server":{"id":1,"host":"speed.example.net","port":8080,"name":"Example Net","location":"Riyadh","country":"Saudi Arabia","ip":"203.0.113.5"},"result":{"id":"x","url":"https://www.speedtest.net/result/c/x","persisted":true}}"#;

    #[test]
    fn parses_the_result() {
        let m = parse(SAMPLE).unwrap();
        assert_eq!(m.idle_ping_ms, Some(41.2));
        assert_eq!(m.download_mbps, Some(108.0));
        assert_eq!(m.upload_mbps, Some(1.3));
        assert_eq!(m.ping_down_ms, Some(610.5));
        assert_eq!(m.ping_up_ms, Some(1450.0));
        assert_eq!(m.bytes_used, 122_000_000);
        assert_eq!(m.server.as_deref(), Some("Example Net, Riyadh"));
    }

    #[test]
    fn reports_the_cli_error_message() {
        let out = r#"{"type":"log","timestamp":"t","message":"Configuration - Could not retrieve or read configuration (ConfigurationError)","level":"error"}"#;
        assert!(parse(out).unwrap_err().contains("Configuration"));
        assert_eq!(parse("").unwrap_err(), "no result in the CLI output");
    }

    #[test]
    fn a_failed_run_says_where_and_why() {
        let stdout = concat!(
            r#"{"type":"testStart","timestamp":"t"}"#,
            "\n",
            r#"{"type":"ping","timestamp":"t","ping":{"latency":41.0,"progress":1.0}}"#,
            "\n",
            r#"{"type":"download","timestamp":"t","download":{"bandwidth":4000000,"bytes":1000,"progress":1.0}}"#,
            "\n",
            r#"{"type":"upload","timestamp":"t","upload":{"bandwidth":100000,"bytes":100,"progress":0.2}}"#,
        );
        let stderr = r#"{"type":"log","timestamp":"t","message":"Error: [0] Cannot read from socket: ","level":"error"}"#;
        let d = failure_detail(stdout, stderr);
        assert_eq!(
            d,
            "during the upload test: Error: [0] Cannot read from socket: "
        );
        // Plain-text stderr is kept when the CLI printed no JSON error.
        assert_eq!(
            failure_detail("", "[error] Timeout occurred in connect."),
            "[error] Timeout occurred in connect."
        );
        assert_eq!(failure_detail("", ""), "no result in the CLI output");
    }

    /// Shaped like the CLI's interim lines; the values are synthetic.
    #[test]
    fn interim_lines_become_live_numbers() {
        let mut t = Tracker::default();
        assert!(t.line(r#"{"type":"testStart","timestamp":"t"}"#).is_none());
        let (phase, f, bytes, live) = t
            .line(r#"{"type":"ping","ping":{"jitter":1.0,"latency":41.0,"progress":0.5}}"#)
            .unwrap();
        assert_eq!((phase, f, bytes), (SpeedPhase::IdlePing, 0.5, 0));
        assert_eq!(live.idle_ping_ms, Some(41.0));

        let (phase, f, bytes, live) = t
            .line(r#"{"type":"download","download":{"bandwidth":12500000,"bytes":1000,"elapsed":900,"progress":0.25,"latency":{"iqm":600.0}}}"#)
            .unwrap();
        assert_eq!((phase, f, bytes), (SpeedPhase::Download, 0.25, 1000));
        assert_eq!(live.download_mbps, Some(100.0));
        assert_eq!(live.ping_down_ms, Some(600.0));
        assert_eq!(live.idle_ping_ms, Some(41.0), "earlier values stay");

        let (phase, _, bytes, live) = t
            .line(r#"{"type":"upload","upload":{"bandwidth":125000,"bytes":500,"progress":2.0}}"#)
            .unwrap();
        assert_eq!((phase, bytes), (SpeedPhase::Upload, 1500));
        assert_eq!(live.upload_mbps, Some(1.0));
        assert_eq!(live.download_mbps, Some(100.0));

        assert!(t.line(SAMPLE).is_none(), "the result isn't progress");
        assert!(t.line("not json").is_none());
    }

    fn make_zip(exe: &[u8]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file("speedtest.md", opts).unwrap();
        w.write_all(b"readme").unwrap();
        w.start_file("speedtest.exe", opts).unwrap();
        w.write_all(exe).unwrap();
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn extraction_checks_both_hashes() {
        let exe = b"MZ synthetic exe";
        let z = make_zip(exe);
        let (zs, es) = (sha256_hex(&z), sha256_hex(exe));
        assert_eq!(extract(&z, &zs, &es).unwrap(), exe);
        assert!(extract(&z, &"0".repeat(64), &es)
            .unwrap_err()
            .contains("download hash"));
        assert!(extract(&z, &zs, &"0".repeat(64))
            .unwrap_err()
            .contains("speedtest.exe"));
    }

    #[tokio::test]
    async fn installs_from_a_verified_download() {
        let exe = b"MZ synthetic exe";
        let z = make_zip(exe);
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/cli.zip"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(z.clone()))
            .mount(&s)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let target = exe_path(dir.path());
        let (_tx, rx) = watch::channel(false);
        let url = format!("{}/cli.zip", s.uri());
        let es = sha256_hex(exe);
        install(&url, &sha256_hex(&z), &es, &target, rx.clone())
            .await
            .unwrap();
        assert!(is_installed(&target, &es));

        // A tampered download is refused and nothing new is written.
        let dir2 = tempfile::tempdir().unwrap();
        let t2 = exe_path(dir2.path());
        let r = install(&url, &"0".repeat(64), &es, &t2, rx).await;
        assert!(matches!(r, Err(OoklaError::Install(_))));
        assert!(!t2.exists());
    }

    #[tokio::test]
    async fn refuses_to_run_a_missing_or_changed_exe() {
        let dir = tempfile::tempdir().unwrap();
        let exe = exe_path(dir.path());
        let (_tx, rx) = watch::channel(false);
        let r = run(
            &exe,
            EXE_SHA256,
            Duration::from_secs(1),
            rx.clone(),
            &|_, _, _, _| {},
        )
        .await;
        assert!(matches!(r, Err(OoklaError::Install(_))));
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"not the pinned build").unwrap();
        let r = run(
            &exe,
            EXE_SHA256,
            Duration::from_secs(1),
            rx,
            &|_, _, _, _| {},
        )
        .await;
        assert!(matches!(r, Err(OoklaError::Install(_))));
    }
}
