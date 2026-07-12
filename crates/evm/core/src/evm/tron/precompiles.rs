//! java-tron precompiled contracts for tron-revm.
//!
//! Tron's precompile set (`actuator/.../vm/PrecompiledContracts.java` @develop,
//! read via jsdelivr on 2026-07-12) diverges from Ethereum's in several ways that
//! silently corrupt results if left on the vanilla revm set:
//!
//! - **`0x01` ECRecover** returns the Tron 21-byte address form. java-tron wraps
//!   `SignUtils.signatureToAddress` (which is `Hash.sha3omit12` = `0x41 || keccak256(pub)[12..32]`,
//!   a 21-byte value) in `new DataWord(..)`, which left-pads it into the word. So the output word
//!   is `[0u8;11] ++ 0x41 ++ eth20`, i.e. Ethereum's word with byte 11 set to `0x41`. (Verified
//!   from `Hash.sha3omit12`, `DecodeUtil.addressPreFixByte` = mainnet `0x41`, and the
//!   `DataWord(byte[])` left-pad constructor.)
//! - **`0x03`** is NOT ripemd160: java-tron computes `sha256(sha256(data)[0..20])`
//!   (`PrecompiledContracts.Ripempd160.execute`). The real ripemd160 lives at `0x020003`.
//! - **`0x05` ModExp** uses EIP-198 pricing (GQUAD divisor 20, no 200 floor), which is exactly
//!   revm's `modexp::byzantium_run` (`gas_calc::<0, 8, 20, _>` plus the Byzantium mult-complexity
//!   curve). Confirmed identical to `ModExp.getMultComplexity` / `getAdjustedExponentLength` /
//!   `GQUAD_DIVISOR`. The one output-shape difference: for a zero modulus java-tron's
//!   `ModExp.execute` returns `EMPTY_BYTE_ARRAY` (0 bytes) where revm left-pads to `mod_len` zero
//!   bytes, so we wrap it (see [`tron_modexp_run`]).
//! - **`0x09`/`0x0a`** are BatchValidateSign (TIP-43) and ValidateMultiSign (TIP-60), which collide
//!   with Ethereum's blake2f (`0x09`) and KZG point-eval (`0x0a`). We deliberately override both.
//!
//! The `allowTvmOsaka` flag is inactive on mainnet and Nile (probed 2026-07-12),
//! so we use the pre-Osaka semantics throughout: EIP-198 modexp pricing, Istanbul
//! bn128 pricing, no P256Verify (`0x100`), no `isValidAbiEncoding` rejection for
//! `0x09`/`0x0a`, and `allowTvmSelfdestructRestriction` active (arrays capped at
//! 16/5). All cryptographic cores are reused from `revm-precompile`; nothing new
//! is pulled in.

use std::borrow::Cow;

use alloy_evm::precompiles::{DynPrecompile, PrecompileInput};
use alloy_primitives::{Address, Bytes, U256};
use foundry_evm_networks::tron::{
    AVAILABLE_UNFREEZE_V2_SIZE, BATCH_VALIDATE_SIGN, BLAKE2F, BN128_ADD, BN128_MUL, BN128_PAIRING,
    CHECK_UN_DELEGATE_RESOURCE, DELEGATABLE_RESOURCE, ECRECOVER, ETH_RIPEMD160,
    EXPIRE_UNFREEZE_BALANCE_V2, GET_CHAIN_PARAMETER, IDENTITY, IS_SR_CANDIDATE, MERKLE_HASH,
    MODEXP, RECEIVED_VOTE_COUNT, RESOURCE_USAGE, RESOURCE_V2, REWARD_BALANCE, RIPEMD160_BROKEN,
    SHA256, TOTAL_ACQUIRED_RESOURCE, TOTAL_DELEGATED_RESOURCE, TOTAL_RESOURCE, TOTAL_VOTE_COUNT,
    UNFREEZABLE_BALANCE_V2, USED_VOTE_COUNT, VALIDATE_MULTISIGN, VERIFY_BURN_PROOF,
    VERIFY_MINT_PROOF, VERIFY_TRANSFER_PROOF, VOTE_COUNT,
};
use revm::precompile::{
    EthPrecompileOutput, EthPrecompileResult, PrecompileHalt, PrecompileId, PrecompileOutput,
    PrecompileResult, bn254, calc_linear_cost, crypto, hash, identity, modexp, secp256k1,
};

/// Number of energy units charged per verified signature in BatchValidateSign
/// (`0x09`) and ValidateMultiSign (`0x0a`) — `ENGERYPERSIGN` in java-tron.
const ENERGY_PER_SIGN: u64 = 1500;

/// Maximum signatures accepted by BatchValidateSign under
/// `allowTvmSelfdestructRestriction` (`MAX_SIZE` = 16).
const BATCH_MAX_SIZE: usize = 16;

/// Length of a Tron signature (`VMConstant.SIG_LENGTH`), r ‖ s ‖ v.
const SIG_LENGTH: usize = 65;

/// Returns the full java-tron precompile set, ready for
/// [`PrecompilesMap::extend_precompiles`]. Pure precompiles use
/// [`DynPrecompile::new`] (cacheable); state-dependent ones and stubs use
/// [`DynPrecompile::new_stateful`].
///
/// Installed on both `create_evm` and `create_evm_with_inspector` from
/// `inject_tron_extensions`, this both **overrides** revm's `0x03` (ripemd160),
/// `0x05` (Berlin modexp), `0x09` (blake2f) and `0x0a` (KZG) and **adds** the
/// Tron-only addresses (`0x020003`, `0x020009`, and the shielded/vote/FreezeV2
/// stub range). `0x100` P256Verify is intentionally absent (Osaka-gated, inactive
/// on mainnet).
pub fn tron_precompiles() -> Vec<(Address, DynPrecompile)> {
    let mut set = vec![
        // Standard 0x01-0x08. Faithful cores reused from revm-precompile.
        (ECRECOVER, pure(PrecompileId::EcRec, tron_ecrecover_run)),
        (SHA256, pure(PrecompileId::Sha256, hash::sha256_run)),
        (RIPEMD160_BROKEN, pure(custom("tron ripemd(sha256d)"), tron_double_sha256_run)),
        (IDENTITY, pure(PrecompileId::Identity, identity::identity_run)),
        (MODEXP, pure(PrecompileId::ModExp, tron_modexp_run)),
        (BN128_ADD, pure(PrecompileId::Bn254Add, bn128_add_run)),
        (BN128_MUL, pure(PrecompileId::Bn254Mul, bn128_mul_run)),
        (BN128_PAIRING, pure(PrecompileId::Bn254Pairing, bn128_pairing_run)),
        // TIP-43 / TIP-60. Override blake2f / KZG at 0x09 / 0x0a.
        (BATCH_VALIDATE_SIGN, pure(custom("tron batchvalidatesign"), tron_batch_validate_sign_run)),
        (
            VALIDATE_MULTISIGN,
            stateful(custom("tron validatemultisign"), tron_validate_multisign_run),
        ),
        // allowTvmCompatibleEvm set: the real ripemd160 and blake2f.
        (ETH_RIPEMD160, pure(PrecompileId::Ripemd160, hash::ripemd160_run)),
        (BLAKE2F, pure(PrecompileId::Blake2F, revm::precompile::blake2::run)),
    ];

    // Shielded-TRC20 / vote / FreezeV2 precompiles need chain, account or
    // zk-proof state that a bare local backend lacks. They are loud-fail stubs:
    // they charge the correct fixed energy, then revert with an explanatory
    // message (never a fatal `PrecompileError`), matching the 0xD0 CALLTOKEN
    // precedent of failing loudly rather than faking output.
    for &(address, name, energy) in STUBS {
        set.push((address, stub(name, energy)));
    }

    set
}

/// Fixed-energy stub precompiles: `(address, trace name, energy)`. Energies are
/// the `getEnergyForData` constants from `PrecompiledContracts.java`.
const STUBS: &[(Address, &str, u64)] = &[
    (VERIFY_MINT_PROOF, "VerifyMintProof", 150_000),
    (VERIFY_TRANSFER_PROOF, "VerifyTransferProof", 200_000),
    (VERIFY_BURN_PROOF, "VerifyBurnProof", 150_000),
    (MERKLE_HASH, "MerkleHash", 500),
    (REWARD_BALANCE, "RewardBalance", 500),
    (IS_SR_CANDIDATE, "IsSrCandidate", 20),
    (VOTE_COUNT, "VoteCount", 500),
    (USED_VOTE_COUNT, "UsedVoteCount", 20),
    (RECEIVED_VOTE_COUNT, "ReceivedVoteCount", 20),
    (TOTAL_VOTE_COUNT, "TotalVoteCount", 20),
    (GET_CHAIN_PARAMETER, "GetChainParameter", 50),
    (AVAILABLE_UNFREEZE_V2_SIZE, "AvailableUnfreezeV2Size", 50),
    (UNFREEZABLE_BALANCE_V2, "UnfreezableBalanceV2", 50),
    (EXPIRE_UNFREEZE_BALANCE_V2, "ExpireUnfreezeBalanceV2", 50),
    (DELEGATABLE_RESOURCE, "DelegatableResource", 50),
    (RESOURCE_V2, "ResourceV2", 50),
    (CHECK_UN_DELEGATE_RESOURCE, "CheckUnDelegateResource", 50),
    (RESOURCE_USAGE, "ResourceUsage", 50),
    (TOTAL_RESOURCE, "TotalResource", 50),
    (TOTAL_DELEGATED_RESOURCE, "TotalDelegatedResource", 50),
    (TOTAL_ACQUIRED_RESOURCE, "TotalAcquiredResource", 50),
];

/// A custom [`PrecompileId`] from a static label.
const fn custom(label: &'static str) -> PrecompileId {
    PrecompileId::Custom(Cow::Borrowed(label))
}

/// Wraps a pure `(input, gas) -> EthPrecompileResult` core into a cacheable
/// [`DynPrecompile`], mapping non-fatal halts through
/// [`PrecompileOutput::from_eth_result`] (never a fatal `PrecompileError`).
fn pure(id: PrecompileId, f: fn(&[u8], u64) -> EthPrecompileResult) -> DynPrecompile {
    DynPrecompile::new(id, move |input: PrecompileInput<'_>| -> PrecompileResult {
        Ok(PrecompileOutput::from_eth_result(f(input.data, input.gas), input.reservoir))
    })
}

/// Same as [`pure`] but marks the precompile non-cacheable
/// ([`DynPrecompile::new_stateful`]), for cores whose result would depend on
/// account/chain state once that state exists.
fn stateful(id: PrecompileId, f: fn(&[u8], u64) -> EthPrecompileResult) -> DynPrecompile {
    DynPrecompile::new_stateful(id, move |input: PrecompileInput<'_>| -> PrecompileResult {
        Ok(PrecompileOutput::from_eth_result(f(input.data, input.gas), input.reservoir))
    })
}

/// Builds a loud-fail stub that charges `energy` then reverts with a message.
fn stub(name: &'static str, energy: u64) -> DynPrecompile {
    DynPrecompile::new_stateful(
        custom(name),
        move |input: PrecompileInput<'_>| -> PrecompileResult {
            if energy > input.gas {
                return Ok(PrecompileOutput::halt(PrecompileHalt::OutOfGas, input.reservoir));
            }
            let msg = format!(
                "tron precompile {name} is a stage-2 stub (needs chain/zk state; unavailable in local tests)"
            );
            Ok(PrecompileOutput::revert(energy, Bytes::from(msg.into_bytes()), input.reservoir))
        },
    )
}

/// `0x06` alt_bn128 addition with Istanbul pricing (150).
fn bn128_add_run(input: &[u8], gas_limit: u64) -> EthPrecompileResult {
    bn254::run_add(input, bn254::add::ISTANBUL_ADD_GAS_COST, gas_limit)
}

/// `0x07` alt_bn128 scalar multiplication with Istanbul pricing (6000).
fn bn128_mul_run(input: &[u8], gas_limit: u64) -> EthPrecompileResult {
    bn254::run_mul(input, bn254::mul::ISTANBUL_MUL_GAS_COST, gas_limit)
}

/// `0x08` alt_bn128 pairing with Istanbul pricing (34000·k + 45000).
fn bn128_pairing_run(input: &[u8], gas_limit: u64) -> EthPrecompileResult {
    bn254::run_pair(
        input,
        bn254::pair::ISTANBUL_PAIR_PER_POINT,
        bn254::pair::ISTANBUL_PAIR_BASE,
        gas_limit,
    )
}

/// `0x01` ECRecover in java-tron's 21-byte-address form.
///
/// Reuses revm's secp256k1 recover (identical curve, identical 3000 energy,
/// identical input parsing and validity rules), then rewrites the output word
/// from Ethereum's `[0u8;12] ++ eth20` to Tron's `[0u8;11] ++ 0x41 ++ eth20` by
/// setting byte 11 to the mainnet address prefix `0x41`. On recovery failure
/// java-tron returns an empty result, matching revm's empty output.
pub fn tron_ecrecover_run(input: &[u8], gas_limit: u64) -> EthPrecompileResult {
    let out = secp256k1::ec_recover_run(input, gas_limit)?;
    if out.bytes.len() == 32 {
        let mut word = out.bytes.to_vec();
        word[11] = 0x41;
        Ok(EthPrecompileOutput::new(out.gas_used, word.into()))
    } else {
        Ok(out)
    }
}

/// `0x03` java-tron's broken "ripemd160": `sha256(sha256(data)[0..20])`
/// (`PrecompiledContracts.Ripempd160.execute`). Energy 600 + 120/word.
pub fn tron_double_sha256_run(input: &[u8], gas_limit: u64) -> EthPrecompileResult {
    let cost = calc_linear_cost(input.len(), 600, 120);
    if cost > gas_limit {
        return Err(PrecompileHalt::OutOfGas);
    }
    let first = crypto().sha256(input);
    let out = crypto().sha256(&first[..20]);
    Ok(EthPrecompileOutput::new(cost, out.to_vec().into()))
}

/// `0x05` ModExp in java-tron form: EIP-198 pricing and math via revm's
/// `byzantium_run`, but returning an **empty** output when the modulus is zero.
///
/// java-tron `ModExp.execute` returns `EMPTY_BYTE_ARRAY` (0 bytes) for
/// `isZero(mod)`, whereas revm left-pads the result to `mod_len` zero bytes, so
/// `RETURNDATASIZE` would be `mod_len` instead of 0. The gas cost is unchanged
/// (the modexp price does not depend on the modulus *value*), and the zero-value
/// result of a *non-zero* modulus (e.g. `0^2 mod 5 = 0`) is left untouched — only
/// a zero *modulus* collapses to empty, exactly as java-tron does.
pub fn tron_modexp_run(input: &[u8], gas_limit: u64) -> EthPrecompileResult {
    let out = modexp::byzantium_run(input, gas_limit)?;
    if modexp_modulus_is_zero(input) {
        return Ok(EthPrecompileOutput::new(out.gas_used, Bytes::new()));
    }
    Ok(out)
}

/// Whether the ModExp modulus is zero, matching java-tron's `isZero(mod)` where
/// `mod = parseArg(data, 96 + baseLen + expLen, modLen)`. The input layout is
/// `baseLen | expLen | modLen` (three 32-byte big-endian words) followed by the
/// base, exponent and modulus bytes; missing trailing bytes are implicitly zero
/// (right-padding), matching revm's parsing.
fn modexp_modulus_is_zero(input: &[u8]) -> bool {
    // Reads the 32-byte big-endian word at word index `i`, right-padding with
    // zeroes when `input` is short (as revm's `right_pad_with_offset` does).
    let read_word = |i: usize| -> U256 {
        let start = i * 32;
        let mut buf = [0u8; 32];
        if start < input.len() {
            let end = (start + 32).min(input.len());
            buf[..end - start].copy_from_slice(&input[start..end]);
        }
        U256::from_be_bytes(buf)
    };
    let base_len = read_word(0);
    let exp_len = read_word(1);
    let mod_len = read_word(2);
    if mod_len.is_zero() {
        return true;
    }
    // Byte offset where the modulus starts: 96 + baseLen + expLen.
    let start = U256::from(96u64).saturating_add(base_len).saturating_add(exp_len);
    let Ok(start) = usize::try_from(start) else {
        // Offset past addressable memory: every modulus byte is implicitly zero.
        return true;
    };
    let mod_len = usize::try_from(mod_len).unwrap_or(usize::MAX);
    // Bytes present in `input`; anything past the end is an implicit zero.
    input.get(start..).unwrap_or_default().iter().take(mod_len).all(|&b| b == 0)
}

/// `0x09` BatchValidateSign (TIP-43): `res[i] = 1` iff the `i`-th recovered
/// address matches `addresses[i]` (compared on the low 20 bytes), else 0.
///
/// Energy is `((len/32 - 5) / 6) * 1500` (`getEnergyForData`). We take the
/// single-threaded `isConstantCall()` branch — behaviour is identical to the
/// thread-pool path. Any malformed calldata is collapsed to a 32-byte zero
/// result, matching java-tron's outer `try/catch`. `allowTvmOsaka` is off, so the
/// `isValidAbiEncoding` reject path is not taken.
pub fn tron_batch_validate_sign_run(input: &[u8], gas_limit: u64) -> EthPrecompileResult {
    let cost = per_sign_energy(input.len(), 6);
    if cost > gas_limit {
        return Err(PrecompileHalt::OutOfGas);
    }
    Ok(EthPrecompileOutput::new(cost, batch_validate_sign_execute(input).to_vec().into()))
}

/// `0x0a` ValidateMultiSign (TIP-60). The local revm account model carries no
/// Tron permission structures, so this always takes java-tron's `account == null`
/// branch and returns `DATA_FALSE` (32 zero bytes). Energy is
/// `((len/32 - 5) / 5) * 1500`. Multisig-dependent contracts require fork-mode
/// account state to be golden-tested; that is a documented stage-2 limitation.
pub fn tron_validate_multisign_run(input: &[u8], gas_limit: u64) -> EthPrecompileResult {
    let cost = per_sign_energy(input.len(), 5);
    if cost > gas_limit {
        return Err(PrecompileHalt::OutOfGas);
    }
    Ok(EthPrecompileOutput::new(cost, vec![0u8; 32].into()))
}

/// Energy shared by `0x09` (`items_per_sign` = 6) and `0x0a` (= 5):
/// `((len/WORD - 5) / items_per_sign) * 1500`. Java integer division truncates
/// toward zero, so any `len` under five words costs nothing.
fn per_sign_energy(len: usize, items_per_sign: u64) -> u64 {
    let words = (len / 32) as u64;
    words.checked_sub(5).map(|w| (w / items_per_sign) * ENERGY_PER_SIGN).unwrap_or(0)
}

/// Executes BatchValidateSign against `data`, returning the 32-byte flag word.
/// Faithfully replicates `PrecompiledContracts.BatchValidateSign.doExecute` with
/// `allowTvmSelfdestructRestriction` active: any index/parse failure yields the
/// all-zero word (the java outer `catch`).
fn batch_validate_sign_execute(data: &[u8]) -> [u8; 32] {
    batch_validate_sign_inner(data).unwrap_or([0u8; 32])
}

/// The fallible core of [`batch_validate_sign_execute`]; `None` (mapped to the
/// zero word) is java-tron's `Pair.of(true, new byte[32])` catch-all.
fn batch_validate_sign_inner(data: &[u8]) -> Option<[u8; 32]> {
    let hash = word_at(data, 0)?;
    // words[1] / words[2] are byte offsets to the signatures / addresses arrays.
    let sig_off = word_int_value_safe(&word_at(data, 1)?) / 32;
    let addr_off = word_int_value_safe(&word_at(data, 2)?) / 32;

    // allowTvmSelfdestructRestriction: both array lengths are capped at 16.
    let sig_len = word_int_value_safe(&word_at(data, sig_off)?);
    let addr_len = word_int_value_safe(&word_at(data, addr_off)?);
    if sig_len > BATCH_MAX_SIZE || addr_len > BATCH_MAX_SIZE {
        return Some([0u8; 32]);
    }

    // extractSigArray: each 65-byte signature lives at
    // (words[sig_off + i + 1]/32 + sig_off + 2) words into `data`.
    let mut signatures = Vec::with_capacity(sig_len);
    for i in 0..sig_len {
        let bytes_off = word_int_value_safe(&word_at(data, sig_off + i + 1)?) / 32;
        let start = bytes_off.checked_add(sig_off)?.checked_add(2)?.checked_mul(32)?;
        let end = start.checked_add(SIG_LENGTH)?;
        let sig: [u8; SIG_LENGTH] = data.get(start..end)?.try_into().ok()?;
        signatures.push(sig);
    }

    // extractBytes32Array: addresses are consecutive words after the length word.
    let mut addresses = Vec::with_capacity(addr_len);
    for i in 0..addr_len {
        addresses.push(word_at(data, addr_off + i + 1)?);
    }

    let cnt = signatures.len();
    if cnt == 0 || cnt > BATCH_MAX_SIZE || signatures.len() != addresses.len() {
        return Some([0u8; 32]);
    }

    let mut res = [0u8; 32];
    for i in 0..cnt {
        if let Some(recovered) = recover_eth_address(&signatures[i], &hash)
            && recovered == addresses[i][12..32]
        {
            res[i] = 1;
        }
    }
    Some(res)
}

/// Recovers the 20-byte Ethereum address a `0x09`/`0x0a` signature signed for
/// `hash`, replicating java-tron's `recoverAddrBySign` (`PrecompiledContracts`
/// `:371-386`). `Rsv.fromSignature` bumps a bare recovery id below 27 by 27,
/// then `ECDSASignature.validateComponents` (`ECKey.java:923-940`) rejects any
/// `v` outside `{27, 28}` before recovery — so only raw `v ∈ {0, 1, 27, 28}` is
/// accepted (recovery id 0 or 1). The chain-tagged `header >= 31 → -4` branch in
/// `signatureToKeyBytes` is unreachable here because `validateComponents` has
/// already rejected every `v != 27/28`. Returns `None` on any invalid component.
/// The comparison is on the low 20 bytes, matching `DataWord.equalAddressByteArray`.
fn recover_eth_address(sig: &[u8; SIG_LENGTH], hash: &[u8; 32]) -> Option<[u8; 20]> {
    let mut v = sig[64];
    // Rsv.fromSignature: a bare recovery id (< 27) is lifted to the 27/28 form.
    if v < 27 {
        v = v.wrapping_add(27);
    }
    // validateComponents: only v ∈ {27, 28} passes; everything else (raw recid
    // > 1, or chain-tagged 31..34) is rejected before any recovery is attempted.
    if v != 27 && v != 28 {
        return None;
    }
    let recid = v - 27;
    let mut sig64 = [0u8; 64];
    sig64.copy_from_slice(&sig[..64]);
    let word = crypto().secp256k1_ecrecover(&sig64, recid, hash).ok()?;
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&word[12..32]);
    Some(addr)
}

/// Returns the 32-byte word at word index `i` of `data`, or `None` if `data` is
/// shorter than `(i + 1) * 32` bytes (`DataWord.parseArray` drops any trailing
/// sub-word, so we require a full word).
fn word_at(data: &[u8], i: usize) -> Option<[u8; 32]> {
    let start = i.checked_mul(32)?;
    let end = start.checked_add(32)?;
    data.get(start..end)?.try_into().ok()
}

/// Mirrors `DataWord.intValueSafe`: the numeric value of a word, saturated to
/// `i32::MAX` when more than four bytes are significant or the low 32 bits have
/// the sign bit set. Saturated offsets fall out of bounds and are handled as a
/// parse failure upstream, matching java-tron's `ArrayIndexOutOfBounds` catch.
fn word_int_value_safe(word: &[u8; 32]) -> usize {
    let bytes_occupied = 32 - word.iter().take_while(|&&b| b == 0).count();
    let low4 = u32::from_be_bytes([word[28], word[29], word[30], word[31]]);
    if bytes_occupied > 4 || low4 > i32::MAX as u32 { i32::MAX as usize } else { low4 as usize }
}

#[cfg(test)]
mod tests {
    use super::{super::TronEvmFactory, *};
    use alloy_evm::{EthEvmFactory, Evm, EvmEnv, EvmFactory};
    use alloy_primitives::{TxKind, hex};
    use revm::{
        context::{CfgEnv, TxEnv},
        database::{CacheDB, EmptyDB},
        primitives::hardfork::SpecId,
    };

    // The canonical secp256k1 ecrecover vector (also revm's): message hash, v, r,
    // s, and the recovered 20-byte Ethereum address.
    const HASH: &str = "456e9aea5e197a1f1af7a3e85a3212fa4049a3ba34c2289b4c860fc0b0c64ef3";
    const R: &str = "9242685bf161793cc25603c231bc2f568eb630ea16aa137d2664ac8038825608";
    const S: &str = "4f8ae3bd7535248d0bd448298cc2e2071e56992d0774dc340c368ae950852ada";
    const ETH_ADDR: &str = "7156526fbd7a3c72969b54f64e42c10fbb768c8a";

    fn h(s: &str) -> Vec<u8> {
        hex::decode(s).unwrap()
    }

    // ---------------------------------------------------------------------
    // 0x01 ECRecover: 21-byte Tron address form (byte 11 = 0x41).
    // ---------------------------------------------------------------------

    #[test]
    fn ecrecover_returns_tron_21_byte_address_form() {
        // input = hash ‖ v(32, =28) ‖ r ‖ s.
        let input = h(&format!("{HASH}{:064x}{R}{S}", 28));
        let out = tron_ecrecover_run(&input, 3000).unwrap();
        assert_eq!(out.gas_used, 3000, "ECRecover energy is 3000");

        let tron = hex::encode(&out.bytes);
        // 11 zero bytes, then 0x41, then the 20-byte eth address.
        assert_eq!(tron, format!("{}41{ETH_ADDR}", "00".repeat(11)));

        // Contrast: the vanilla revm 0x01 returns the Ethereum form (12 zeros).
        let eth = secp256k1::ec_recover_run(&input, 3000).unwrap();
        assert_eq!(hex::encode(&eth.bytes), format!("{}{ETH_ADDR}", "00".repeat(12)));
        assert_ne!(out.bytes, eth.bytes, "Tron 0x01 must differ from Ethereum 0x01");
        // The only difference is byte 11: 0x41 vs 0x00; the address bytes match.
        assert_eq!(out.bytes[12..32], eth.bytes[12..32]);
        assert_eq!(out.bytes[11], 0x41);
        assert_eq!(eth.bytes[11], 0x00);
    }

    #[test]
    fn ecrecover_invalid_signature_returns_empty() {
        // v = 26 is out of the {27, 28} range -> empty, like java-tron.
        let input = h(&format!("{HASH}{:064x}{R}{S}", 26));
        let out = tron_ecrecover_run(&input, 3000).unwrap();
        assert!(out.bytes.is_empty(), "invalid recovery must yield empty output");
    }

    // ---------------------------------------------------------------------
    // 0x03: double-sha256, not ripemd160.
    // ---------------------------------------------------------------------

    #[test]
    fn tron_0x03_is_double_sha256_not_ripemd() {
        let data = b"abc";
        let out = tron_double_sha256_run(data, 1_000_000).unwrap();
        // sha256(sha256("abc")[0..20]) computed independently with sha2.
        let first = crypto().sha256(data);
        let expected = crypto().sha256(&first[..20]);
        assert_eq!(out.bytes.as_ref(), expected.as_slice());
        assert_eq!(hex::encode(&out.bytes), h_double_sha256_abc());
        assert_eq!(out.gas_used, 600 + 120, "energy 600 + 120/word for one word");

        // Contrast: the real ripemd160 (revm 0x03 / Tron 0x020003) differs.
        let real_ripemd = hash::ripemd160_run(data, 1_000_000).unwrap();
        assert_ne!(out.bytes, real_ripemd.bytes, "0x03 must not be ripemd160");
    }

    fn h_double_sha256_abc() -> String {
        // Independently computed: sha256(sha256("abc")[:20]).
        "6b6ea134869d649e6f52658be1a5691e37db83c6b8b72b0f1b36d4f849929c9e".to_string()
    }

    // ---------------------------------------------------------------------
    // 0x05: EIP-198 pricing (divisor 20), not Berlin (divisor 3, 200 floor).
    // ---------------------------------------------------------------------

    #[test]
    fn modexp_uses_eip198_pricing_divisor_20() {
        // 3^2 mod 5: base_len=exp_len=mod_len=1. Byzantium cost = mult_complexity(1)
        // * adj_exp / 20 = 1 * 1 / 20 = 0 (no floor). Berlin floors it to 200.
        let input = h(&format!("{:064x}{:064x}{:064x}0302 05", 1, 1, 1).replace(' ', ""));
        let tron = modexp::byzantium_run(&input, 1_000_000).unwrap();
        assert_eq!(tron.gas_used, 0, "EIP-198 has no 200 floor");
        assert_eq!(tron.bytes.as_ref(), &[4u8], "3^2 mod 5 = 4");

        let berlin = modexp::berlin_run(&input, 1_000_000).unwrap();
        assert_eq!(berlin.gas_used, 200, "Berlin floors to 200");
        assert_ne!(tron.gas_used, berlin.gas_used, "Tron modexp must reprice vs Berlin");
    }

    #[test]
    fn modexp_zero_modulus_returns_empty_output() {
        // base_len=exp_len=mod_len=1, base=03, exp=02, mod=00. java-tron's
        // ModExp.execute returns EMPTY_BYTE_ARRAY for a zero modulus, whereas
        // revm's byzantium_run left-pads to mod_len zero bytes.
        let input = h(&format!("{:064x}{:064x}{:064x}030200", 1, 1, 1));
        let tron = tron_modexp_run(&input, 1_000_000).unwrap();
        assert!(tron.bytes.is_empty(), "zero modulus -> empty output (RETURNDATASIZE 0)");

        let revm = modexp::byzantium_run(&input, 1_000_000).unwrap();
        assert_eq!(
            revm.bytes.as_ref(),
            &[0u8],
            "revm left-pads to one zero byte (RETURNDATASIZE 1)"
        );
        assert_eq!(tron.gas_used, revm.gas_used, "gas is identical (independent of modulus value)");
        assert_ne!(tron.bytes, revm.bytes, "Tron modexp must reshape the zero-modulus output");
    }

    #[test]
    fn modexp_zero_result_nonzero_modulus_keeps_modlen() {
        // 0^2 mod 5 = 0: the result is zero but the *modulus* (5) is not, so the
        // output must stay mod_len bytes and match revm exactly -- only a zero
        // modulus collapses to empty, not a zero-valued result.
        let input = h(&format!("{:064x}{:064x}{:064x}000205", 1, 1, 1));
        let tron = tron_modexp_run(&input, 1_000_000).unwrap();
        let revm = modexp::byzantium_run(&input, 1_000_000).unwrap();
        assert_eq!(tron.bytes, revm.bytes, "non-zero modulus is passed through unchanged");
        assert_eq!(tron.bytes.len(), 1, "a non-zero modulus keeps the mod_len-byte output");
    }

    // ---------------------------------------------------------------------
    // 0x09 BatchValidateSign.
    // ---------------------------------------------------------------------

    /// Builds a standard ABI encoding of
    /// `batchvalidatesign(bytes32 hash, bytes[] signatures, address[] addresses)`
    /// (which java-tron's `extractSigArray` / `extractBytes32Array` decode). Each
    /// signature is exactly 65 bytes, so every element occupies 4 words.
    fn abi_batch(hash: &[u8; 32], entries: &[([u8; 65], [u8; 20])]) -> Vec<u8> {
        let n = entries.len();
        let word_usize = |v: usize| {
            let mut w = [0u8; 32];
            w[24..32].copy_from_slice(&(v as u64).to_be_bytes());
            w
        };
        let mut words: Vec<[u8; 32]> = Vec::new();
        // Head: hash, offset to sigs (0x60), offset to addrs.
        words.push(*hash);
        words.push(word_usize(0x60));
        let addr_off = 96 + (1 + 5 * n) * 32;
        words.push(word_usize(addr_off));
        // Signatures block: length, per-element offsets, then [len ‖ 3 data words].
        words.push(word_usize(n));
        for i in 0..n {
            words.push(word_usize(32 * n + i * 128));
        }
        for (sig, _) in entries {
            words.push(word_usize(SIG_LENGTH));
            let mut buf = [0u8; 96];
            buf[..SIG_LENGTH].copy_from_slice(sig);
            words.push(buf[0..32].try_into().unwrap());
            words.push(buf[32..64].try_into().unwrap());
            words.push(buf[64..96].try_into().unwrap());
        }
        // Addresses block (static elements): length then one word per address.
        words.push(word_usize(n));
        for (_, addr) in entries {
            let mut w = [0u8; 32];
            w[12..32].copy_from_slice(addr);
            words.push(w);
        }
        words.concat()
    }

    fn valid_sig() -> [u8; 65] {
        let mut sig = [0u8; 65];
        sig[..32].copy_from_slice(&h(R));
        sig[32..64].copy_from_slice(&h(S));
        sig[64] = 28; // v; normalized to recid 1.
        sig
    }

    #[test]
    fn batch_validate_sign_flags_matches_and_mismatches() {
        let hash: [u8; 32] = h(HASH).try_into().unwrap();
        let good_addr: [u8; 20] = h(ETH_ADDR).try_into().unwrap();
        let wrong_addr = [0u8; 20];
        let input = abi_batch(&hash, &[(valid_sig(), good_addr), (valid_sig(), wrong_addr)]);

        let out = tron_batch_validate_sign_run(&input, 1_000_000).unwrap();
        // res[0] = 1 (signature recovers to good_addr), res[1] = 0 (wrong addr).
        let mut expected = [0u8; 32];
        expected[0] = 1;
        assert_eq!(out.bytes.as_ref(), &expected, "res=[1,0,0,..]");

        // Energy: 17 words -> ((17 - 5) / 6) * 1500 = 3000.
        assert_eq!(input.len() / 32, 17);
        assert_eq!(out.gas_used, 3000);

        // Contrast: Ethereum's 0x09 is blake2f, which rejects this 544-byte input.
        assert!(
            revm::precompile::blake2::run(&input, 1_000_000).is_err(),
            "eth blake2f must reject the batchvalidatesign calldata"
        );
    }

    #[test]
    fn recover_eth_address_only_accepts_v_27_28_form() {
        let hash: [u8; 32] = h(HASH).try_into().unwrap();
        let expected: [u8; 20] = h(ETH_ADDR).try_into().unwrap();
        let sig_with_v = |v: u8| -> [u8; SIG_LENGTH] {
            let mut sig = [0u8; SIG_LENGTH];
            sig[..32].copy_from_slice(&h(R));
            sig[32..64].copy_from_slice(&h(S));
            sig[64] = v;
            sig
        };

        // v = 28 (recovery id 1) recovers the canonical address; a bare v = 1 is
        // lifted to 28 by Rsv.fromSignature and recovers the same address.
        assert_eq!(recover_eth_address(&sig_with_v(28), &hash), Some(expected));
        assert_eq!(recover_eth_address(&sig_with_v(1), &hash), Some(expected));

        // validateComponents rejects every v ∉ {27, 28}. v = 31 (and v = 4, which
        // normalizes to 31) were previously mis-mapped to recovery id 0 via a
        // chain-tag -4 branch that is unreachable on this java-tron path; they
        // must now return None. Likewise v = 29 (recovery id 2) is out of range.
        assert_eq!(recover_eth_address(&sig_with_v(31), &hash), None);
        assert_eq!(recover_eth_address(&sig_with_v(4), &hash), None);
        assert_eq!(recover_eth_address(&sig_with_v(29), &hash), None);
    }

    #[test]
    fn batch_validate_sign_rejects_chain_tagged_v() {
        // The valid signature with its raw v swapped to 31 must NOT verify:
        // java-tron's validateComponents accepts only v ∈ {27, 28}. Previously
        // the raw 31 recovered with recovery id 0 and could flag a false match.
        let hash: [u8; 32] = h(HASH).try_into().unwrap();
        let good_addr: [u8; 20] = h(ETH_ADDR).try_into().unwrap();
        let mut sig = valid_sig();
        sig[64] = 31;
        let input = abi_batch(&hash, &[(sig, good_addr)]);
        let out = tron_batch_validate_sign_run(&input, 1_000_000).unwrap();
        assert_eq!(out.bytes.as_ref(), &[0u8; 32], "v=31 must not verify (res[0]=0)");

        // Sanity: the same signature with the correct v=28 does verify, so the
        // rejection above is the v-gate, not a broken vector.
        let ok = abi_batch(&hash, &[(valid_sig(), good_addr)]);
        let mut expected = [0u8; 32];
        expected[0] = 1;
        assert_eq!(tron_batch_validate_sign_run(&ok, 1_000_000).unwrap().bytes.as_ref(), &expected);
    }

    #[test]
    fn batch_validate_sign_short_input_is_zero_word_and_free() {
        // Fewer than five words: energy 0, all-zero result (java outer catch).
        let out = tron_batch_validate_sign_run(&[0u8; 64], 1_000_000).unwrap();
        assert_eq!(out.gas_used, 0);
        assert_eq!(out.bytes.as_ref(), &[0u8; 32]);
    }

    // ---------------------------------------------------------------------
    // 0x0a ValidateMultiSign.
    // ---------------------------------------------------------------------

    #[test]
    fn validate_multisign_returns_false_and_charges_energy() {
        // 11 words -> ((11 - 5) / 5) * 1500 = 1500. No permission state locally,
        // so the account==null branch returns DATA_FALSE (32 zero bytes).
        let input = vec![0u8; 11 * 32];
        let out = tron_validate_multisign_run(&input, 1_000_000).unwrap();
        assert_eq!(out.bytes.as_ref(), &[0u8; 32]);
        assert_eq!(out.gas_used, 1500);

        // Contrast: Ethereum's 0x0a is KZG point-eval, which accepts exactly 192
        // input bytes and rejects this 352-byte word-aligned calldata. Tron yields
        // a 32-byte DATA_FALSE word; Ethereum errors, so the two never coincide.
        assert!(
            revm::precompile::kzg_point_evaluation::run(&input, 1_000_000).is_err(),
            "eth KZG point-eval must reject the validatemultisign calldata"
        );
    }

    // ---------------------------------------------------------------------
    // OOG halts are non-fatal (never Err(PrecompileError)).
    // ---------------------------------------------------------------------

    #[test]
    fn insufficient_gas_halts_not_fatally() {
        // Each core returns Ok(halt) rather than Err(PrecompileError).
        assert!(matches!(
            PrecompileOutput::from_eth_result(tron_double_sha256_run(b"abc", 1), 0).status,
            revm::precompile::PrecompileStatus::Halt(PrecompileHalt::OutOfGas)
        ));
        assert!(matches!(
            PrecompileOutput::from_eth_result(
                tron_ecrecover_run(&h(&format!("{HASH}{:064x}{R}{S}", 28)), 1),
                0
            )
            .status,
            revm::precompile::PrecompileStatus::Halt(PrecompileHalt::OutOfGas)
        ));
    }

    // ---------------------------------------------------------------------
    // End-to-end: the injection actually overrides revm's precompiles inside
    // TronEvmFactory. These exercise inject_tron_extensions, not just the cores.
    // ---------------------------------------------------------------------

    fn cancun_env() -> EvmEnv {
        EvmEnv { cfg_env: CfgEnv::new_with_spec(SpecId::CANCUN), ..Default::default() }
    }

    /// Creation bytecode that STATICCALLs precompile `address` with `input`
    /// (stored from memory 0) and RETURNs its returndata verbatim, so a creation
    /// tx's `output()` is the precompile output.
    fn staticcall_creation(address: Address, input: &[u8]) -> Vec<u8> {
        let mut code: Vec<u8> = Vec::new();
        // Store the input word by word (last word zero-padded).
        let mut offset = 0usize;
        for chunk in input.chunks(32) {
            let mut word = [0u8; 32];
            word[..chunk.len()].copy_from_slice(chunk);
            code.push(0x7f); // PUSH32 word
            code.extend_from_slice(&word);
            code.push(0x61); // PUSH2 offset (>1 byte once inputs exceed 256 bytes)
            code.extend_from_slice(&(offset as u16).to_be_bytes());
            code.push(0x52); // MSTORE
            offset += 32;
        }
        // STATICCALL(gas, address, argsOffset=0, argsLen=input.len, retOff=0, retLen=0).
        code.push(0x5f); // PUSH0 retLen
        code.push(0x5f); // PUSH0 retOffset
        code.push(0x61); // PUSH2 argsLen
        code.extend_from_slice(&(input.len() as u16).to_be_bytes());
        code.push(0x5f); // PUSH0 argsOffset
        code.push(0x73); // PUSH20 address
        code.extend_from_slice(address.as_slice());
        code.push(0x5a); // GAS
        code.push(0xfa); // STATICCALL
        code.push(0x50); // POP success
        // RETURNDATACOPY(dest=0, offset=0, size=RETURNDATASIZE); RETURN(0, RETURNDATASIZE).
        code.push(0x3d); // RETURNDATASIZE (size)
        code.push(0x5f); // PUSH0 (returndata offset)
        code.push(0x5f); // PUSH0 (dest offset)
        code.push(0x3e); // RETURNDATACOPY
        code.push(0x3d); // RETURNDATASIZE
        code.push(0x5f); // PUSH0 (mem offset)
        code.push(0xf3); // RETURN
        code
    }

    fn create_tx(bytecode: Vec<u8>) -> TxEnv {
        TxEnv {
            kind: TxKind::Create,
            data: bytecode.into(),
            gas_limit: 10_000_000,
            ..Default::default()
        }
    }

    fn tron_precompile_output(address: Address, input: &[u8]) -> Vec<u8> {
        let mut evm = TronEvmFactory.create_evm(CacheDB::<EmptyDB>::default(), cancun_env());
        let out = evm.transact_raw(create_tx(staticcall_creation(address, input))).unwrap();
        assert!(out.result.is_success(), "tron staticcall must succeed: {:?}", out.result);
        out.result.output().unwrap().to_vec()
    }

    fn eth_precompile_output(address: Address, input: &[u8]) -> Vec<u8> {
        let mut evm =
            EthEvmFactory::default().create_evm(CacheDB::<EmptyDB>::default(), cancun_env());
        let out = evm.transact_raw(create_tx(staticcall_creation(address, input))).unwrap();
        assert!(out.result.is_success(), "eth staticcall must succeed: {:?}", out.result);
        out.result.output().unwrap().to_vec()
    }

    #[test]
    fn e2e_0x03_override_takes_effect_in_tron_evm() {
        // Through the full TronEvmFactory, 0x03 is double-sha256; through vanilla
        // revm it is ripemd160. Same address, different result: proves the
        // extend_precompiles injection overrides revm's default set.
        let data = b"abc";
        let tron = tron_precompile_output(RIPEMD160_BROKEN, data);
        assert_eq!(hex::encode(&tron), h_double_sha256_abc());

        let eth = eth_precompile_output(RIPEMD160_BROKEN, data);
        let first = crypto().sha256(data);
        assert_eq!(hex::encode(&crypto().ripemd160(data)[..]), hex::encode(&eth[..]));
        assert_ne!(tron, eth, "TronEvmFactory 0x03 must override revm ripemd160");
        let _ = first;
    }

    #[test]
    fn e2e_0x01_override_takes_effect_in_tron_evm() {
        let input = h(&format!("{HASH}{:064x}{R}{S}", 28));
        let tron = tron_precompile_output(ECRECOVER, &input);
        assert_eq!(hex::encode(&tron), format!("{}41{ETH_ADDR}", "00".repeat(11)));

        let eth = eth_precompile_output(ECRECOVER, &input);
        assert_eq!(hex::encode(&eth), format!("{}{ETH_ADDR}", "00".repeat(12)));
        assert_ne!(tron, eth);
    }

    #[test]
    fn e2e_0x09_overrides_blake2f_in_tron_evm() {
        // Address 0x09 is blake2f on Ethereum but BatchValidateSign on Tron.
        let hash: [u8; 32] = h(HASH).try_into().unwrap();
        let good_addr: [u8; 20] = h(ETH_ADDR).try_into().unwrap();
        let input = abi_batch(&hash, &[(valid_sig(), good_addr)]);
        let tron = tron_precompile_output(BATCH_VALIDATE_SIGN, &input);
        let mut expected = [0u8; 32];
        expected[0] = 1;
        assert_eq!(tron, expected, "Tron 0x09 recovers and matches -> res[0]=1");

        // Ethereum's blake2f rejects this input, so the STATICCALL fails and
        // returndata is empty.
        let eth = eth_precompile_output(BATCH_VALIDATE_SIGN, &input);
        assert!(eth.is_empty(), "eth blake2f rejects the input -> empty returndata");
    }

    #[test]
    fn e2e_0x0a_overrides_kzg_in_tron_evm() {
        // Address 0x0a is KZG point-eval on Ethereum but ValidateMultiSign on Tron.
        // The 352-byte all-zero calldata charges 1500 energy and, with no local
        // permission state, returns DATA_FALSE (32 zero bytes) on Tron.
        let input = vec![0u8; 11 * 32];
        let tron = tron_precompile_output(VALIDATE_MULTISIGN, &input);
        assert_eq!(tron, [0u8; 32], "Tron 0x0a returns the DATA_FALSE word");

        // Ethereum's KZG point-eval requires exactly 192 input bytes, so it rejects
        // this 352-byte input: the STATICCALL fails and returndata is empty. A
        // 32-byte word vs empty returndata proves the extend_precompiles injection
        // overrides revm's KZG at 0x0a.
        let eth = eth_precompile_output(VALIDATE_MULTISIGN, &input);
        assert!(eth.is_empty(), "eth KZG point-eval rejects the input -> empty returndata");
        assert_ne!(tron, eth, "TronEvmFactory 0x0a must override revm KZG point-eval");
    }
}
