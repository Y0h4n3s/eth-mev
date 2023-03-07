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

/**
@dev This contract is not meant to be deployed. Instead, use a static call with the
             deployment bytecode as payload.
        */
contract UniswapV2DataAggregator {

    struct PoolData {
        uint256 reserve0;
        uint256 reserve1;
    }

    constructor(address[] memory pools) {
        PoolData[] memory allPoolData = new PoolData[](pools.length);

        for (uint256 i = 0; i < pools.length; ++i) {
            address poolAddress = pools[i];

            if (codeSizeIsZero(poolAddress)) continue;

            PoolData memory poolData;
            (poolData.reserve0, poolData.reserve1, ) = IUniswapV2Pair(poolAddress).getReserves()

            poolData.tokenA = IUniswapV3Pool(poolAddress).token0();
            poolData.tokenB = IUniswapV3Pool(poolAddress).token1();

            //Check that tokenA and tokenB do not have codesize of 0
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

            (uint160 sqrtPriceX96, int24 tick, , , , ,) = IUniswapV3Pool(
                poolAddress
            ).slot0();

            (, int128 liquidityNet, , , , , ,) = IUniswapV3Pool(poolAddress)
            .ticks(tick);

            poolData.liquidity = IUniswapV3Pool(poolAddress).liquidity();
            poolData.tickSpacing = IUniswapV3Pool(poolAddress).tickSpacing();
            poolData.fee = IUniswapV3Pool(poolAddress).fee();

            poolData.sqrtPrice = sqrtPriceX96;
            poolData.tick = tick;

            poolData.liquidityNet = liquidityNet;
            poolData.tickBitmapXY = getTickBitmap(poolAddress, tick, true, poolData.tickSpacing);
            poolData.tickBitmapYX = getTickBitmap(poolAddress, tick, false, poolData.tickSpacing);
            poolData.tokenAAmount = IERC20(poolData.tokenA).balanceOf(poolAddress);
            poolData.tokenBAmount = IERC20(poolData.tokenB).balanceOf(poolAddress);
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