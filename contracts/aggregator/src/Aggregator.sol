pragma solidity ^0.8.13;
// extra gas spent: 113320
// uniswapv3 avg gas: 104522
// uniswapv2 avg gas: 119421
import "./interfaces/ISwapRouter.sol";
import "./interfaces/IUniswapV3Pool.sol";
import "./interfaces/IUniswapV2Pair.sol";
import "./interfaces/IUniswapV2Router02.sol";
import {SafeCast} from '@uniswap3/contracts/libraries/SafeCast.sol';
import "./SafeMath.sol";

contract Aggregator {
    address private immutable owner;
    address private immutable executor;

    uint160 internal constant MIN_SQRT_RATIO = 4295128739;
    uint160 internal constant MAX_SQRT_RATIO = 1461446703485210103287273052203988822378723970342;

    // state variables
    uint128 arbPreBalance = 0;
    address arbToken = address(0);
    uint8 private globalStep = 0;
    uint8 private stepPayToIndex = 0;
    bool private stepXtoY = true;
    address private stepPool = address(0);
    bytes4 private stepNextFunction = bytes4(0);
    bytes4 private profitCheckpointFunction = 0x00000;
    uint constant private DATA_FRAME_SIZE = 27;
    uint constant private PROFIT_CHECK_POINT_DATA_FRAME_SIZE = 16 + 20;


    modifier onlyExecutor() {
        require(msg.sender == executor);
        _;
    }

    modifier onlyOwner() {
        require(msg.sender == owner);
        _;
    }

    // step add checklist
    // 1. Take in asset Pay Back debt somewhere
    modifier step(bytes calldata data) {
        (, stepPool, stepXtoY, , stepPayToIndex) = decodeDataRow(data[(globalStep) * DATA_FRAME_SIZE : (globalStep + 1) * DATA_FRAME_SIZE]);
        globalStep = globalStep + 1;
        bytes calldata remainingData = data[(globalStep) * DATA_FRAME_SIZE :];
        if (remainingData.length == PROFIT_CHECK_POINT_DATA_FRAME_SIZE) {
            stepNextFunction = profitCheckpointFunction;
            (arbPreBalance, arbToken) = decodeArbMetadata(remainingData);
            _;
        } else {
            (,,, stepNextFunction,) = decodeDataRow(data[(globalStep) * DATA_FRAME_SIZE : (globalStep + 1) * DATA_FRAME_SIZE]);
            _;
        }

    }

    constructor(address _executor) {
        owner = msg.sender;
        executor = _executor;
    }
    receive() external payable {}

    function withdrawValue(address payable _to, uint256 _value, bytes calldata _data) external onlyOwner payable returns (bytes memory) {
        require(_to != address(0));
        (bool _success, bytes memory _result) = _to.call{value : _value}(_data);
        require(_success);
        return _result;
    }

    function withdraw(address token, address to, uint256 amount) external onlyOwner {
        IERC20(token).transferFrom(address(this), to, amount);
    }

    event bal(uint);
    event bali(int);
    event by(bytes);
    event by4(bytes4);
    event acc(address);
    event accBal(address, uint, address);
    event imessage(string, int);

    function getAmountOut(uint amountIn, uint reserveIn, uint reserveOut) internal pure returns (uint amountOut) {
    unchecked {
        uint amountInWithFee = amountIn * 997;
        uint numerator = amountInWithFee * reserveOut;
        uint denominator = reserveIn * 1000 + amountInWithFee;
        return numerator / denominator;
    }
    }

    function getAmountIn(uint amountOut, uint reserveIn, uint reserveOut) internal pure returns (uint amountIn) {
    unchecked {
        uint numerator = reserveIn * amountOut * 1000;
        uint denominator = reserveOut - amountOut * 997;
        return (numerator / denominator) + 1;
    }
    }

    function sqrtRatio(bool xToy) public pure returns (uint160) {
    unchecked {
        return (xToy ? MIN_SQRT_RATIO + 1 : MAX_SQRT_RATIO - 1);

    }
    }

    function balanceOf(address token, address account) private view returns (uint) {
        return IERC20(token).balanceOf(account);
    }

    function toBytes(bytes calldata _bytes, uint256 arg) internal pure returns (bytes calldata res) {
        assembly {
            let lengthPtr := add(_bytes.offset, calldataload(add(_bytes.offset, mul(0x20, arg))))
            res.offset := add(lengthPtr, 0x20)
            res.length := calldataload(lengthPtr)
        }
    }

    function decodeDataRow(bytes calldata _bytes) internal pure returns (uint8 poolId, address pool, bool xToy, bytes4 nextFunc, uint8 payToIndex) {
        assembly {
            poolId := shr(248, calldataload(_bytes.offset))
            pool := shr(96, calldataload(add(_bytes.offset, 1)))
            xToy := shr(248, calldataload(add(_bytes.offset, 21)))
            nextFunc := calldataload(add(_bytes.offset, 22))
            payToIndex := shr(248, calldataload(add(_bytes.offset, 26)))
        }
    }

    function decodeArbMetadata(bytes calldata _bytes) internal pure returns (uint128 preBalance, address token) {
        assembly {
            preBalance :=shr(128,calldataload(_bytes.offset))
//            arbAmount := calldataload(add(_bytes.offset, 32))
            token := shr(96, calldataload(add(_bytes.offset, 16)))
//            ethDenomination := shr(0, calldataload(add(_bytes.offset, 84)))
        }
    }

    function defaultStepData() internal pure returns (uint8 poolId, address pool, bool xToy) {
        return (0, address(0), true);
    }

    function bytesToAddress(bytes calldata bys) private pure returns (address addr) {
        assembly {
            addr := mload(add(add(bys.offset, 12), 20))
        }
    }

    function addressToBytes(address a) public pure returns (bytes memory b){
        assembly {
            let m := mload(0x40)
            a := and(a, 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF)
            mstore(add(m, 20), xor(0x140000000000000000000000000000000000000000, a))
            mstore(0x40, add(m, 52))
            b := m
        }
    }

    function poolAtIndex(uint8 index, bytes calldata data) internal pure returns (address) {
        return address(bytes20(data[index * DATA_FRAME_SIZE + 1 : index * DATA_FRAME_SIZE + 21]));
    }

    //TODO: uniswapV3ExactInPayToSelf
    //TODO: uniswapV3ExactInPayToIndex
    //TODO: uniswapV2ExactInPayToSender
    //TODO: uniswapV2ExactInPayToSelf
    //TODO: uniswapV2ExactInPayToIndex
    //TODO: uniswapV2ExactOutPayToSender
    //TODO: uniswapV2ExactOutPayToSelf
    //TODO: uniswapV2ExactOutPayToIndex
    // when called from uniswapV3callback exactOutStep(payToIndex, debtTokenAmount, assetTokenAmount, data)
    // assetToken = stepXtoY ? Y : X
    // debtToken = !assetToken
    // Get exact output send it to message sender
    function uniswapV3ExactOutPayToSender(int256 amountIn, int256 amountOut, bytes calldata data) public step(data) {
        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        (int amount0, int amount1) = pair.swap(msg.sender, stepXtoY, amountOut < 0 ? amountOut : - amountOut, sqrtRatio(stepXtoY), data);
    }
    function uniswapV3ExactOutPayToSenderR(int256 amountIn, int256 amountOut, bytes calldata data) public step(data) {
        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        (int amount0, int amount1) = pair.swap(msg.sender, stepXtoY, amountIn < 0 ? amountIn : - amountIn, sqrtRatio(stepXtoY), data);
    }
    function uniswapV3ExactOutPayToSelf(int256 amountIn, int256 amountOut, bytes calldata data) public step(data) {
        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        (int amount0, int amount1) = pair.swap(address(this), stepXtoY, amountOut < 0 ? amountOut : - amountOut, sqrtRatio(stepXtoY), data);
    }
    function uniswapV3ExactOutPayToSelfR(int256 amountIn, int256 amountOut, bytes calldata data) public step(data) {
        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        (int amount0, int amount1) = pair.swap(address(this), stepXtoY, amountIn < 0 ? amountIn : - amountIn, sqrtRatio(stepXtoY), data);
    }
    function uniswapV3ExactOutPayToIndex(int256 amountIn, int256 amountOut, bytes calldata data) public step(data) {
        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        (int amount0, int amount1) = pair.swap(poolAtIndex(stepPayToIndex, data), stepXtoY, amountOut < 0 ? amountOut : - amountOut, sqrtRatio(stepXtoY), data);
    }
    function uniswapV3ExactOutPayToIndexR(int256 amountIn, int256 amountOut, bytes calldata data) public step(data) {
        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        (int amount0, int amount1) = pair.swap(poolAtIndex(stepPayToIndex, data), stepXtoY, amountIn < 0 ? amountIn : - amountIn, sqrtRatio(stepXtoY), data);

    }
    function uniswapV3ExactInPayToSender(int256 amountIn, int256 amountOut, bytes calldata data) public step(data) {
        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        (int amount0, int amount1) = pair.swap(msg.sender, stepXtoY, amountIn < 0 ? - amountIn : amountIn, sqrtRatio(stepXtoY), data);
    }
    function uniswapV3ExactInPayToSenderR(int256 amountIn, int256 amountOut, bytes calldata data) public step(data) {
        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        (int amount0, int amount1) = pair.swap(msg.sender, stepXtoY, amountOut < 0 ? - amountOut : amountOut, sqrtRatio(stepXtoY), data);
    }
    function uniswapV3ExactInPayToIndex(int256 amountIn, int256 amountOut, bytes calldata data) public step(data) {
        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        (int amount0, int amount1) = pair.swap(poolAtIndex(stepPayToIndex, data), stepXtoY, amountIn < 0 ? - amountIn : amountIn, sqrtRatio(stepXtoY), data);
    }
    function uniswapV3ExactInPayToSelf(int256 amountIn, int256 amountOut, bytes calldata data) public step(data) {
        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        (int amount0, int amount1) = pair.swap(address(this), stepXtoY, amountIn < 0 ? - amountIn : amountIn, sqrtRatio(stepXtoY), data);
    }
    function uniswapV3ExactInPayToSelfR(int256 amountIn, int256 amountOut, bytes calldata data) public step(data) {
        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        (int amount0, int amount1) = pair.swap(address(this), stepXtoY, amountOut < 0 ? - amountOut : amountOut, sqrtRatio(stepXtoY), data);
    }
    function uniswapV3ExactInPayToIndexR(int256 amountIn, int256 amountOut, bytes calldata data) public step(data) {
        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        (int amount0, int amount1) = pair.swap(poolAtIndex(stepPayToIndex, data), stepXtoY, amountOut < 0 ? - amountOut : amountOut, sqrtRatio(stepXtoY), data);
    }


    function uniswapV2ExactOutPayToSender(int256 amountIn, int256 amountOut, bytes calldata data) public step(data) {
        uint amount0Out = 0;
        uint amount1Out = amountOut;

        if (!stepXtoY) {
            amount1Out = 0;
            amount0Out = amountOut;
        }

        IUniswapV2Pair pair = IUniswapV2Pair(stepPool);
        pair.swap(amount0Out, amount1Out, msg.sender);
        if (stepNextFunction == profitCheckpointFunction) {
            profitCheckpoint();
        } else {
            function(int256, int256, bytes calldata) next = nextFunctionPointer(stepNextFunction);
            next(amountOut, amountToPay, data);
            if (stepNextFunction == profitCheckpointFunction) {
                profitCheckpoint();
            }
        }
    }



    function payUniswapV3AtIndex(int256 amountIn, int256 amountOut, bytes calldata data) public step(data) {
        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        IERC20(stepXtoY ? pair.token0() : pair.token1()).transfer(poolAtIndex(stepPayToIndex, data), amountOut < 0 ? uint256(- amountOut) : uint256(amountOut));
    }

    function payUniswapV2AtIndex(int256 amountIn, int256 amountOut, bytes calldata data) public step(data) {
        IUniswapV2Pair pair = IUniswapV2Pair(stepPool);
        IERC20(stepXtoY ? pair.token0() : pair.token1()).transfer(poolAtIndex(stepPayToIndex, data), amountIn < 0 ? uint256(- amountIn) : uint256(amountIn));
    }

    function uniswapV3SwapCallback(int256 amount0Delta, int256 amount1Delta, bytes calldata data) external {

        // amountToPay == debt, amountOut == asset
        (int256 amountToPay, int256 amountOut) =
        amount0Delta > 0 ? (amount0Delta, amount1Delta) : (amount1Delta, amount0Delta);
        emit imessage("amountToPay:", amountToPay);
        emit imessage("amountOut:", amountOut);
        emit by4(stepNextFunction);
        if (stepNextFunction == profitCheckpointFunction) {
            profitCheckpoint();
        } else {
            function(int256, int256, bytes calldata) next = nextFunctionPointer(stepNextFunction);
            next(amountOut, amountToPay, data);
            if (stepNextFunction == profitCheckpointFunction) {
                profitCheckpoint();
            }
        }

    }

    function nextFunctionPointer(bytes4 hash) internal pure returns (function(int256, int256, bytes calldata)) {
        if (hash == 0x5b16a784) {
            return uniswapV3ExactOutPayToSender;
        } else if (hash == 0x4e9a7c15) {
            return uniswapV3ExactOutPayToIndex;
        } else if (hash == 0x23d8a9db) {
            return uniswapV3ExactInPayToSender;
        }else if (hash == 0x77a56c2f) {
            return uniswapV3ExactInPayToIndex;
        }else if (hash == 0x4dfcfd17) {
            return uniswapV3ExactInPayToIndexR;
        } else if (hash == 0xc8bdd598) {
            return uniswapV3ExactOutPayToSelf;
        } else if (hash == 0x65216d26) {
            return uniswapV3ExactOutPayToSelfR;
        } else if (hash == 0x78632200) {
            return payUniswapV3AtIndex;
        } else {
            return uniswapV3ExactOutPayToSender;
        }

    }

    function profitCheckpoint() public {
        // reset for next opportunity
        globalStep = 0;
        // avoid recursive calls from uniswapV3SwapCallback
        stepNextFunction = bytes4(0x11111111);
        unchecked {
            uint256 balance = IERC20(arbToken).balanceOf(address(this));
            emit bal(balance);
            require(balance >= arbPreBalance, "NP");
        }

    }
}
