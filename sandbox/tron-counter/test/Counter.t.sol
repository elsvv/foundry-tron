// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Counter} from "../src/Counter.sol";

contract CounterTest {
    Counter internal counter;

    function setUp() public {
        counter = new Counter();
        counter.setNumber(41);
    }

    function testIncrement() public {
        counter.increment();
        require(counter.number() == 42, "increment failed");
    }

    function testTronChainId() public view {
        require(block.chainid == 728126428, "chain id is not Tron mainnet");
    }

    function testTransientStorageCancun() public {
        // TSTORE/TLOAD available only from Cancun - proves evm_version
        assembly {
            tstore(0, 42)
            if iszero(eq(tload(0), 42)) { revert(0, 0) }
        }
    }

    function testNonPayableGuardWithTvmOpcodes() public {
        // Любой вызов non-payable функции tron-solc-контракта проходит через
        // guard CALLVALUE -> CALLTOKENID (0xD3) -> CALLTOKENVALUE (0xD2).
        // Если бы опкоды не исполнялись, setNumber ревертил бы OpcodeNotFound.
        counter.setNumber(7);
        require(counter.number() == 7, "guard blocked a plain call");
    }
}
