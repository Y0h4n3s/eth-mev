//SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

interface CurvePlainFactory {
    function pool_list(uint256 arg0) external returns (address);
}

interface CurvePlainPool {
    function A() external returns (uint256);
    function gamma() external returns (uint256);
    function fee() external returns (uint256);
    function fee_gamma() external returns (uint256);
    function price_scale() external returns (uint256);
    function coins(uint256 arg0) external returns (address);
    function balances(uint256 arg0) external returns (uint256);

}
interface IERC20 {
    function decimals() external view returns (uint8);
    function balanceOf(address) external returns(uint256);
}
/**
@dev This contract is not meant to be deployed. Instead, use a static call with the
             deployment bytecode as payload.
        */
contract CurvePlainDataAggregator {

    struct PoolData {
        address addr;
        address[] tokens;
        uint256[] balances;
        uint256[] decimals;
        uint256 amp;
        uint256 fee;
        uint256 blockNumber;
    }

    constructor(address[] memory pools) {
        PoolData[] memory allPoolData = new PoolData[](pools.length);


        for (uint256 i = 0; i < pools.length; ++i) {

            address poolAddress = pools[i];
            if (codeSizeIsZero(poolAddress)) continue;

            address[] memory tokens = new address[](8);
            uint256[] memory balances = new uint256[](8);
            uint256[] memory decimals = new uint256[](8);
            for (uint i = 0; i < 8; i++) {
                (bool tokenCallSuccess, bytes memory tokenCallData) = poolAddress.call(abi.encodeWithSignature("coins(uint256)",i));
                if (tokenCallSuccess) {
                    address token = abi.decode(tokenCallData, (address));
                    if (codeSizeIsZero(token)) continue;
                    tokens[i] = token;
                    decimals[i] = IERC20(token).decimals();
                    balances[i] = CurvePlainPool(poolAddress).balances(i);
                    (bool balanceCallSuccess, bytes memory balanceCallData) = poolAddress.call(abi.encodeWithSignature("balances(uint256)",i));
                    if (balanceCallSuccess) {
                        balances[i] = abi.decode(balanceCallData, (uint256));

                    } else {
                        continue;
                    }

                } else {
                    continue;
                }
            }
            if (tokens[0] == address(0)) continue;

            PoolData memory poolData;

            poolData.tokens = tokens;
            poolData.balances = balances;
            poolData.decimals = decimals;
            (bool feeCallSuccess, bytes memory feeCallData) = poolAddress.call(abi.encodeWithSignature("fee()"));
            if (feeCallSuccess) {
                poolData.fee = abi.decode(feeCallData, (uint256));

            } else {
                continue;
            }

            (bool ampCallSuccess, bytes memory ampCallData) = poolAddress.call(abi.encodeWithSignature("A()"));
            if (ampCallSuccess) {
                poolData.amp = abi.decode(ampCallData, (uint256));

            } else {
                continue;
            }

            poolData.addr = poolAddress;


            poolData.blockNumber = block.number;
            allPoolData[i] = poolData;
        }


        bytes memory _abiEncodedData = abi.encode(allPoolData);
        assembly {
        // Return from the start of the data (discarding the original data address)
        // up to the end of the memory used
            let dataStart := add(_abiEncodedData, 0x20)
        return (dataStart, sub(msize(), dataStart))
        }
    }

    function codeSizeIsZero(address target) internal view returns (bool) {
        if (target.code.length == 0) {
            return true;
        } else {
            return false;
        }
    }
}

contract CurvePlainPairsBatch {
    constructor(address factory, uint256 start) {
        address[] memory pools = new address[](700);

        for (uint i = start; i < start + 700; i++) {
            address pool = CurvePlainFactory(factory).pool_list(i);
            if (pool == address(0)) {
                continue;
            }
            pools[i] = pool;
        }
        bytes memory _abiEncodedData = abi.encode(pools);

        assembly {
        // Return from the start of the data (discarding the original data address)
        // up to the end of the memory used
            let dataStart := add(_abiEncodedData, 0x20)
        return (dataStart, sub(msize(), dataStart))
        }
    }
}
