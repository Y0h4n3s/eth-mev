pragma solidity ^0.8.13;

import "src/interfaces/IUniswapV3Pool.sol";
import "src/interfaces/IUniswapV2Pair.sol";
contract Aggregator {
    uint256 public number;

    uint8 constant private UNISWAPV2 = 1;
    uint8 constant private UNISWAPV3 = 2;
    function tryRoute(address[] calldata pools, uint8[] calldata poolIds, bool[] calldata directions, uint256 amountIn)
        external {

        uint256 amount = amountIn;
        for (uint8 i = 0; i < poolIds.length; i++) {
            uint8 id = poolIds[i];
            if (id == UNISWAPV2) {
                IUniswapV2Pair pair = IUniswapV2Pair(pools[i]);
                address token0 = pair.token0();
                address token1 = pair.token1();
                uint amount0Out = 0;
                uint amount1Out = 0;
                uint initialBalance = IERC20(token0).balanceOf(address(this));

                if (directions[i]) {
                    amount0Out = amount;
                    initialBalance = IERC20(token1).balanceOf(address(this));
                } else {
                    amount1Out = amount;
                }
                pair.swap(amount0Out, amount1Out, address(this), "");
                uint token2Balance = IERC20(token1).balanceOf(address(this));
                amount = token2Balance - initialBalance;
            }
            if (id == UNISWAPV3) {
                (int256 amount0, int256 amount1) = IUniswapV3Pool(pools[i]).swap(address(this), directions[i], int(amount), 0, "");
                if (directions[i]) {
                    amount = uint(amount1);
                } else {
                    amount = uint(amount0);
                }
            }
        }

        require(amount > amountIn, "Aggregator: No profit" );
    }
}
