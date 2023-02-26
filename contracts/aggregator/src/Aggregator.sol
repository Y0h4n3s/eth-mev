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
    using SafeCast for uint;
    address private immutable owner;

    uint160 internal constant MIN_SQRT_RATIO = 4295128739;
    uint160 internal constant MAX_SQRT_RATIO = 1461446703485210103287273052203988822378723970342;

    bool done = false;



    modifier onlyOwner() {
        require(msg.sender == owner);
        _;
    }

    modifier beforeEachStep() {
        done = false;
        _;
    }


    constructor() {
        owner = msg.sender;
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
    event by3(bytes32);
    event by4(bytes4);
    event acc(address);
    event accBal(address, uint, address);
    event imessage(string, int);
    event message(string, uint);

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

    function sqrtRatio(bool xToy) internal pure returns (uint160) {
    unchecked {
        return (xToy ? MIN_SQRT_RATIO + 1 : MAX_SQRT_RATIO - 1);

    }
    }


    function getMeta(bytes calldata data) internal pure returns (uint8 dataLen, bytes4 nextFunction) {
        uint8 dataLen;
        assembly {
            dataLen := shr(248, calldataload(data.offset))
        }
        bytes4 nextFunction;
        assembly {
            nextFunction := calldataload(add(data.offset, add(div(dataLen, 2), 2)))
        }
        return (dataLen / 2 + 1, nextFunction);
    }


    function toByte32Uint(bytes calldata data) internal pure returns (uint) {
        uint byteLen = data.length;
        bytes memory data32 = "";
        for (uint8 i = 0; i < 32 - byteLen; i++) {
            data32 = bytes.concat(data32, bytes1(0));
        }
        data32 = bytes.concat(data32, data);
        return uint256(bytes32(data32));

    }

    //0000004b
    function uniswapV3ExactOutPayToSender_A729BB(bytes calldata data) public beforeEachStep {

        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);
        bytes calldata myData = data[1 : dataLen];
        emit by(myData);

        bool isXToY;
        address pool;
        int amount = toByte32Uint(myData[25 :]).toInt256();
        assembly {
            isXToY := shr(248, calldataload(add(myData.offset, 4)))
            pool := shr(96, calldataload(add(myData.offset, 5)))
        }
        IUniswapV3Pool pair = IUniswapV3Pool(pool);
        (int amount0, int amount1) = pair.swap(msg.sender, isXToY, amount < 0 ? amount : - amount, sqrtRatio(isXToY), data[dataLen :]);

        if (done) {
            return;
        }
        if (nextFunction == 0x00000000) {
            done = true;
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen :]);
    }
    //000000d0
    function uniswapV3ExactInPayToSender_1993B5C(bytes calldata data) public beforeEachStep {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);
        bytes calldata myData = data[1 : dataLen];

        bool isXToY;
        address pool;
        int amount = toByte32Uint(myData[25 :]).toInt256();
        assembly {
            isXToY := shr(248, calldataload(add(myData.offset, 4)))
            pool := shr(96, calldataload(add(myData.offset, 5)))
        }

        IUniswapV3Pool pair = IUniswapV3Pool(pool);
        (int amount0, int amount1) = pair.swap(msg.sender, isXToY, amount < 0 ? - amount : amount, sqrtRatio(isXToY), data[dataLen :]);

        if (done) {
            return;
        }
        if (nextFunction == 0x00000000) {
            done = true;
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen :]);
    }

    //000000fc
    function uniswapV3ExactInPayToSelf_A9C0BD(bytes calldata data) public beforeEachStep {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);
        bytes calldata myData = data[1 : dataLen];
        bool isXToY;

        address pool;
        int amount = toByte32Uint(myData[25 :]).toInt256();
        assembly {
            isXToY := shr(248, calldataload(add(myData.offset, 4)))
            pool := shr(96, calldataload(add(myData.offset, 5)))
        }

        IUniswapV3Pool pair = IUniswapV3Pool(pool);
        (int amount0, int amount1) = pair.swap(address(this), isXToY, amount < 0 ? - amount : amount, sqrtRatio(isXToY), data[dataLen :]);

        if (done) {
            return;
        }
        if (nextFunction == 0x00000000) {
            done = true;
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen :]);
    }
    //000000e1
    function uniswapV3ExactOutPayToSelf_1377F03(bytes calldata data) public beforeEachStep {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);
        bytes calldata myData = data[1 : dataLen];
        bool isXToY;

        address pool;
        int amount = toByte32Uint(myData[25 :]).toInt256();
        assembly {
            isXToY := shr(248, calldataload(add(myData.offset, 4)))
            pool := shr(96, calldataload(add(myData.offset, 5)))
        }
        IUniswapV3Pool pair = IUniswapV3Pool(pool);
        (int amount0, int amount1) = pair.swap(address(this), isXToY, amount < 0 ? amount : - amount, sqrtRatio(isXToY), data[dataLen :]);

        if (done) {
            return;
        }
        if (nextFunction == 0x00000000) {
            done = true;
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen :]);
    }

    //000000c9
    function uniswapV3ExactOutPayToAddress_37EB331(bytes calldata data) public beforeEachStep {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];
        bool isXToY;

        address pool;
        address receiver;
        int amount = toByte32Uint(myData[45 :]).toInt256();
        assembly {
            isXToY := shr(248, calldataload(add(myData.offset, 4)))
            pool := shr(96, calldataload(add(myData.offset, 5)))
            receiver := shr(96, calldataload(add(myData.offset, 25)))
        }

        IUniswapV3Pool pair = IUniswapV3Pool(pool);
        (int amount0, int amount1) = pair.swap(receiver, isXToY, amount < 0 ? amount : - amount, sqrtRatio(isXToY), data[dataLen :]);
        if (done) {
            return;
        }
        if (nextFunction == 0x00000000) {
            done = true;
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen :]);
    }
    //00000091
    function uniswapV3ExactInPayToAddress_8F71A6(bytes calldata data) public beforeEachStep {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];
        bool isXToY;
        address pool;
        address receiver;
        int amount = toByte32Uint(myData[45 :]).toInt256();
        assembly {
            isXToY := shr(248, calldataload(add(myData.offset, 4)))
            pool := shr(96, calldataload(add(myData.offset, 5)))
            receiver := shr(96, calldataload(add(myData.offset, 25)))
        }
        IUniswapV3Pool pair = IUniswapV3Pool(pool);
        (int amount0, int amount1) = pair.swap(receiver, isXToY, amount < 0 ? - amount : amount, sqrtRatio(isXToY), data[dataLen :]);
        if (done) {
            return;
        }
        if (nextFunction == 0x00000000) {
            done = true;
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen :]);
    }

    
    //00000015
    function uniswapV2ExactOutPayToSender_31D5F3(bytes calldata data) public beforeEachStep {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];

        bool isXToY;
        address pool;
        uint8 assetDataLen;
        assembly {
            isXToY := shr(248, calldataload(add(myData.offset, 4)))
            pool := shr(96, calldataload(add(myData.offset, 5)))
            assetDataLen := shr(248, calldataload(add(myData.offset, 25)))
        }
        assetDataLen = assetDataLen / 2;

        uint amountAsset = toByte32Uint(myData[26:26+assetDataLen]);
        uint amountDebt = toByte32Uint(myData[26+assetDataLen:]);
        IUniswapV2Pair pair = IUniswapV2Pair(pool);
        emit message("Asset:", amountAsset);
        emit message("Debt:", amountDebt);
        uint amount0Out = amountAsset;
        uint amount1Out = 0;
        if (isXToY) {
            amount0Out = 0;
            amount1Out = amountAsset;
            IERC20(pair.token0()).transfer(pool, amountDebt);
        } else {
            IERC20(pair.token1()).transfer(pool, amountDebt);
        }
        pair.swap(amount0Out, amount1Out, msg.sender, "");
        if (nextFunction == 0x00000000) {
            done = true;
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen :]);
    }
    //000000cd
    function uniswapV2ExactInPayToSender_120576(bytes calldata data) public beforeEachStep {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];
        bool isXToY;
        address pool;
        uint8 assetDataLen;
        assembly {
            isXToY := shr(248, calldataload(add(myData.offset, 4)))
            pool := shr(96, calldataload(add(myData.offset, 5)))
            assetDataLen := shr(248, calldataload(add(myData.offset, 25)))
        }
        assetDataLen = assetDataLen / 2;
        uint amountAsset = toByte32Uint(myData[26:26+assetDataLen]);
        uint amountDebt = toByte32Uint(myData[26+assetDataLen:]);
        IUniswapV2Pair pair = IUniswapV2Pair(pool);
        emit message("Asset:", amountAsset);
        emit message("Debt:", amountDebt);
        uint amount0Out = amountAsset;
        uint amount1Out = 0;
        if (isXToY) {
            amount0Out = 0;
            amount1Out = amountAsset;
            IERC20(pair.token0()).transfer(pool, amountDebt);
        } else {
            IERC20(pair.token1()).transfer(pool, amountDebt);
        }
        pair.swap(amount0Out, amount1Out, msg.sender, "");
        if (nextFunction == 0x00000000) {
            done = true;
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen :]);
    }

    //0000003c
    function uniswapV2ExactOutPayToSelf_12BAA3(bytes calldata data) public beforeEachStep {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];

        bool isXToY;
        address pool;
        uint8 assetDataLen;
        assembly {
            isXToY := shr(248, calldataload(add(myData.offset, 4)))
            pool := shr(96, calldataload(add(myData.offset, 5)))
            assetDataLen := shr(248, calldataload(add(myData.offset, 25)))
        }
        assetDataLen = assetDataLen / 2;

        uint amountAsset = toByte32Uint(myData[26:26+assetDataLen]);
        uint amountDebt = toByte32Uint(myData[26+assetDataLen:]);
        IUniswapV2Pair pair = IUniswapV2Pair(pool);
        emit message("Asset:", amountAsset);
        emit message("Debt:", amountDebt);
        uint amount0Out = amountAsset;
        uint amount1Out = 0;
        if (isXToY) {
            amount0Out = 0;
            amount1Out = amountAsset;
            IERC20(pair.token0()).transfer(pool, amountDebt);
        } else {
            IERC20(pair.token1()).transfer(pool, amountDebt);
        }
        pair.swap(amount0Out, amount1Out, address(this), "");
        if (nextFunction == 0x00000000) {
            done = true;
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen :]);
    }
    //00000082
    function uniswapV2ExactInPayToSelf_FDC770(bytes calldata data) public beforeEachStep {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];
        bool isXToY;
        address pool;
        uint8 assetDataLen;
        assembly {
            isXToY := shr(248, calldataload(add(myData.offset, 4)))
            pool := shr(96, calldataload(add(myData.offset, 5)))
            assetDataLen := shr(248, calldataload(add(myData.offset, 25)))
        }
        assetDataLen = assetDataLen / 2;
        uint amountAsset = toByte32Uint(myData[26:26+assetDataLen]);
        uint amountDebt = toByte32Uint(myData[26+assetDataLen:]);
        IUniswapV2Pair pair = IUniswapV2Pair(pool);
        emit message("Asset:", amountAsset);
        emit message("Debt:", amountDebt);
        uint amount0Out = amountAsset;
        uint amount1Out = 0;
        if (isXToY) {
            amount0Out = 0;
            amount1Out = amountAsset;
            IERC20(pair.token0()).transfer(pool, amountDebt);
        } else {
            IERC20(pair.token1()).transfer(pool, amountDebt);
        }
        pair.swap(amount0Out, amount1Out, address(this), "");
        if (nextFunction == 0x00000000) {
            done = true;
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen :]);
    }

    //000000e5
    function uniswapV2ExactOutPayToAddress_E0E335(bytes calldata data) public beforeEachStep {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];
        bool isXToY;
        address pool;
        uint amount = toByte32Uint(myData[25 :]);
        address receiver;
        assembly {
            isXToY := shr(248, calldataload(add(myData.offset, 4)))
            pool := shr(96, calldataload(add(myData.offset, 5)))
            receiver := shr(96, calldataload(add(myData.offset, 25)))
        }

        uint amount0Out = amount;
        uint amount1Out = 0;
        if (!isXToY) {
            amount0Out = 0;
            amount1Out = amount;
        }
        IUniswapV2Pair pair = IUniswapV2Pair(pool);
        pair.swap(amount0Out, amount1Out, receiver, "");
        if (nextFunction == 0x00000000) {
            done = true;
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen :]);
    }
    //00000059
    function uniswapV2ExactInPayToAddress_35CB03(bytes calldata data) public beforeEachStep {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];
        bool isXToY;
        address pool;
        uint amount = toByte32Uint(myData[25 :]);
        address receiver;
        assembly {
            isXToY := shr(248, calldataload(add(myData.offset, 4)))
            pool := shr(96, calldataload(add(myData.offset, 5)))
            receiver := shr(96, calldataload(add(myData.offset, 25)))
        }

        uint amount0Out = 0;
        uint amount1Out = amount;
        if (!isXToY) {
            amount0Out = amount;
            amount1Out = 0;
        }
        IUniswapV2Pair pair = IUniswapV2Pair(pool);
        pair.swap(amount0Out, amount1Out, receiver, "");
        if (nextFunction == 0x00000000) {
            done = true;
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);

        next(data[dataLen :]);
    }

    //000000ea
    function paySender_7437EA(bytes calldata data) public beforeEachStep {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);

        bytes calldata myData = data[1 : dataLen];
        uint amount = toByte32Uint(myData[24 :]);
        address token;
        assembly {
            token := shr(96, calldataload(add(myData.offset, 4)))
        }
        emit bal(amount);
        emit bal(IERC20(token).balanceOf(address(this)));
        emit acc(token);
        IERC20(token).transfer(msg.sender, amount);
        if (nextFunction == 0x00000000) {
            done = true;
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen :]);
    }
    //00000081
    function payAddress_1A718EA(bytes calldata data) public beforeEachStep {
        (uint8 dataLen, bytes4 nextFunction) = getMeta(data);
        bytes calldata myData = data[1 : dataLen];
        address receiver;
        address token;
        uint amount = toByte32Uint(myData[44 :]);
        assembly {
            token := shr(96, calldataload(add(myData.offset, 4)))
            receiver := shr(96, calldataload(add(myData.offset, 24)))
        }
        emit acc(receiver);
        emit bal(amount);
        IERC20(token).transfer(receiver, amount);

        if (nextFunction == 0x00000000) {
            done = true;
            return;
        }
        function(bytes calldata) next = nextFunctionPointer(nextFunction);
        next(data[dataLen :]);

    }


    function uniswapV3SwapCallback(int256 amount0Delta, int256 amount1Delta, bytes calldata data) external {

        // amountToPay == debt, amountOut == asset
        (int256 amountToPay, int256 amountOut) =
        amount0Delta > 0 ? (amount0Delta, amount1Delta) : (amount1Delta, amount0Delta);
        emit imessage("amountToPay:", amountToPay);
        emit imessage("amountOut:", amountOut);
        (uint8 dataLen,) = getMeta(data);
        bytes4 nextFunction;
        assembly {
            nextFunction := calldataload(add(data.offset, 1))
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
        } else if (hash == 0x000000cd) {
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


}

