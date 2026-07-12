// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Counter} from "../src/Counter.sol";

/// Minimal broadcast cheatcode surface (no forge-std, matching the rest of this sandbox).
interface Vm {
    function startBroadcast() external;
    function stopBroadcast() external;
}

/// Deploys a Counter on Tron and sets its value under a broadcast so `forge script
/// --broadcast` collects both a CreateSmartContract and a TriggerSmartContract transaction.
contract Deploy {
    Vm internal constant vm = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));

    function run() external {
        vm.startBroadcast();
        Counter counter = new Counter();
        counter.setNumber(42);
        vm.stopBroadcast();
    }
}
