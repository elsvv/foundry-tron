//! Tron precompile address book and trace labels.
//!
//! These are the canonical addresses of the java-tron precompiled contracts
//! (`PrecompiledContracts.java` @develop, `getContractForAddress`). They are the
//! single source of truth shared by two consumers:
//!
//! - `foundry-evm-core` installs the actual precompile implementations at these addresses
//!   (`crates/evm/core/src/evm/tron/precompiles.rs`);
//! - [`TRON_PRECOMPILES`] here feeds `NetworkConfigs::precompiles_label` / `precompiles` so forge
//!   traces and `forge config` name them (the Tempo `TEMPO_PRECOMPILES` model).
//!
//! Addresses mirror java-tron's `new DataWord("00..00<id>")` constants exactly:
//! the standard set at `0x01`-`0x0a`, the `allowTvmCompatibleEvm` set at
//! `0x020003`/`0x020009`, and the shielded/vote/FreezeV2 sets at `0x1000001`-
//! `0x1000015`.

use alloy_primitives::Address;

/// Tron mainnet chain id (`728126428`) — the value the `CHAINID` opcode returns on
/// Tron mainnet. Used as the default local (non-fork) chain id for `network = "tron"`
/// projects so EIP-712 / permit domains resolve without a manual `chain_id` in
/// `foundry.toml`. An explicit `chain_id` in config still wins; a fork takes the
/// node's id.
pub const TRON_MAINNET_CHAIN_ID: u64 = 728_126_428;

/// Tron Nile testnet chain id (`3448148188`). Broadcast artifacts land under this id
/// (`broadcast/<script>/3448148188/`). Set `chain_id = 3448148188` in config to target
/// Nile; the explicit value overrides the mainnet default above.
pub const TRON_NILE_CHAIN_ID: u64 = 3_448_148_188;

/// Builds a 20-byte precompile address whose low 8 bytes are `id`, matching
/// java-tron's `new DataWord("0000..00<id>")` precompile address constants.
const fn addr(id: u64) -> Address {
    let b = id.to_be_bytes();
    Address::new([
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ])
}

/// `0x01` ECRecover (secp256k1 recover). Output is the Tron 21-byte address form
/// (`0x41`-prefixed) left-padded into the word, not Ethereum's 20-byte form.
pub const ECRECOVER: Address = addr(0x01);
/// `0x02` SHA-256 (identical to Ethereum).
pub const SHA256: Address = addr(0x02);
/// `0x03` — NOT ripemd160. java-tron computes `sha256(sha256(data)[0..20])` here.
pub const RIPEMD160_BROKEN: Address = addr(0x03);
/// `0x04` Identity / datacopy (identical to Ethereum).
pub const IDENTITY: Address = addr(0x04);
/// `0x05` ModExp with EIP-198 pricing (GQUAD divisor 20, no 200 floor).
pub const MODEXP: Address = addr(0x05);
/// `0x06` alt_bn128 addition (Istanbul pricing).
pub const BN128_ADD: Address = addr(0x06);
/// `0x07` alt_bn128 scalar multiplication (Istanbul pricing).
pub const BN128_MUL: Address = addr(0x07);
/// `0x08` alt_bn128 pairing (Istanbul pricing).
pub const BN128_PAIRING: Address = addr(0x08);
/// `0x09` BatchValidateSign (TIP-43). Replaces Ethereum's blake2f at this address.
pub const BATCH_VALIDATE_SIGN: Address = addr(0x09);
/// `0x0a` ValidateMultiSign (TIP-60). Replaces Ethereum's KZG point-eval here.
pub const VALIDATE_MULTISIGN: Address = addr(0x0a);
/// `0x020003` EthRipemd160 (the real ripemd160), `allowTvmCompatibleEvm`.
pub const ETH_RIPEMD160: Address = addr(0x02_0003);
/// `0x020009` Blake2F (EIP-152), `allowTvmCompatibleEvm`.
pub const BLAKE2F: Address = addr(0x02_0009);
/// `0x1000001` VerifyMintProof (shielded TRC-20). Local stub.
pub const VERIFY_MINT_PROOF: Address = addr(0x0100_0001);
/// `0x1000002` VerifyTransferProof (shielded TRC-20). Local stub.
pub const VERIFY_TRANSFER_PROOF: Address = addr(0x0100_0002);
/// `0x1000003` VerifyBurnProof (shielded TRC-20). Local stub.
pub const VERIFY_BURN_PROOF: Address = addr(0x0100_0003);
/// `0x1000004` MerkleHash (shielded TRC-20). Local stub.
pub const MERKLE_HASH: Address = addr(0x0100_0004);
/// `0x1000005` RewardBalance (TIP-271 vote). Local stub.
pub const REWARD_BALANCE: Address = addr(0x0100_0005);
/// `0x1000006` IsSrCandidate (vote). Local stub.
pub const IS_SR_CANDIDATE: Address = addr(0x0100_0006);
/// `0x1000007` VoteCount (vote). Local stub.
pub const VOTE_COUNT: Address = addr(0x0100_0007);
/// `0x1000008` UsedVoteCount (vote). Local stub.
pub const USED_VOTE_COUNT: Address = addr(0x0100_0008);
/// `0x1000009` ReceivedVoteCount (vote). Local stub.
pub const RECEIVED_VOTE_COUNT: Address = addr(0x0100_0009);
/// `0x100000a` TotalVoteCount (vote). Local stub.
pub const TOTAL_VOTE_COUNT: Address = addr(0x0100_000a);
/// `0x100000b` GetChainParameter (FreezeV2). Local stub.
pub const GET_CHAIN_PARAMETER: Address = addr(0x0100_000b);
/// `0x100000c` AvailableUnfreezeV2Size (FreezeV2). Local stub.
pub const AVAILABLE_UNFREEZE_V2_SIZE: Address = addr(0x0100_000c);
/// `0x100000d` UnfreezableBalanceV2 (FreezeV2). Local stub.
pub const UNFREEZABLE_BALANCE_V2: Address = addr(0x0100_000d);
/// `0x100000e` ExpireUnfreezeBalanceV2 (FreezeV2). Local stub.
pub const EXPIRE_UNFREEZE_BALANCE_V2: Address = addr(0x0100_000e);
/// `0x100000f` DelegatableResource (FreezeV2). Local stub.
pub const DELEGATABLE_RESOURCE: Address = addr(0x0100_000f);
/// `0x1000010` ResourceV2 (FreezeV2). Local stub.
pub const RESOURCE_V2: Address = addr(0x0100_0010);
/// `0x1000011` CheckUnDelegateResource (FreezeV2). Local stub.
pub const CHECK_UN_DELEGATE_RESOURCE: Address = addr(0x0100_0011);
/// `0x1000012` ResourceUsage (FreezeV2). Local stub.
pub const RESOURCE_USAGE: Address = addr(0x0100_0012);
/// `0x1000013` TotalResource (FreezeV2). Local stub.
pub const TOTAL_RESOURCE: Address = addr(0x0100_0013);
/// `0x1000014` TotalDelegatedResource (FreezeV2). Local stub.
pub const TOTAL_DELEGATED_RESOURCE: Address = addr(0x0100_0014);
/// `0x1000015` TotalAcquiredResource (FreezeV2). Local stub.
pub const TOTAL_ACQUIRED_RESOURCE: Address = addr(0x0100_0015);

/// The complete java-tron precompile set with trace labels, mirroring
/// `TEMPO_PRECOMPILES`. Used by `NetworkConfigs::precompiles_label` and
/// `precompiles` so forge names them in traces and `forge config`.
pub const TRON_PRECOMPILES: &[(&str, Address)] = &[
    ("ECRecover", ECRECOVER),
    ("SHA256", SHA256),
    ("RIPEMD160", RIPEMD160_BROKEN),
    ("Identity", IDENTITY),
    ("ModExp", MODEXP),
    ("BN128Add", BN128_ADD),
    ("BN128Mul", BN128_MUL),
    ("BN128Pairing", BN128_PAIRING),
    ("BatchValidateSign", BATCH_VALIDATE_SIGN),
    ("ValidateMultiSign", VALIDATE_MULTISIGN),
    ("EthRipemd160", ETH_RIPEMD160),
    ("Blake2F", BLAKE2F),
    ("VerifyMintProof", VERIFY_MINT_PROOF),
    ("VerifyTransferProof", VERIFY_TRANSFER_PROOF),
    ("VerifyBurnProof", VERIFY_BURN_PROOF),
    ("MerkleHash", MERKLE_HASH),
    ("RewardBalance", REWARD_BALANCE),
    ("IsSrCandidate", IS_SR_CANDIDATE),
    ("VoteCount", VOTE_COUNT),
    ("UsedVoteCount", USED_VOTE_COUNT),
    ("ReceivedVoteCount", RECEIVED_VOTE_COUNT),
    ("TotalVoteCount", TOTAL_VOTE_COUNT),
    ("GetChainParameter", GET_CHAIN_PARAMETER),
    ("AvailableUnfreezeV2Size", AVAILABLE_UNFREEZE_V2_SIZE),
    ("UnfreezableBalanceV2", UNFREEZABLE_BALANCE_V2),
    ("ExpireUnfreezeBalanceV2", EXPIRE_UNFREEZE_BALANCE_V2),
    ("DelegatableResource", DELEGATABLE_RESOURCE),
    ("ResourceV2", RESOURCE_V2),
    ("CheckUnDelegateResource", CHECK_UN_DELEGATE_RESOURCE),
    ("ResourceUsage", RESOURCE_USAGE),
    ("TotalResource", TOTAL_RESOURCE),
    ("TotalDelegatedResource", TOTAL_DELEGATED_RESOURCE),
    ("TotalAcquiredResource", TOTAL_ACQUIRED_RESOURCE),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_match_java_tron_hex() {
        // Cross-checked against PrecompiledContracts.java address constants.
        assert_eq!(format!("{ECRECOVER:?}"), "0x0000000000000000000000000000000000000001");
        assert_eq!(
            format!("{BATCH_VALIDATE_SIGN:?}"),
            "0x0000000000000000000000000000000000000009"
        );
        assert_eq!(format!("{VALIDATE_MULTISIGN:?}"), "0x000000000000000000000000000000000000000a");
        assert_eq!(format!("{ETH_RIPEMD160:?}"), "0x0000000000000000000000000000000000020003");
        assert_eq!(format!("{BLAKE2F:?}"), "0x0000000000000000000000000000000000020009");
        assert_eq!(format!("{VERIFY_MINT_PROOF:?}"), "0x0000000000000000000000000000000001000001");
        assert_eq!(
            format!("{TOTAL_ACQUIRED_RESOURCE:?}"),
            "0x0000000000000000000000000000000001000015"
        );
    }

    #[test]
    fn chain_ids_match_tron_networks() {
        // Cross-checked against `getChainId` on both networks (mainnet `api.trongrid.io`,
        // Nile `api.nileex.io`) and the broadcast-artifact directory layout.
        assert_eq!(TRON_MAINNET_CHAIN_ID, 728_126_428);
        assert_eq!(TRON_NILE_CHAIN_ID, 3_448_148_188);
    }

    #[test]
    fn precompile_table_has_no_duplicate_addresses() {
        let mut seen = std::collections::HashSet::new();
        for (_, address) in TRON_PRECOMPILES {
            assert!(seen.insert(*address), "duplicate address {address:?} in TRON_PRECOMPILES");
        }
        assert_eq!(TRON_PRECOMPILES.len(), 33);
    }
}
