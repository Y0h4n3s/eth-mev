pragma solidity ^0.8.13;

import "interfaces/IUniswapV3Pool.sol";
import "interfaces/IUniswapV2Pair.sol";
import "interfaces/IERC20.sol";
contract Aggregator {
    uint256 public number;

    uint8 constant private UNISWAPV2 = 0;
    uint8 constant private UNISWAPV3 = 1;
    function tryRoute(address[] pools, uint8[] poolIds, bool[] directions, uint256 amountIn)
        external
        view
        returns (uint256 amountOut) {

        uint256 amount = amountIn;
        for (uint8 i = 0; i < poolIds.len; i++) {
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
                pair.swap(,_token1, _token2,_amount);
                uint token2Balance = IERC20(_token2).balanceOf(address(this));
                amount = token2Balance - token2InitialBalance;
            }
            if (id == UNISWAPV3) {
                (amount0, amount1) = IUniswapV3Pool(pools[i]).swap(address(this), directions[i], amount, 0, "");
                if (directions[i]) {
                    amount = amount1;
                } else {
                    amount = amount0;
                }
            }
        }
    }
