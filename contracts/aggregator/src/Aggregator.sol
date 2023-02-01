pragma solidity ^0.8.13;
// extra gas spent: 113320
// uniswapv3 avg gas: 104522
// uniswapv2 avg gas: 119421
import "./interfaces/ISwapRouter.sol";
import "./interfaces/IUniswapV3Pool.sol";
import "./interfaces/IUniswapV2Pair.sol";
import "./interfaces/IUniswapV2Router02.sol";
contract Aggregator {
    uint256 public number;
    address constant private UNISWAPV3ROUTER = address(0xE592427A0AEce92De3Edee1F18E0157C05861564);
    address constant private UNISWAPV2ROUTER = address(0x7a250d5630B4cF539739dF2C5dAcb4c659F2488D);

    uint8 constant private UNISWAPV2 = 1;
    uint8 constant private UNISWAPV3 = 2;

    function tryRoute(address[] calldata pools, uint8[] calldata poolIds, bool[] calldata directions, uint256 amountIn)
        external {

        uint256 amount = amountIn;
        for (uint8 i = 0; i < poolIds.length; i++) {
            uint8 id = poolIds[i];
            if (id == UNISWAPV2) {
                IUniswapV2Pair pair = IUniswapV2Pair(pools[i]);
                address tokenIn = pair.token0();
                address tokenOut = pair.token1();

                if (!directions[i]) {
                    tokenIn = pair.token1();
                    tokenOut = pair.token0();
                }
                IERC20(tokenIn).approve(address(UNISWAPV2ROUTER), amount);
                uint initialBalance = IERC20(tokenOut).balanceOf(address(this));
                address[] memory path = new address[](2);
                path[0] = tokenIn;
                path[1] = tokenOut;
                IUniswapV2Router02(UNISWAPV2ROUTER).swapExactTokensForTokensSupportingFeeOnTransferTokens(amount,0, path, msg.sender, 9999999999999999999);
                uint token2Balance = IERC20(tokenOut).balanceOf(address(this));
                amount = token2Balance - initialBalance;
            }
            if (id == UNISWAPV3) {

                IUniswapV3Pool pool = IUniswapV3Pool(pools[i]);
                address tokenIn = pool.token0();
                address tokenOut = pool.token1();
                address factory = pool.factory();
                if (!directions[i]) {
                    tokenOut = pool.token0();
                    tokenIn = pool.token1();
                }
                IERC20(tokenIn).approve(address(UNISWAPV3ROUTER), amount);

                ISwapRouter.ExactInputSingleParams memory params = ISwapRouter.ExactInputSingleParams({
                    tokenIn: tokenIn,
                    tokenOut: tokenOut,
                    fee: pool.fee(),
                    recipient: msg.sender,
                    deadline: 9999999999999999,
                    amountIn: amount,
                    amountOutMinimum: 0,
                    sqrtPriceLimitX96: 0
                });
                amount = ISwapRouter(UNISWAPV3ROUTER).exactInputSingle(params);
            }
            require(amount > 0, "AL0");
        }

        require(amount > amountIn, "NP" );
    }
}
