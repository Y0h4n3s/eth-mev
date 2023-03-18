//SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

interface IUniswapV2Pair {
    event Approval(address indexed owner, address indexed spender, uint value);
    event Transfer(address indexed from, address indexed to, uint value);

    function name() external pure returns (string memory);
    function symbol() external pure returns (string memory);
    function decimals() external pure returns (uint8);
    function totalSupply() external view returns (uint);
    function balanceOf(address owner) external view returns (uint);
    function allowance(address owner, address spender) external view returns (uint);

    function approve(address spender, uint value) external returns (bool);
    function transfer(address to, uint value) external returns (bool);
    function transferFrom(address from, address to, uint value) external returns (bool);

    function DOMAIN_SEPARATOR() external view returns (bytes32);
    function PERMIT_TYPEHASH() external pure returns (bytes32);
    function nonces(address owner) external view returns (uint);

    function permit(address owner, address spender, uint value, uint deadline, uint8 v, bytes32 r, bytes32 s) external;

    event Mint(address indexed sender, uint amount0, uint amount1);
    event Burn(address indexed sender, uint amount0, uint amount1, address indexed to);
    event Swap(
        address indexed sender,
        uint amount0In,
        uint amount1In,
        uint amount0Out,
        uint amount1Out,
        address indexed to
    );
    event Sync(uint112 reserve0, uint112 reserve1);

    function MINIMUM_LIQUIDITY() external pure returns (uint);
    function factory() external view returns (address);
    function token0() external view returns (address);
    function token1() external view returns (address);
    function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
    function price0CumulativeLast() external view returns (uint);
    function price1CumulativeLast() external view returns (uint);
    function kLast() external view returns (uint);

    function mint(address to) external returns (uint liquidity);
    function burn(address to) external returns (uint amount0, uint amount1);
    function swap(uint amount0Out, uint amount1Out, address to, bytes calldata data) external;
    function skim(address to) external;
    function sync() external;

    function initialize(address, address) external;
}
interface IERC20 {
    function decimals() external view returns (uint8);
    function balanceOf(address) external returns(uint256);
}
/**
@dev This contract is not meant to be deployed. Instead, use a static call with the
             deployment bytecode as payload.
        */
contract UniswapV2DataAggregator {

    struct PoolData {
        address addr;
        uint256 reserve0;
        uint256 reserve1;
        uint256 balance0;
        uint256 balance1;
        uint256 blockNumber;
        address tokenA;
        uint8 tokenADecimals;
        address tokenB;
        uint8 tokenBDecimals;
        uint256 fees;
    }

    constructor(address[] memory pools) {
        PoolData[] memory allPoolData = new PoolData[](pools.length);

        for (uint256 i = 0; i < pools.length; ++i) {
            address poolAddress = pools[i];

            if (codeSizeIsZero(poolAddress)) continue;

            PoolData memory poolData;
            (poolData.reserve0, poolData.reserve1, ) = IUniswapV2Pair(poolAddress).getReserves();
            poolData.tokenA = IUniswapV2Pair(poolAddress).token0();
            poolData.addr = poolAddress;
            poolData.tokenB = IUniswapV2Pair(poolAddress).token1();
            if (codeSizeIsZero(poolData.tokenA)) continue;
            if (codeSizeIsZero(poolData.tokenB)) continue;

            //Get tokenA decimals
            (
            bool tokenADecimalsSuccess,
            bytes memory tokenADecimalsData
            ) = poolData.tokenA.call(abi.encodeWithSignature("decimals()"));

            if (tokenADecimalsSuccess) {
                uint256 tokenADecimals;

                if (tokenADecimalsData.length == 32) {
                    (tokenADecimals) = abi.decode(
                        tokenADecimalsData,
                        (uint256)
                    );

                    if (tokenADecimals == 0 || tokenADecimals > 255) {
                        continue;
                    } else {
                        poolData.tokenADecimals = uint8(tokenADecimals);
                    }
                } else {
                    continue;
                }
            } else {
                continue;
            }

            (
            bool tokenBDecimalsSuccess,
            bytes memory tokenBDecimalsData
            ) = poolData.tokenB.call(abi.encodeWithSignature("decimals()"));

            if (tokenBDecimalsSuccess) {
                uint256 tokenBDecimals;
                if (tokenBDecimalsData.length == 32) {
                    (tokenBDecimals) = abi.decode(
                        tokenBDecimalsData,
                        (uint256)
                    );

                    if (tokenBDecimals == 0 || tokenBDecimals > 255) {
                        continue;
                    } else {
                        poolData.tokenBDecimals = uint8(tokenBDecimals);
                    }
                } else {
                    continue;
                }
            } else {
                continue;
            }

            (
            bool tokenABalanceSuccess,
            bytes memory tokenABalanceData
            ) = poolData.tokenA.call(abi.encodeWithSignature("balanceOf(address)", poolAddress));

            if (tokenABalanceSuccess) {
                uint256 tokenABalance;

                if (tokenABalanceData.length == 32) {
                    (poolData.balance0) = abi.decode(
                        tokenABalanceData,
                        (uint256)
                    );
                    if (poolData.balance0 == 0) {
                        continue;
                    }

                } else {
                    continue;
                }
            } else {
                continue;
            }
            (
            bool tokenBBalanceSuccess,
            bytes memory tokenBBalanceData
            ) = poolData.tokenB.call(abi.encodeWithSignature("balanceOf(address)", poolAddress));

            if (tokenBBalanceSuccess) {

                if (tokenBBalanceData.length == 32) {
                    (poolData.balance1) = abi.decode(
                        tokenBBalanceData,
                        (uint256)
                    );
                    if (poolData.balance1 == 0) {
                        continue;
                    }

                } else {
                    continue;
                }
            } else {
                continue;
            }
            poolData.blockNumber = block.number;
            allPoolData[i] = poolData;
        }


        bytes memory _abiEncodedData = abi.encode(allPoolData);
        assembly {
        // Return from the start of the data (discarding the original data address)
        // up to the end of the memory used
            let dataStart := add(_abiEncodedData, 0x20)
            //return (dataStart, sub(msize(), dataStart))
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