// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Counter} from "./Counter.sol";

/// Deploys `Counter` via CREATE2 so tests can compare the on-chain address the
/// TVM 0xF5 opcode produces against the Tron CREATE2 formula.
contract Create2Factory {
    function deploy(bytes32 salt) external returns (address) {
        return address(new Counter{salt: salt}());
    }

    /// keccak256 of the exact `Counter` init code this factory embeds. Lets the
    /// live golden fetch the init-code hash the node itself hashes for CREATE2,
    /// so the parity check is independent of compiler metadata.
    function childCodeHash() external pure returns (bytes32) {
        return keccak256(type(Counter).creationCode);
    }
}
