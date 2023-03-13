//SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

interface IUniswapV3Pool {
    function token0() external view returns (address);

    function token1() external view returns (address);

    function fee() external view returns (uint24);

    function tickSpacing() external view returns (int24);

    function liquidity() external view returns (uint128);

    function slot0()
    external
    view
    returns (
        uint160 sqrtPriceX96,
        int24 tick,
        uint16 observationIndex,
        uint16 observationCardinality,
        uint16 observationCardinalityNext,
        uint8 feeProtocol,
        bool unlocked
    );

    function ticks(int24 tick)
    external
    view
    returns (
        uint128 liquidityGross,
        int128 liquidityNet,
        uint256 feeGrowthOutside0X128,
        uint256 feeGrowthOutside1X128,
        int56 tickCumulativeOutside,
        uint160 secondsPerLiquidityOutsideX128,
        uint32 secondsOutside,
        bool initialized
    );
}

interface IERC20 {
    function decimals() external view returns (uint8);
    function balanceOf(address) external returns(uint256);
}
interface IUniswapV3PoolState {
    function ticks(int24 tick)
    external
    view
    returns (
        uint128 liquidityGross,
        int128 liquidityNet,
        uint256 feeGrowthOutside0X128,
        uint256 feeGrowthOutside1X128,
        int56 tickCumulativeOutside,
        uint160 secondsPerLiquidityOutsideX128,
        uint32 secondsOutside,
        bool initialized
    );

    /// @notice Returns 256 packed tick initialized boolean values. See TickBitmap for more information
    function tickBitmap(int16 wordPosition) external view returns (uint256);
}

/**
@dev This contract is not meant to be deployed. Instead, use a static call with the
             deployment bytecode as payload.
        */
contract UniswapV3DataAggregator {
    int24 internal constant MIN_TICK = - 887272;
    int24 internal constant MAX_TICK = - MIN_TICK;

    struct PoolData {
        address addr;
        address tokenA;
        uint8 tokenADecimals;
        address tokenB;
        uint8 tokenBDecimals;
        uint128 liquidity;
        uint160 sqrtPrice;
        int24 tick;
        int24 tickSpacing;
        uint24 fee;
        int128 liquidityNet;
        uint256 tokenAAmount;
        uint256 tokenBAmount;
        TickData[] tickBitmapXY;
        TickData[] tickBitmapYX;
    }

    struct TickData {
        bool initialized;
        int24 tick;
        int128 liquidityNet;
    }
    constructor(address[] memory pools) {
        PoolData[] memory allPoolData = new PoolData[](pools.length);

        for (uint256 i = 0; i < pools.length; ++i) {
            address poolAddress = pools[i];

            if (codeSizeIsZero(poolAddress)) continue;

            PoolData memory poolData;
            poolData.addr = poolAddress;
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
            //return (dataStart, sub(msize(), dataStart))
        }
    }
    function getTickBitmap(address pool, int24 currentTick, bool zeroForOne, int24 tickSpacing) public returns (TickData[] memory tickData) {
        uint numTicks = 10;
    TickData[] memory tickData = new TickData[](numTicks);
        //Instantiate current word position to keep track of the word count
        uint256 counter = 0;

        while (counter < numTicks) {
            (
            int24 nextTick,
            bool initialized
            ) = nextInitializedTickWithinOneWord(
                pool,
                currentTick,
                tickSpacing,
                zeroForOne
            );

            //Make sure the next tick is initialized
            (, int128 liquidityNet, , , , , ,) = IUniswapV3PoolState(pool)
            .ticks(nextTick);

            //Make sure not to overshoot the max/min tick
            //If we do, break the loop, and set the last initialized tick to the max/min tick=
            if (nextTick < MIN_TICK) {
                nextTick = MIN_TICK;
                tickData[counter].initialized = initialized;
                tickData[counter].tick = nextTick;
                tickData[counter].liquidityNet = liquidityNet;
                break;
            } else if (nextTick > MAX_TICK) {
                nextTick = MIN_TICK;
                tickData[counter].initialized = initialized;
                tickData[counter].tick = nextTick;
                tickData[counter].liquidityNet = liquidityNet;
                break;
            } else {
                tickData[counter].initialized = initialized;
                tickData[counter].tick = nextTick;
                tickData[counter].liquidityNet = liquidityNet;
            }

            counter++;

            //Set the current tick to the next tick and repeat
            currentTick = zeroForOne ? nextTick - 1 : nextTick;
        }

        return tickData;
    }

    function codeSizeIsZero(address target) internal view returns (bool) {
        if (target.code.length == 0) {
            return true;
        } else {
            return false;
        }
    }

    function position(int24 tick)
    private
    pure
    returns (int16 wordPos, uint8 bitPos)
    {
    unchecked {
        wordPos = int16(tick >> 8);
        bitPos = uint8(int8(tick % 256));
    }
    }

    function nextInitializedTickWithinOneWord(
        address pool,
        int24 tick,
        int24 tickSpacing,
        bool lte
    ) internal view returns (int24 next, bool initialized) {
    unchecked {
        int24 compressed = tick / tickSpacing;
        if (tick < 0 && tick % tickSpacing != 0) compressed--;
        // round towards negative infinity

        if (lte) {
            (int16 wordPos, uint8 bitPos) = position(compressed);

            // all the 1s at or to the right of the current bitPos
            uint256 mask = (1 << bitPos) - 1 + (1 << bitPos);
            uint256 masked = IUniswapV3PoolState(pool).tickBitmap(wordPos) &
            mask;

            // if there are no initialized ticks to the right of or at the current tick, return rightmost in the word
            initialized = masked != 0;
            // overflow/underflow is possible, but prevented externally by limiting both tickSpacing and tick
            next = initialized
            ? (compressed -
            int24(
                uint24(bitPos - BitMath.mostSignificantBit(masked))
            )) * tickSpacing
            : (compressed - int24(uint24(bitPos))) * tickSpacing;
        } else {
            // start from the word of the next tick, since the current tick state doesn't matter
            (int16 wordPos, uint8 bitPos) = position(compressed + 1);
            // all the 1s at or to the left of the bitPos
            uint256 mask = ~((1 << bitPos) - 1);
            uint256 masked = IUniswapV3PoolState(pool).tickBitmap(wordPos) &
            mask;

            // if there are no initialized ticks to the left of the current tick, return leftmost in the word
            initialized = masked != 0;
            // overflow/underflow is possible, but prevented externally by limiting both tickSpacing and tick
            next = initialized
            ? (compressed +
            1 +
            int24(
                uint24(BitMath.leastSignificantBit(masked) - bitPos)
            )) * tickSpacing
            : (compressed +
            1 +
            int24(uint24(type(uint8).max - bitPos))) * tickSpacing;
        }
    }
    }
}


/// @title BitMath
/// @dev This library provides functionality for computing bit properties of an unsigned integer
library BitMath {
    /// @notice Returns the index of the most significant bit of the number,
    ///     where the least significant bit is at index 0 and the most significant bit is at index 255
    /// @dev The function satisfies the property:
    ///     x >= 2**mostSignificantBit(x) and x < 2**(mostSignificantBit(x)+1)
    /// @param x the value for which to compute the most significant bit, must be greater than 0
    /// @return r the index of the most significant bit
    function mostSignificantBit(uint256 x) internal pure returns (uint8 r) {
        require(x > 0);

    unchecked {
        if (x >= 0x100000000000000000000000000000000) {
            x >>= 128;
            r += 128;
        }
        if (x >= 0x10000000000000000) {
            x >>= 64;
            r += 64;
        }
        if (x >= 0x100000000) {
            x >>= 32;
            r += 32;
        }
        if (x >= 0x10000) {
            x >>= 16;
            r += 16;
        }
        if (x >= 0x100) {
            x >>= 8;
            r += 8;
        }
        if (x >= 0x10) {
            x >>= 4;
            r += 4;
        }
        if (x >= 0x4) {
            x >>= 2;
            r += 2;
        }
        if (x >= 0x2) r += 1;
    }
    }


    function leastSignificantBit(uint256 x) internal pure returns (uint8 r) {
        require(x > 0);

    unchecked {
        r = 255;
        if (x & type(uint128).max > 0) {
            r -= 128;
        } else {
            x >>= 128;
        }
        if (x & type(uint64).max > 0) {
            r -= 64;
        } else {
            x >>= 64;
        }
        if (x & type(uint32).max > 0) {
            r -= 32;
        } else {
            x >>= 32;
        }
        if (x & type(uint16).max > 0) {
            r -= 16;
        } else {
            x >>= 16;
        }
        if (x & type(uint8).max > 0) {
            r -= 8;
        } else {
            x >>= 8;
        }
        if (x & 0xf > 0) {
            r -= 4;
        } else {
            x >>= 4;
        }
    }
    }
}