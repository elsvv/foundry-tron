//! Pinned sha256 checksums for native tron-solc release binaries.
//!
//! Source: `https://tronprotocol.github.io/solc-bin/{platform}/list.json`,
//! platform ∈ {`linux-amd64`, `macosx-amd64`, `windows-amd64`}. Fetched
//! 2026-07-12. The list is a checksum table only (no download URLs); the
//! binaries themselves come from the GitHub releases of
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

/// Embedded checksum table for the supported tron-solc versions and platforms.
pub(crate) const PINS: &[Pin] = &[
    // linux-amd64 (solc-static-linux, x86_64).
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
