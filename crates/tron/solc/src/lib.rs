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
//! Versions released after this toolchain shipped are not in the [`pins`] table.
//! For those, an online resolve consults the same live `solc-bin` `list.json`
//! the pins were snapshotted from, downloads the binary it points to, and
//! verifies it against that list's own sha256. This lets a new `tron-solc`
//! release be selected from config (`solc = "X.Y.Z"`) without shipping new
//! toolchain code. Pins stay authoritative when present; the live list is only
//! a fallback for unpinned versions, and never runs when `offline` is set.
//!
//! The public entrypoint is [`resolve_tron_solc`]. It never touches the network
//! when `offline` is set or when a verified binary is already cached, and it
//! always verifies sha256 before accepting a binary — a mismatch is a hard
//! error, never a silent fallback.

mod pins;

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
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
    /// No pinned checksum exists for this version/platform combination, and (when
    /// `searched_solc_bin` is set) the live solc-bin list did not list it either.
    #[error(
        "no pinned tron-solc checksum for version {version} on platform '{platform}'{extra}",
        extra = if *.searched_solc_bin {
            ", and it is not in the tronprotocol solc-bin list either"
        } else {
            ""
        }
    )]
    NoPin {
        /// The requested version.
        version: Version,
        /// The solc-bin platform key that was looked up.
        platform: &'static str,
        /// Whether the live solc-bin `list.json` was also consulted (online path)
        /// and lacked this version. `false` means only the compile-time pin table
        /// was checked (offline path).
        searched_solc_bin: bool,
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
    Version::new(0, 8, 28)
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

/// Returns the solc long version (`<version>+commit.<8hex>`) for a semantic
/// version, or `None` if it is neither pinned nor cached.
///
/// The value is the platform-independent `builds[].longVersion` from the same
/// `solc-bin` list.json the checksum table is sourced from. Pinned versions
/// resolve from the embedded table; a version resolved at runtime from the live
/// list (see [`resolve_tron_solc`]) writes its long version to a sidecar next to
/// the cached binary, so this lookup keeps working for versions newer than the
/// pins — which TronScan verification needs to build its `compiler` field.
pub fn tron_solc_long_version(version: &Version) -> Option<Cow<'static, str>> {
    let version_str = version.to_string();
    if let Some(entry) = pins::LONG_VERSIONS.iter().find(|entry| entry.version == version_str) {
        return Some(Cow::Borrowed(entry.long_version));
    }
    read_cached_long_version(version).map(Cow::Owned)
}

/// Reads the long-version sidecar written by [`resolve_via_list`] for a version
/// resolved from the live solc-bin list. Best-effort: any error yields `None`.
fn read_cached_long_version(version: &Version) -> Option<String> {
    let path = tron_solc_dir().ok()?.join(long_version_sidecar_name(version));
    let contents = fs::read_to_string(path).ok()?;
    let trimmed = contents.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The sidecar filename holding the long version for a cached binary:
/// `tron-solc-{version}.longversion`.
fn long_version_sidecar_name(version: &Version) -> String {
    format!("tron-solc-{version}.longversion")
}

/// Returns the TronScan `compiler` field for a pinned tron-solc version:
/// `tron_v<longVersion>`, e.g. `tron_v0.8.27+commit.19164bed`.
///
/// TronScan's contract-verification API validates the compiler by this exact
/// string (live-confirmed for 0.8.25). Returns `None` when the version is not
/// pinned, so callers can surface a clear error instead of a bad guess.
pub fn tronscan_compiler_string(version: &Version) -> Option<String> {
    tron_solc_long_version(version).map(|long| format!("tron_v{long}"))
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

    let Some(expected) = pinned_sha256(version, platform) else {
        // Not compile-time pinned. Without a checksum, offline resolution is
        // impossible, so fail exactly as before. Online, fall back to the live
        // solc-bin list — the same source the pins were snapshotted from — so a
        // `tron-solc` release newer than this toolchain resolves from config.
        if offline {
            return Err(TronSolcError::NoPin {
                version: version.clone(),
                platform: platform_key,
                searched_solc_bin: false,
            });
        }
        return resolve_via_list(dir, version, platform);
    };

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

/// The solc-bin base URL. Both `list.json` and the binaries it references live
/// under `{SOLC_BIN_BASE}/{list_key}/`.
const SOLC_BIN_BASE: &str = "https://tronprotocol.github.io/solc-bin";

/// A solc-bin `list.json` document (only the fields this crate consumes).
#[derive(Debug, Deserialize)]
struct SolcBinList {
    /// The per-version build entries.
    builds: Vec<SolcBinBuild>,
}

/// A single `builds[]` entry from a solc-bin `list.json`.
#[derive(Debug, Deserialize)]
struct SolcBinBuild {
    /// Binary filename relative to the platform directory, e.g.
    /// `solc-windows-amd64-v0.8.28+commit.9c4253d2.exe`.
    path: String,
    /// Semantic version, e.g. `0.8.28`.
    version: String,
    /// Full long version, e.g. `0.8.28+commit.9c4253d2`.
    #[serde(rename = "longVersion")]
    long_version: String,
    /// sha256 of the binary, `0x`-prefixed in the list.
    sha256: String,
}

/// Parses a solc-bin `list.json` body and returns the `builds[]` entry whose
/// `version` matches `version` exactly, or `None` when the list omits it.
fn find_list_build(
    list_json: &[u8],
    version: &Version,
) -> Result<Option<SolcBinBuild>, TronSolcError> {
    let list: SolcBinList = serde_json::from_slice(list_json)
        .map_err(|err| TronSolcError::Http(format!("invalid solc-bin list.json: {err}")))?;
    let want = version.to_string();
    Ok(list.builds.into_iter().find(|build| build.version == want))
}

/// Normalizes a solc-bin sha256 (`0x`-prefixed, possibly mixed case) to the
/// lowercase hex, no-`0x` form used everywhere else in this crate.
fn normalize_sha256(raw: &str) -> String {
    raw.strip_prefix("0x").unwrap_or(raw).to_ascii_lowercase()
}

/// Resolves a `tron-solc` binary for a version that is not compile-time pinned by
/// consulting the live solc-bin `list.json` for `platform`.
///
/// This is the same authoritative source the [`pins`] table was snapshotted
/// from, so it lets a `tron-solc` release published after this toolchain shipped
/// be resolved from config. The binary is still sha256-verified against the
/// list's own checksum before it is accepted — a mismatch is a hard error, never
/// a silent fallback. On success the binary's long version is recorded in a
/// sidecar next to the cache entry so [`tron_solc_long_version`] (hence TronScan
/// verification) keeps working for the unpinned version.
fn resolve_via_list(
    dir: &Path,
    version: &Version,
    platform: Platform,
) -> Result<PathBuf, TronSolcError> {
    let Some(list_key) = platform.list_key() else {
        return Err(TronSolcError::UnsupportedPlatform(platform.name()));
    };

    let list_url = format!("{SOLC_BIN_BASE}/{list_key}/list.json");
    let list_json = download_bytes(&list_url, 3)?;
    let Some(build) = find_list_build(&list_json, version)? else {
        return Err(TronSolcError::NoPin {
            version: version.clone(),
            platform: list_key,
            searched_solc_bin: true,
        });
    };
    let expected = normalize_sha256(&build.sha256);

    let path = dir.join(format!("tron-solc-{version}"));
    if path.is_file() {
        let actual = sha256_hex(&fs::read(&path)?);
        if actual != expected {
            return Err(TronSolcError::CorruptedCache { path, expected, actual });
        }
        write_cached_long_version(dir, version, &build.long_version);
        return Ok(path);
    }

    let bin_url = format!("{SOLC_BIN_BASE}/{list_key}/{}", build.path);
    let bytes = download_bytes(&bin_url, 3)?;
    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(TronSolcError::ChecksumMismatch { version: version.clone(), expected, actual });
    }
    atomic_write_executable(&path, &bytes)?;
    write_cached_long_version(dir, version, &build.long_version);
    Ok(path)
}

/// Writes the long-version sidecar (`tron-solc-{version}.longversion`) next to a
/// cached binary. Best-effort: filesystem errors are swallowed so a failed
/// sidecar write never fails an otherwise-successful resolve.
fn write_cached_long_version(dir: &Path, version: &Version, long_version: &str) {
    if let Err(_err) = fs::create_dir_all(dir) {
        return;
    }
    let _ = fs::write(dir.join(long_version_sidecar_name(version)), long_version);
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

    /// A trimmed but byte-real solc-bin `windows-amd64/list.json` (builds
    /// 0.8.26–0.8.28), captured 2026-07-23, for offline list-parsing tests.
    const WINDOWS_LIST_FIXTURE: &str = include_str!("../testdata/list-windows-amd64.json");

    #[test]
    fn default_version_is_0_8_28() {
        assert_eq!(default_version(), Version::new(0, 8, 28));
        // The default must always be a pinned version so it resolves offline.
        assert!(pinned_sha256(&default_version(), Platform::MacOs).is_some());
        assert!(pinned_sha256(&default_version(), Platform::LinuxAmd64).is_some());
        assert!(pinned_sha256(&default_version(), Platform::WindowsAmd64).is_some());
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
        let v24 = Version::new(0, 8, 24);
        let v23 = Version::new(0, 8, 23);
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
        // 0.8.23 / 0.8.24 pins from list.json (fetched 2026-07-16); 0.8.23 is the
        // exact pragma pinned by the W4 target project's contracts.
        assert_eq!(
            pinned_sha256(&v23, Platform::LinuxAmd64),
            Some("40f5145ef352bc139acfcc0e257db67a80de1dcdc018d1762d584dff866faa6c")
        );
        assert_eq!(
            pinned_sha256(&v23, Platform::MacOs),
            Some("0397a9dfc52667a36f6020f5f99977114ebda4758bc49b246d622fd727a72058")
        );
        assert_eq!(
            pinned_sha256(&v23, Platform::WindowsAmd64),
            Some("8723f35f2e609bf488fd958300e30800f96e8ab8f43365ec00ca0367ea18cea9")
        );
        assert_eq!(
            pinned_sha256(&v24, Platform::LinuxAmd64),
            Some("82605d2d64d9bcc2ab0b7608d192463de009a8ece6032058d4cbdbefbde87e78")
        );
        assert_eq!(
            pinned_sha256(&v24, Platform::MacOs),
            Some("9bdddd9e9afd96eabada84cb9cafe7b6bf5d6bb101e08019ce1d103824a561ed")
        );
        assert_eq!(
            pinned_sha256(&v24, Platform::WindowsAmd64),
            Some("35957118ebcb217903138e65fa1ded0a8dceffe6994d2ba7554c92cee81dad0a")
        );
        // 0.8.28 pins (fetched from list.json 2026-07-23), all three platforms.
        let v28 = Version::new(0, 8, 28);
        assert_eq!(
            pinned_sha256(&v28, Platform::MacOs),
            Some("e492e14fd3da07e65830c1189758ac34f8a3f06c3f59df42f5c5f6a744e0dbd2")
        );
        assert_eq!(
            pinned_sha256(&v28, Platform::LinuxAmd64),
            Some("0eba121b08e9fbc1019e71bb6d36467a7f653929af9f51a793a17688d859f856")
        );
        assert_eq!(
            pinned_sha256(&v28, Platform::WindowsAmd64),
            Some("000b24310f0b849d886b280908fde4c2dd63658bca3ffb4909ac7d9c6ab647e9")
        );
        // Unknown version and unsupported platform have no pin.
        assert_eq!(pinned_sha256(&Version::new(0, 8, 99), Platform::MacOs), None);
        assert_eq!(pinned_sha256(&v27, Platform::LinuxAarch64), None);
    }

    #[test]
    fn tronscan_compiler_string_matches_pinned_long_versions() {
        // 0.8.25 is live-confirmed against a verified mainnet contract's /info
        // (compiler == "tron_v0.8.25+commit.77bd169f").
        assert_eq!(
            tronscan_compiler_string(&Version::new(0, 8, 25)).as_deref(),
            Some("tron_v0.8.25+commit.77bd169f")
        );
        // 0.8.26 / 0.8.27 come from the authoritative list.json longVersion.
        assert_eq!(
            tronscan_compiler_string(&Version::new(0, 8, 26)).as_deref(),
            Some("tron_v0.8.26+commit.733b4d28")
        );
        assert_eq!(
            tronscan_compiler_string(&Version::new(0, 8, 27)).as_deref(),
            Some("tron_v0.8.27+commit.19164bed")
        );
        // 0.8.23 / 0.8.24 long versions from list.json (fetched 2026-07-16).
        assert_eq!(
            tronscan_compiler_string(&Version::new(0, 8, 23)).as_deref(),
            Some("tron_v0.8.23+commit.8ed33446")
        );
        assert_eq!(
            tronscan_compiler_string(&Version::new(0, 8, 24)).as_deref(),
            Some("tron_v0.8.24+commit.7d902c66")
        );
        assert_eq!(
            tron_solc_long_version(&Version::new(0, 8, 23)).as_deref(),
            Some("0.8.23+commit.8ed33446")
        );
        // The long version alone (no tron_v prefix) is also exposed.
        assert_eq!(
            tron_solc_long_version(&Version::new(0, 8, 27)).as_deref(),
            Some("0.8.27+commit.19164bed")
        );
        // 0.8.28 is the newest pinned release.
        assert_eq!(
            tronscan_compiler_string(&Version::new(0, 8, 28)).as_deref(),
            Some("tron_v0.8.28+commit.9c4253d2")
        );
        // Unpinned versions have no compiler string (absent any list-resolve sidecar).
        assert_eq!(tronscan_compiler_string(&Version::new(0, 4, 25)), None);
        assert_eq!(tron_solc_long_version(&Version::new(0, 4, 25)).as_deref(), None);
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
        // Offline: only the compile-time pin table was checked, never the network.
        match err {
            TronSolcError::NoPin { searched_solc_bin, .. } => assert!(
                !searched_solc_bin,
                "offline resolve must not claim the solc-bin list was consulted"
            ),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn find_list_build_selects_exact_version() {
        let build = find_list_build(WINDOWS_LIST_FIXTURE.as_bytes(), &Version::new(0, 8, 28))
            .unwrap()
            .expect("0.8.28 present in fixture");
        assert_eq!(build.version, "0.8.28");
        assert_eq!(build.long_version, "0.8.28+commit.9c4253d2");
        assert_eq!(build.path, "solc-windows-amd64-v0.8.28+commit.9c4253d2.exe");
        assert_eq!(
            build.sha256,
            "0x000b24310f0b849d886b280908fde4c2dd63658bca3ffb4909ac7d9c6ab647e9"
        );
        // A version absent from the list resolves to None, not an error.
        assert!(
            find_list_build(WINDOWS_LIST_FIXTURE.as_bytes(), &Version::new(0, 9, 99))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn find_list_build_rejects_malformed_json() {
        let err = find_list_build(b"{not json", &Version::new(0, 8, 28)).unwrap_err();
        assert!(matches!(err, TronSolcError::Http(_)), "unexpected error: {err:?}");
    }

    #[test]
    fn normalize_sha256_strips_prefix_and_lowercases() {
        assert_eq!(normalize_sha256("0x000B24310F"), "000b24310f");
        assert_eq!(normalize_sha256("ABCDEF"), "abcdef");
        assert_eq!(normalize_sha256("already0xfree"), "already0xfree");
    }

    #[test]
    fn list_resolve_urls_follow_solc_bin_layout() {
        let build = find_list_build(WINDOWS_LIST_FIXTURE.as_bytes(), &Version::new(0, 8, 28))
            .unwrap()
            .unwrap();
        assert_eq!(
            format!("{SOLC_BIN_BASE}/windows-amd64/list.json"),
            "https://tronprotocol.github.io/solc-bin/windows-amd64/list.json"
        );
        assert_eq!(
            format!("{SOLC_BIN_BASE}/windows-amd64/{}", build.path),
            "https://tronprotocol.github.io/solc-bin/windows-amd64/\
             solc-windows-amd64-v0.8.28+commit.9c4253d2.exe"
        );
    }

    /// The list.json checksum is the exact source the pins were snapshotted from,
    /// so for every version present in both, the fixture's sha256 must equal the
    /// embedded pin. Guards against the pin table and the live list drifting.
    #[test]
    fn fixture_sha256_matches_embedded_pins() {
        for version in [Version::new(0, 8, 26), Version::new(0, 8, 27), Version::new(0, 8, 28)] {
            let build =
                find_list_build(WINDOWS_LIST_FIXTURE.as_bytes(), &version).unwrap().unwrap();
            assert_eq!(
                normalize_sha256(&build.sha256),
                pinned_sha256(&version, Platform::WindowsAmd64).unwrap(),
                "list.json sha256 must equal the embedded pin for {version}"
            );
        }
    }

    #[test]
    fn nopin_message_mentions_solc_bin_only_when_searched() {
        let searched = TronSolcError::NoPin {
            version: Version::new(0, 8, 99),
            platform: "macosx-amd64",
            searched_solc_bin: true,
        };
        assert!(
            searched.to_string().contains("not in the tronprotocol solc-bin list either"),
            "message: {searched}"
        );
        let offline = TronSolcError::NoPin {
            version: Version::new(0, 8, 99),
            platform: "macosx-amd64",
            searched_solc_bin: false,
        };
        assert!(
            !offline.to_string().contains("solc-bin list either"),
            "offline message must not claim the list was consulted: {offline}"
        );
    }

    /// An unpinned version resolved from the live list writes a long-version
    /// sidecar; [`tron_solc_long_version`] then finds it via the real cache dir.
    /// This drives the sidecar read/write pair without any network.
    #[test]
    fn long_version_sidecar_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let version = Version::new(0, 8, 44);
        write_cached_long_version(dir.path(), &version, "0.8.44+commit.deadbeef");
        let contents = fs::read_to_string(dir.path().join("tron-solc-0.8.44.longversion")).unwrap();
        assert_eq!(contents.trim(), "0.8.44+commit.deadbeef");
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

    /// Real network resolve through the live solc-bin `list.json`, gated on
    /// `TRON_SOLC_DOWNLOAD=1`. Calls [`resolve_via_list`] directly to force the
    /// unpinned branch — 0.8.28 is pinned, so `resolve_in` would otherwise take
    /// the pinned GitHub-release path. Downloads the ~10 MB Windows build hosted
    /// under solc-bin, cross-checks its sha256 against the embedded pin (proving
    /// the solc-bin-hosted binary is byte-identical to the release), and asserts
    /// the long-version sidecar was written next to the cache entry.
    #[test]
    fn tron_solc_resolve_via_list_downloads_verifies_and_records_long_version() {
        if std::env::var("TRON_SOLC_DOWNLOAD").is_err() {
            eprintln!(
                "skipping list-resolve download test: set TRON_SOLC_DOWNLOAD=1 to run the \
                 real solc-bin list.json resolve + checksum test"
            );
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let version = Version::new(0, 8, 28);
        let platform = Platform::WindowsAmd64;
        let path = resolve_via_list(dir.path(), &version, platform).unwrap();
        assert!(path.is_file(), "resolved binary must exist at {}", path.display());
        let actual = sha256_hex(&fs::read(&path).unwrap());
        assert_eq!(
            actual,
            pinned_sha256(&version, platform).unwrap(),
            "solc-bin-hosted 0.8.28 windows binary sha256 must equal the embedded pin"
        );
        let sidecar = dir.path().join("tron-solc-0.8.28.longversion");
        assert_eq!(
            fs::read_to_string(&sidecar).unwrap().trim(),
            "0.8.28+commit.9c4253d2",
            "resolve_via_list must record the long version in a sidecar"
        );
    }
}
