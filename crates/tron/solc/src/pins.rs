//! Pinned sha256 checksums for native tron-solc release binaries.
//!
//! Source: `https://tronprotocol.github.io/solc-bin/{platform}/list.json`,
//! platform ∈ {`linux-amd64`, `macosx-amd64`, `windows-amd64`}. The
//! `0.8.25`–`0.8.27` pins were fetched 2026-07-12; the `0.8.23` / `0.8.24` pins
//! were fetched 2026-07-16. The list is a checksum table only (no download
//! URLs); the binaries themselves come from the GitHub releases of
//! `tronprotocol/solidity` (tag `tv_{version}`).
//!
//! macOS ARM is served by the universal fat `solc-macos` binary, whose
//! checksum lives under the `macosx-amd64` list. There is no native Linux ARM
//! build (`linux-arm64` / `macosx-aarch64` lists 404), so no ARM Linux pins
//! exist here.
//!
//! Cross-checked: the `0.8.27` / `macosx-amd64` pin equals the sha256 of the
//! manually installed Stage-1 binary at `~/.foundry-tron/solc/tron-solc-0.8.27`.

/// A single pinned checksum, keyed by semantic version and solc-bin platform.
pub(crate) struct Pin {
    /// Semantic version string, e.g. `"0.8.27"`.
    pub version: &'static str,
    /// solc-bin platform key, e.g. `"macosx-amd64"`.
    pub platform: &'static str,
    /// Lowercase hex sha256 of the release binary, without a `0x` prefix.
    pub sha256: &'static str,
}

/// A pinned solc long version (`<version>+commit.<8hex>`), keyed by semantic
/// version. Platform-independent, so it is tracked separately from [`Pin`].
///
/// The value is exactly `builds[].longVersion` from the same
/// `tronprotocol.github.io/solc-bin/{platform}/list.json` used for the checksum
/// table. TronScan's contract-verification `compiler` field wants this form
/// prefixed with `tron_v` (e.g. `tron_v0.8.25+commit.77bd169f`), because the
/// native `tron-solc` resolver skips the `solc --version` exec and so never
/// captures the commit itself.
pub(crate) struct LongVersion {
    /// Semantic version string, e.g. `"0.8.27"`.
    pub version: &'static str,
    /// Full solc long version, e.g. `"0.8.27+commit.19164bed"`.
    pub long_version: &'static str,
}

/// Embedded long-version table for the supported tron-solc versions.
///
/// Source: `list.json` `builds[].longVersion` (0.8.25/0.8.26/0.8.27 fetched
/// 2026-07-14; 0.8.23/0.8.24 fetched 2026-07-16). The 0.8.25 entry is
/// additionally confirmed live: TronScan stores
/// `compiler="tron_v0.8.25+commit.77bd169f"` for the verified mainnet contract
/// `TMv7hAfswe2EvXG4nUeNFGEgNWE8Joedtu`.
pub(crate) const LONG_VERSIONS: &[LongVersion] = &[
    LongVersion { version: "0.8.23", long_version: "0.8.23+commit.8ed33446" },
    LongVersion { version: "0.8.24", long_version: "0.8.24+commit.7d902c66" },
    LongVersion { version: "0.8.25", long_version: "0.8.25+commit.77bd169f" },
    LongVersion { version: "0.8.26", long_version: "0.8.26+commit.733b4d28" },
    LongVersion { version: "0.8.27", long_version: "0.8.27+commit.19164bed" },
];

/// Embedded checksum table for the supported tron-solc versions and platforms.
pub(crate) const PINS: &[Pin] = &[
    // linux-amd64 (solc-static-linux, x86_64).
    Pin {
        version: "0.8.23",
        platform: "linux-amd64",
        sha256: "40f5145ef352bc139acfcc0e257db67a80de1dcdc018d1762d584dff866faa6c",
    },
    Pin {
        version: "0.8.24",
        platform: "linux-amd64",
        sha256: "82605d2d64d9bcc2ab0b7608d192463de009a8ece6032058d4cbdbefbde87e78",
    },
    Pin {
        version: "0.8.25",
        platform: "linux-amd64",
        sha256: "203573476b68e657d274fffc6713214497881439039f2c74a9c64fe9e8a58ccf",
    },
    Pin {
        version: "0.8.26",
        platform: "linux-amd64",
        sha256: "98a0df6518346d07f13ec151f64bed48f4df9dd01fbfd81018f9485a374f8edd",
    },
    Pin {
        version: "0.8.27",
        platform: "linux-amd64",
        sha256: "7a3dfc3d987bbe447c0eba89e5fd89a1fc053e8629799e167810367196a3281e",
    },
    // macosx-amd64 (solc-macos, universal fat binary covering arm64 + x86_64).
    Pin {
        version: "0.8.23",
        platform: "macosx-amd64",
        sha256: "0397a9dfc52667a36f6020f5f99977114ebda4758bc49b246d622fd727a72058",
    },
    Pin {
        version: "0.8.24",
        platform: "macosx-amd64",
        sha256: "9bdddd9e9afd96eabada84cb9cafe7b6bf5d6bb101e08019ce1d103824a561ed",
    },
    Pin {
        version: "0.8.25",
        platform: "macosx-amd64",
        sha256: "36876c7e98c192584a99974905fc5f90c60540beb9776ee4a437f5a9d3aec707",
    },
    Pin {
        version: "0.8.26",
        platform: "macosx-amd64",
        sha256: "1776deae651ebbf9aa2e536d1d1f9b8a3bcf63b45b9fb5f7206602a9b89bd350",
    },
    Pin {
        version: "0.8.27",
        platform: "macosx-amd64",
        sha256: "9e369b442b3a835320cf1450a21ce5e8acf0d319a50d10a10a2001aac7ce17aa",
    },
    // windows-amd64 (solc-windows.exe).
    Pin {
        version: "0.8.23",
        platform: "windows-amd64",
        sha256: "8723f35f2e609bf488fd958300e30800f96e8ab8f43365ec00ca0367ea18cea9",
    },
    Pin {
        version: "0.8.24",
        platform: "windows-amd64",
        sha256: "35957118ebcb217903138e65fa1ded0a8dceffe6994d2ba7554c92cee81dad0a",
    },
    Pin {
        version: "0.8.25",
        platform: "windows-amd64",
        sha256: "f57a89ee2b34f00ac30d147d9b2b34749df129921ea0b55794809c09448fd99d",
    },
    Pin {
        version: "0.8.26",
        platform: "windows-amd64",
        sha256: "bbb9e290265494e8ec9c31482b42e8bc4310d71bb37bb773bc97524b12d52d2a",
    },
    Pin {
        version: "0.8.27",
        platform: "windows-amd64",
        sha256: "6c387a01ac81ad044c0f37816c059828c1080cc38f5882fd1f08d58f990ac7e0",
    },
];
