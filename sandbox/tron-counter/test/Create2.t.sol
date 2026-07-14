// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Counter} from "../src/Counter.sol";
import {Create2Factory} from "../src/Create2Factory.sol";

// Minimal cheatcode surface (the sandbox has no forge-std). The address is the
// canonical foundry cheatcode address address(uint160(uint256(keccak256("hevm
// cheat code")))).
interface IVm {
    function computeCreate2Address(bytes32 salt, bytes32 initCodeHash, address deployer)
        external
        pure
        returns (address);
}

contract Create2Test {
    IVm constant vm = IVm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);
    Create2Factory internal factory;

    function setUp() public {
        factory = new Create2Factory();
    }

    /// Triple consistency across three independent paths that must all agree on
    /// the Tron CREATE2 address `keccak256(0x41 ‖ deployer ‖ salt ‖ hash)[12..]`:
    ///   1. the address the 0xF5 opcode actually deployed the child to;
    ///   2. the `computeCreate2Address` cheatcode (Rust primitives formula);
    ///   3. an independent Solidity re-implementation of the Tron formula.
    /// Non-tautological: the three paths share no code.
    function testCreate2TripleConsistency() public {
        bytes32 salt = bytes32(uint256(0xC0FFEE));
        bytes32 initCodeHash = keccak256(type(Counter).creationCode);

        // Path 1: on-chain deployment via the TVM CREATE2 opcode.
        address deployed = factory.deploy(salt);

        // Path 2: the cheatcode's Tron-scheme computation.
        address viaCheatcode = vm.computeCreate2Address(salt, initCodeHash, address(factory));

        // Path 3: independent Solidity computation of the Tron formula (0x41
        // prefix, 21-byte sender, no 0xff).
        address viaSolidity = address(
            uint160(
                uint256(
                    keccak256(abi.encodePacked(bytes1(0x41), address(factory), salt, initCodeHash))
                )
            )
        );

        require(deployed == viaCheatcode, "cheatcode address != deployed");
        require(deployed == viaSolidity, "solidity address != deployed");

        // The child must be a live, working Counter at that address.
        Counter(deployed).setNumber(9);
        require(Counter(deployed).number() == 9, "child is not a working Counter");
    }
}
