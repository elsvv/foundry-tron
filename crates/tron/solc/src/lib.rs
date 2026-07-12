//! # foundry-tron-solc
//!
//! Resolver and downloader for the native `tron-solc` (TVM Solidity) compiler.
//!
//! `svm` cannot install `tron-solc`: its release host and checksum list are
//! hardcoded to `binaries.soliditylang.org`, which never matches the
//! `tronprotocol/solidity` builds. This crate mirrors what `svm` does for
//! vanilla solc — resolve a cached binary or download and verify one — but
//! against the Tron release layout:
//!
//! - binaries come from GitHub releases of `tronprotocol/solidity`, tag `tv_{version}`, with bare
//!   asset names (`solc-macos`, `solc-static-linux`, `solc-windows.exe`);
//! - checksums come from a table pinned in [`pins`], sourced from
//!   `tronprotocol.github.io/solc-bin/{platform}/list.json`;
//! - the cache lives at `~/.foundry-tron/solc/tron-solc-{version}` (Stage-1 convention, codified
//!   here).
//!
//! The public entrypoint is [`resolve_tron_solc`]. It never touches the network
//! when `offline` is set or when a verified binary is already cached, and it
//! always verifies sha256 before accepting a binary — a mismatch is a hard
//! error, never a silent fallback.

mod pins;

use semver::Version;
use sha2::{Digest, Sha256};
use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::runtime::{Handle, Runtime};

/// Errors returned while resolving or downloading a `tron-solc` binary.
#[derive(Debug, thiserror::Error)]
pub enum TronSolcError {
    /// The current platform has no native `tron-solc` build.
    #[error(
        "no native tron-solc build for platform '{0}' (build from source; \
         Linux ARM and other non-amd64 targets are unsupported)"
    )]
    UnsupportedPlatform(&'static str),
    /// No pinned checksum exists for this version/platform combination.
    #[error("no pinned tron-solc checksum for version {version} on platform '{platform}'")]
    NoPin {
        /// The requested version.
        version: Version,
        /// The solc-bin platform key that was looked up.
        platform: &'static str,
    },
    /// Download is disabled (offline) and no cached binary is present.
    #[error("tron-solc download disabled (offline) and no cached binary at {0}")]
    Offline(PathBuf),
    /// A cached binary exists but its checksum does not match the pin.
    #[error(
        "cached tron-solc at {path} is corrupted (sha256 {actual} != pinned {expected}); \
         delete it and rebuild"
    )]
    CorruptedCache {
        /// Path to the corrupted cache entry.
        path: PathBuf,
        /// The pinned checksum.
        expected: String,
        /// The checksum actually computed from the cached file.
        actual: String,
    },
    /// A freshly downloaded binary failed checksum verification.
    #[error(
        "downloaded tron-solc {version} failed checksum (sha256 {actual} != pinned {expected})"
    )]
    ChecksumMismatch {
        /// The requested version.
        version: Version,
        /// The pinned checksum.
        expected: String,
        /// The checksum of the downloaded bytes.
        actual: String,
    },
    /// The home directory could not be determined.
    #[error("could not determine home directory for ~/.foundry-tron")]
    NoHome,
    /// A network error occurred while downloading.
    #[error("failed to download tron-solc: {0}")]
    Http(String),
    /// A filesystem error occurred.
    #[error("tron-solc I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// A `tron-solc` target platform.
///
/// `LinuxAarch64` is modelled explicitly (and used as the fallback for unknown
/// targets) so that the "no native build" path is representable and testable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// macOS, universal fat `solc-macos` binary (arm64 + x86_64).
    MacOs,
    /// Linux x86_64, `solc-static-linux`.
    LinuxAmd64,
    /// Windows x86_64, `solc-windows.exe`.
    WindowsAmd64,
    /// Linux ARM64 (or any unsupported target): no native build exists.
    LinuxAarch64,
}

impl Platform {
    /// Detects the platform of the current build target.
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "windows") {
            Self::WindowsAmd64
        } else if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
            Self::LinuxAmd64
        } else {
            // Linux ARM and any other target has no native tron-solc build.
            Self::LinuxAarch64
        }
    }

    /// Returns the GitHub release asset name for this platform, or an error if
    /// no native build exists.
    pub const fn asset_name(self) -> Result<&'static str, TronSolcError> {
        match self {
            Self::MacOs => Ok("solc-macos"),
            Self::LinuxAmd64 => Ok("solc-static-linux"),
            Self::WindowsAmd64 => Ok("solc-windows.exe"),
            Self::LinuxAarch64 => Err(TronSolcError::UnsupportedPlatform(self.name())),
        }
    }

    /// Returns the `solc-bin` list.json platform key for checksum lookup, or
    /// `None` if no build exists for this platform.
    pub const fn list_key(self) -> Option<&'static str> {
        match self {
            Self::MacOs => Some("macosx-amd64"),
            Self::LinuxAmd64 => Some("linux-amd64"),
            Self::WindowsAmd64 => Some("windows-amd64"),
            Self::LinuxAarch64 => None,
        }
    }

    /// Returns a stable human-readable name for this platform.
    pub const fn name(self) -> &'static str {
        match self {
            Self::MacOs => "macos",
            Self::LinuxAmd64 => "linux-amd64",
            Self::WindowsAmd64 => "windows-amd64",
            Self::LinuxAarch64 => "linux-aarch64",
        }
    }
}

/// The default `tron-solc` version (latest pinned release).
pub const fn default_version() -> Version {
    Version::new(0, 8, 27)
}

/// Returns the cache directory for `tron-solc` binaries: `~/.foundry-tron/solc`.
pub fn tron_solc_dir() -> Result<PathBuf, TronSolcError> {
    dirs::home_dir()
        .map(|home| home.join(".foundry-tron").join("solc"))
        .ok_or(TronSolcError::NoHome)
}

/// Returns the absolute cache path for a specific `tron-solc` version:
/// `~/.foundry-tron/solc/tron-solc-{version}`.
pub fn binary_path(version: &Version) -> Result<PathBuf, TronSolcError> {
    Ok(tron_solc_dir()?.join(format!("tron-solc-{version}")))
}

/// Returns the pinned sha256 (lowercase hex, no `0x`) for a version/platform,
/// or `None` if the combination is not pinned.
pub fn pinned_sha256(version: &Version, platform: Platform) -> Option<&'static str> {
    let key = platform.list_key()?;
    let version = version.to_string();
    pins::PINS
        .iter()
        .find(|pin| pin.version == version && pin.platform == key)
        .map(|pin| pin.sha256)
}

/// Returns the GitHub release download URL for a version/platform.
pub fn download_url(version: &Version, platform: Platform) -> Result<String, TronSolcError> {
    let asset = platform.asset_name()?;
    Ok(format!("https://github.com/tronprotocol/solidity/releases/download/tv_{version}/{asset}"))
}

/// Resolves a native `tron-solc` binary for the current platform, returning the
/// absolute path to a checksum-verified executable.
///
/// A cached binary at `~/.foundry-tron/solc/tron-solc-{version}` is reused when
/// its sha256 matches the pin. Otherwise, unless `offline` is set, the binary is
/// downloaded from the `tronprotocol/solidity` release, verified against the
/// pinned sha256, and written atomically (temp file + rename) with mode `755`.
pub fn resolve_tron_solc(version: &Version, offline: bool) -> Result<PathBuf, TronSolcError> {
    resolve_in(&tron_solc_dir()?, version, Platform::current(), offline)
}

/// Core resolution logic, parameterized by cache directory and platform so it
/// can be exercised against temporary directories in tests.
fn resolve_in(
    dir: &Path,
    version: &Version,
    platform: Platform,
    offline: bool,
) -> Result<PathBuf, TronSolcError> {
    // Reject unsupported platforms before any filesystem or network work.
    let Some(platform_key) = platform.list_key() else {
        return Err(TronSolcError::UnsupportedPlatform(platform.name()));
    };
    let expected = pinned_sha256(version, platform)
        .ok_or_else(|| TronSolcError::NoPin { version: version.clone(), platform: platform_key })?;

    let path = dir.join(format!("tron-solc-{version}"));
    if path.is_file() {
        let actual = sha256_hex(&fs::read(&path)?);
        if actual == expected {
            return Ok(path);
        }
        return Err(TronSolcError::CorruptedCache { path, expected: expected.to_string(), actual });
    }

    if offline {
        return Err(TronSolcError::Offline(path));
    }

    let url = download_url(version, platform)?;
    let bytes = download_bytes(&url, 3)?;
    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(TronSolcError::ChecksumMismatch {
            version: version.clone(),
            expected: expected.to_string(),
            actual,
        });
    }
    atomic_write_executable(&path, &bytes)?;
    Ok(path)
}

/// Computes the lowercase hex sha256 of `bytes`.
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}

/// Writes `bytes` to `path` atomically (temp file in the same directory + rename)
/// and sets executable permissions (`755`) on Unix.
fn atomic_write_executable(path: &Path, bytes: &[u8]) -> Result<(), TronSolcError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp.{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Downloads the full body at `url`, following redirects, with up to `retries`
/// attempts. Uses the `RuntimeOrHandle` pattern from `foundry-compilers` so it
/// works both inside and outside an existing Tokio runtime.
fn download_bytes(url: &str, retries: u32) -> Result<Vec<u8>, TronSolcError> {
    BlockingRuntime::new().block_on(async {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(|err| TronSolcError::Http(err.to_string()))?;

        let mut last_err = None;
        for attempt in 0..retries.max(1) {
            match fetch(&client, url).await {
                Ok(bytes) => return Ok(bytes),
                Err(err) => {
                    last_err = Some(err);
                    // Bounded linear backoff between attempts.
                    tokio::time::sleep(Duration::from_millis(500 * u64::from(attempt + 1))).await;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| TronSolcError::Http("download failed".to_string())))
    })
}

/// Performs a single GET request and returns the full body.
async fn fetch(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, TronSolcError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| TronSolcError::Http(err.to_string()))?
        .error_for_status()
        .map_err(|err| TronSolcError::Http(err.to_string()))?;
    let bytes = response.bytes().await.map_err(|err| TronSolcError::Http(err.to_string()))?;
    Ok(bytes.to_vec())
}

/// Either an owned Tokio runtime or a handle to the ambient one, mirroring
/// `foundry_compilers_core::utils::RuntimeOrHandle`. `reqwest::blocking` cannot
/// be used inside a Tokio runtime, so blocking HTTP is driven this way instead.
enum BlockingRuntime {
    Runtime(Runtime),
    Handle(Handle),
}

impl BlockingRuntime {
    fn new() -> Self {
        match Handle::try_current() {
            Ok(handle) => Self::Handle(handle),
            Err(_) => Self::Runtime(Runtime::new().expect("failed to start Tokio runtime")),
        }
    }

    fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        match self {
            Self::Runtime(runtime) => runtime.block_on(future),
            Self::Handle(handle) => tokio::task::block_in_place(|| handle.block_on(future)),
        }
    }
}

// `eprintln!` is used to document why gated tests self-skip; it is disallowed
// in library code but appropriate for the test gate.
#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use super::*;

    #[test]
    fn default_version_is_0_8_27() {
        assert_eq!(default_version(), Version::new(0, 8, 27));
    }

    #[test]
    fn binary_path_is_absolute_under_foundry_tron() {
        let version = Version::new(0, 8, 27);
        let path = binary_path(&version).unwrap();
        let expected =
            dirs::home_dir().unwrap().join(".foundry-tron").join("solc").join("tron-solc-0.8.27");
        assert_eq!(path, expected);
        assert!(path.is_absolute());
    }

    #[test]
    fn asset_name_per_platform() {
        assert_eq!(Platform::MacOs.asset_name().unwrap(), "solc-macos");
        assert_eq!(Platform::LinuxAmd64.asset_name().unwrap(), "solc-static-linux");
        assert_eq!(Platform::WindowsAmd64.asset_name().unwrap(), "solc-windows.exe");
        assert!(matches!(
            Platform::LinuxAarch64.asset_name(),
            Err(TronSolcError::UnsupportedPlatform("linux-aarch64"))
        ));
    }

    #[test]
    fn pinned_sha256_from_table() {
        let v27 = Version::new(0, 8, 27);
        let v26 = Version::new(0, 8, 26);
        let v25 = Version::new(0, 8, 25);
        assert_eq!(
            pinned_sha256(&v27, Platform::MacOs),
            Some("9e369b442b3a835320cf1450a21ce5e8acf0d319a50d10a10a2001aac7ce17aa")
        );
        assert_eq!(
            pinned_sha256(&v25, Platform::LinuxAmd64),
            Some("203573476b68e657d274fffc6713214497881439039f2c74a9c64fe9e8a58ccf")
        );
        assert_eq!(
            pinned_sha256(&v26, Platform::WindowsAmd64),
            Some("bbb9e290265494e8ec9c31482b42e8bc4310d71bb37bb773bc97524b12d52d2a")
        );
        // Unknown version and unsupported platform have no pin.
        assert_eq!(pinned_sha256(&Version::new(0, 8, 99), Platform::MacOs), None);
        assert_eq!(pinned_sha256(&v27, Platform::LinuxAarch64), None);
    }

    #[test]
    fn download_url_format() {
        let version = Version::new(0, 8, 27);
        assert_eq!(
            download_url(&version, Platform::MacOs).unwrap(),
            "https://github.com/tronprotocol/solidity/releases/download/tv_0.8.27/solc-macos"
        );
        assert_eq!(
            download_url(&version, Platform::LinuxAmd64).unwrap(),
            "https://github.com/tronprotocol/solidity/releases/download/tv_0.8.27/solc-static-linux"
        );
        assert!(download_url(&version, Platform::LinuxAarch64).is_err());
    }

    #[test]
    fn offline_without_cache_errors() {
        let dir = tempfile::tempdir().unwrap();
        let version = Version::new(0, 8, 27);
        let err = resolve_in(dir.path(), &version, Platform::MacOs, true).unwrap_err();
        assert!(matches!(err, TronSolcError::Offline(_)), "unexpected error: {err:?}");
    }

    #[test]
    fn corrupted_cache_errors() {
        let dir = tempfile::tempdir().unwrap();
        let version = Version::new(0, 8, 27);
        // Place a file with the right name but wrong contents.
        fs::write(dir.path().join("tron-solc-0.8.27"), b"not a real tron-solc binary").unwrap();
        let err = resolve_in(dir.path(), &version, Platform::MacOs, true).unwrap_err();
        match err {
            TronSolcError::CorruptedCache { expected, actual, .. } => {
                assert_eq!(
                    expected,
                    "9e369b442b3a835320cf1450a21ce5e8acf0d319a50d10a10a2001aac7ce17aa"
                );
                assert_ne!(expected, actual);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn unsupported_platform_errors_before_offline() {
        let dir = tempfile::tempdir().unwrap();
        let version = Version::new(0, 8, 27);
        let err = resolve_in(dir.path(), &version, Platform::LinuxAarch64, true).unwrap_err();
        assert!(
            matches!(err, TronSolcError::UnsupportedPlatform("linux-aarch64")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn unknown_version_has_no_pin() {
        let dir = tempfile::tempdir().unwrap();
        let version = Version::new(0, 8, 99);
        let err = resolve_in(dir.path(), &version, Platform::MacOs, true).unwrap_err();
        assert!(matches!(err, TronSolcError::NoPin { .. }), "unexpected error: {err:?}");
    }

    /// Offline cache hit against the real Stage-1 binary on this machine. Skips
    /// cleanly when the binary is not present so the suite stays portable.
    #[test]
    fn cache_hit_offline_returns_path_without_network() {
        let dir = match tron_solc_dir() {
            Ok(dir) => dir,
            Err(_) => {
                eprintln!("skipping cache-hit test: no home directory");
                return;
            }
        };
        let version = Version::new(0, 8, 27);
        let path = dir.join("tron-solc-0.8.27");
        if !path.is_file() {
            eprintln!(
                "skipping cache-hit test: no cached binary at {} \
                 (install tron-solc 0.8.27 to exercise this path)",
                path.display()
            );
            return;
        }
        // offline = true guarantees this cannot reach the network; success proves
        // the sha256-verified cache hit path.
        let resolved = resolve_in(&dir, &version, Platform::MacOs, true).unwrap();
        assert_eq!(resolved, path);
    }

    /// Cross-check: the pinned 0.8.27 macOS checksum equals the sha256 of the
    /// real binary installed on this machine (the ground truth for the pin).
    #[test]
    fn pin_0_8_27_macos_matches_installed_binary() {
        let Ok(dir) = tron_solc_dir() else {
            eprintln!("skipping pin cross-check: no home directory");
            return;
        };
        let path = dir.join("tron-solc-0.8.27");
        if !path.is_file() {
            eprintln!("skipping pin cross-check: no cached binary at {}", path.display());
            return;
        }
        let actual = sha256_hex(&fs::read(&path).unwrap());
        assert_eq!(
            actual,
            pinned_sha256(&Version::new(0, 8, 27), Platform::MacOs).unwrap(),
            "installed tron-solc-0.8.27 sha256 must equal the embedded pin"
        );
    }

    /// Real network download, gated on `TRON_SOLC_DOWNLOAD=1`. Downloads the
    /// smallest release asset across platforms (the ~10 MB Windows build for
    /// 0.8.27) and verifies its sha256 against the pin, exercising the full
    /// download + verify machinery without pulling the 80 MB macOS binary.
    #[test]
    fn tron_solc_download_verifies_sha256() {
        if std::env::var("TRON_SOLC_DOWNLOAD").is_err() {
            eprintln!(
                "skipping download test: set TRON_SOLC_DOWNLOAD=1 to run the real \
                 tron-solc download + checksum test"
            );
            return;
        }
        let version = Version::new(0, 8, 27);
        let platform = Platform::WindowsAmd64;
        let url = download_url(&version, platform).unwrap();
        let bytes = download_bytes(&url, 3).unwrap();
        let actual = sha256_hex(&bytes);
        assert_eq!(
            actual,
            pinned_sha256(&version, platform).unwrap(),
            "downloaded windows tron-solc 0.8.27 sha256 must match the embedded pin"
        );
    }
}
