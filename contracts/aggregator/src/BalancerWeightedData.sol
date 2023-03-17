//SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

interface IVault {
    function getPoolTokens(bytes32 poolId)
    external
    view
    returns (
        IERC20[] memory tokens,
        uint256[] memory balances,
        uint256 lastChangeBlock
    );
}
interface IERC20 {
    function decimals() external view returns (uint8);
    function balanceOf(address) external returns(uint256);
}
/**
@dev This contract is not meant to be deployed. Instead, use a static call with the
             deployment bytecode as payload.
        */
contract BalancerWeightedDataAggregator {

    struct PoolData {
        bytes32 id;
        address[] tokens;
        uint256[] balances;
        uint256 blockNumber;
    }

    constructor(bytes32[] memory poolIds) {
        IVault vault = IVault(0xBA12222222228d8Ba445958a75a0704d566BF2C8);
        PoolData[] memory allPoolData = new PoolData[](poolIds.length);

        for (uint256 i = 0; i < poolIds.length; ++i) {
            bytes32 id = poolIds[i];
            PoolData memory data;
            (IERC20[] memory tokens, uint256[] memory balances,) = vault.getPoolTokens(id);
            address[] memory token = new address[](tokens.length);
            data.balances = balances;

            for (uint j = 0; j < tokens.length; j++) {
                token[j] = address(tokens[j]);
            }
            data.tokens = token;
            data.id = id;
            data.blockNumber = block.number;

            allPoolData[i] = data;
        }


        bytes memory _abiEncodedData = abi.encode(allPoolData);
        assembly {
        // Return from the start of the data (discarding the original data address)
        // up to the end of the memory used
            let dataStart := add(_abiEncodedData, 0x20)
//        return (dataStart, sub(msize(), dataStart))
        }
    }

}