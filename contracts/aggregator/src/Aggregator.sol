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
            preBalance := shr(128, calldataload(_bytes.offset))
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

    function getMeta(bytes calldata data) internal returns (uint8 dataLen, bytes4 nextFunction) {
        uint8 dataLen;
        assembly {
            dataLen := shr(248, calldataload(data.offset))
        }
        bytes4 nextFunction;
        assembly {
            nextFunction := calldataload(add(data.offset, add(div(dataLen, 2), 2)))
        }
            return (dataLen/2 + 1, nextFunction);
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

    //0000004b
    function uniswapV3ExactOutPayToSender_A729BB(bytes calldata data) public  {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);
        bytes calldata myData = data[1 : dataLen];

        address pool;
        int amount;
        assembly {
            pool := shr(96, calldataload(add(myData.offset, 4)))
            amount := shr(sub(256, mul(8, sub(dataLen, 24))), calldataload(add(myData.offset, 24)))
        }
        emit acc(pool);
        emit bali(amount);

        IUniswapV3Pool pair = IUniswapV3Pool(pool);
        (int amount0, int amount1) = pair.swap(msg.sender, stepXtoY, amount < 0 ? amount : - amount, sqrtRatio(stepXtoY), data);

        if (nextFunction == 0x00000000) {
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen:]);
                }
    //000000d0
    function uniswapV3ExactInPayToSender_1993B5C(bytes calldata data) public  {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);
        bytes calldata myData = data[1 : dataLen];

        address pool;
        int amount;
        assembly {
            pool := shr(96, calldataload(add(myData.offset, 4)))
            amount := shr(sub(256, mul(8, sub(dataLen, 24))), calldataload(add(myData.offset, 24)))
        }
        emit acc(pool);
        emit bali(amount);

        IUniswapV3Pool pair = IUniswapV3Pool(pool);
        (int amount0, int amount1) = pair.swap(msg.sender, stepXtoY, amount < 0 ? -amount : amount, sqrtRatio(stepXtoY), data);


        if (nextFunction == 0x00000000) {
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen:]);
        //        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        //        (int amount0, int amount1) = pair.swap(msg.sender, stepXtoY, amountIn < 0 ? - amountIn : amountIn, sqrtRatio(stepXtoY), data);
    }

    //000000fc
    function uniswapV3ExactInPayToSelf_A9C0BD(bytes calldata data) public {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);
        bytes calldata myData = data[1 : dataLen];
        if (nextFunction == 0x00000000) {
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen:]);
        //        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        //        (int amount0, int amount1) = pair.swap(address(this), stepXtoY, amountIn < 0 ? - amountIn : amountIn, sqrtRatio(stepXtoY), data);
    }
    //000000e1
    function uniswapV3ExactOutPayToSelf_1377F03(bytes calldata data) public {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);
        bytes calldata myData = data[1 : dataLen];

        if (nextFunction == 0x00000000) {
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen:]);
        //        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        //        (int amount0, int amount1) = pair.swap(address(this), stepXtoY, amountOut < 0 ? amountOut : - amountOut, sqrtRatio(stepXtoY), data);
    }

    //000000c9
    function uniswapV3ExactOutPayToAddress_37EB331(bytes calldata data) public  {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];
        if (nextFunction == 0x00000000) {
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen:]);
        //        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        //        (int amount0, int amount1) = pair.swap(poolAtIndex(stepPayToIndex, data), stepXtoY, amountOut < 0 ? amountOut : - amountOut, sqrtRatio(stepXtoY), data);
    }
    //00000091
    function uniswapV3ExactInPayToAddress_8F71A6(bytes calldata data) public  {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];
        if (nextFunction == 0x00000000) {
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen:]);
        //        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        //        (int amount0, int amount1) = pair.swap(poolAtIndex(stepPayToIndex, data), stepXtoY, amountIn < 0 ? - amountIn : amountIn, sqrtRatio(stepXtoY), data);
    }

    //00000015
    function uniswapV2ExactOutPayToSender_31D5F3(bytes calldata data) public {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];

        if (nextFunction == 0x00000000) {
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen:]);
        //        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        //        (int amount0, int amount1) = pair.swap(msg.sender, stepXtoY, amountOut < 0 ? amountOut : - amountOut, sqrtRatio(stepXtoY), data);
    }
    //000000cd
    function uniswapV2ExactInPayToSender_120576(bytes calldata data) public  {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];

        if (nextFunction == 0x00000000) {
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen:]);
        //        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        //        (int amount0, int amount1) = pair.swap(msg.sender, stepXtoY, amountIn < 0 ? - amountIn : amountIn, sqrtRatio(stepXtoY), data);
    }

    //0000003c
    function uniswapV2ExactOutPayToSelf_12BAA3(bytes calldata data) public  {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];
        if (nextFunction == 0x00000000) {
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen:]);
        //        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        //        (int amount0, int amount1) = pair.swap(address(this), stepXtoY, amountOut < 0 ? amountOut : - amountOut, sqrtRatio(stepXtoY), data);
    }
    //00000082
    function uniswapV2ExactInPayToSelf_FDC770(bytes calldata data) public  {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];
        if (nextFunction == 0x00000000) {
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen:]);
        //        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        //        (int amount0, int amount1) = pair.swap(address(this), stepXtoY, amountIn < 0 ? - amountIn : amountIn, sqrtRatio(stepXtoY), data);
    }

    //000000e5
    function uniswapV2ExactOutPayToAddress_E0E335(bytes calldata data) public  {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];
        if (nextFunction == 0x00000000) {
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(stepNextFunction);
        next(data[dataLen:]);
        //        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        //        (int amount0, int amount1) = pair.swap(poolAtIndex(stepPayToIndex, data), stepXtoY, amountOut < 0 ? amountOut : - amountOut, sqrtRatio(stepXtoY), data);
    }
    //00000059
    function uniswapV2ExactInPayToAddress_35CB03(bytes calldata data) public  {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];
        if (nextFunction == 0x00000000) {
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen:]);
        //IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        //        (int amount0, int amount1) = pair.swap(poolAtIndex(stepPayToIndex, data), stepXtoY, amountIn < 0 ? - amountIn : amountIn, sqrtRatio(stepXtoY), data);
    }

    //000000ea
    function paySender_7437EA(bytes calldata data) public {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];
        if (nextFunction == 0x00000000) {
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen:]);
    }
    //00000081
    function payAddress_1A718EA(bytes calldata data) public {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);
        bytes calldata myData = data[1 : dataLen];

        if (nextFunction == 0x00000000) {
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen:]);

    }


    function payUniswapV3AtIndex(int256 amountIn, int256 amountOut, bytes calldata data) public  {
        IUniswapV3Pool pair = IUniswapV3Pool(stepPool);
        IERC20(stepXtoY ? pair.token0() : pair.token1()).transfer(poolAtIndex(stepPayToIndex, data), amountOut < 0 ? uint256(- amountOut) : uint256(amountOut));
    }

    function payUniswapV2AtIndex(int256 amountIn, int256 amountOut, bytes calldata data) public  {
        IUniswapV2Pair pair = IUniswapV2Pair(stepPool);
        IERC20(stepXtoY ? pair.token0() : pair.token1()).transfer(poolAtIndex(stepPayToIndex, data), amountIn < 0 ? uint256(- amountIn) : uint256(amountIn));
    }

    function uniswapV3SwapCallback(int256 amount0Delta, int256 amount1Delta, bytes calldata data) external {

        // amountToPay == debt, amountOut == asset
        (int256 amountToPay, int256 amountOut) =
        amount0Delta > 0 ? (amount0Delta, amount1Delta) : (amount1Delta, amount0Delta);
        emit imessage("amountToPay:", amountToPay);
        emit imessage("amountOut:", amountOut);
        bytes4 nextFunction;
        assembly {
            nextFunction := calldataload(data.offset)
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data);

    }

    function nextFunctionPointer(bytes4 hash) internal pure returns (function(bytes calldata)) {
        if (hash == 0x000000e1) {
            return uniswapV3ExactOutPayToSelf_1377F03;
        } else if (hash == 0x000000fc) {
            return uniswapV3ExactInPayToSelf_A9C0BD;
        } else if (hash == 0x0000004b) {
            return uniswapV3ExactOutPayToSender_A729BB;
        } else if (hash == 0x000000d0) {
            return uniswapV3ExactInPayToSender_1993B5C;
        } else if (hash == 0x000000c9) {
            return uniswapV3ExactOutPayToAddress_37EB331;
        } else if (hash == 0x00000091) {
            return uniswapV3ExactInPayToAddress_8F71A6;
        }

        else if (hash == 0x00000015) {
            return uniswapV2ExactOutPayToSender_31D5F3;
        }else if (hash == 0x000000cd) {
            return uniswapV2ExactInPayToSender_120576;
        } else if (hash == 0x0000003c) {
            return uniswapV2ExactOutPayToSelf_12BAA3;
        } else if (hash == 0x00000082) {
            return uniswapV2ExactInPayToSelf_FDC770;
        } else if (hash == 0x000000e5) {
            return uniswapV2ExactOutPayToAddress_E0E335;
        } else if (hash == 0x00000059) {
            return uniswapV2ExactInPayToAddress_35CB03;
        }

        else if (hash == 0x000000ea) {
            return paySender_7437EA;
        } else if (hash == 0x00000081) {
            return payAddress_1A718EA;
        } else {
            return uniswapV3ExactOutPayToSelf_1377F03;
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
