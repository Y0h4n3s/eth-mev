pragma solidity ^0.8.15;

import "forge-std/Test.sol";
import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {HuffDeployer} from "foundry-huff/HuffDeployer.sol";
interface Aggregator {
    function uniswapV3ExactOutPayToSelf_1377F03(bytes calldata) external;
}

interface IWETH9 {
    function deposit() external payable;
    function balanceOf(address fo) external returns(uint);
    function transfer(address dst, uint256 wad) external payable;
    function transferFrom(address src, address dst, uint wad)
    external
    returns (bool);
    function approve(address guy, uint256 wad) external returns (bool);
}
contract AggregatorTest is Test {
    Aggregator public agg;
    address constant private WETH9 = address(0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2);
    IWETH9 weth;
    function setUp() public {
        agg = Aggregator(HuffDeployer.deploy("AggregatorOptimized"));
        weth = IWETH9(WETH9);
        vm.startPrank(address(agg));
        weth.approve(address(0xBA12222222228d8Ba445958a75a0704d566BF2C8), 11111111111111111602953988394882385);
        vm.stopPrank();
    }
//
//    function test_scscsc() public {
//        // block 16918621
//        bytes memory data = hex"00000600a3f558aebaecaf0e11ca4b2199cc5ed341edfd74011002d89577d7d40200000007008adfed0b833bf4bd2c4159e93a910f4a06417102011208f13adccf8b30ac34000007009437ad40056ca3ec2fc1efe41885ad4b6ac460610014115d2c2c9e53bb0886cb00000080c02aaa39b223fe8d0a0e5c4f27ead9083c756cc21002d80462955320f2";
//        (bool success, bytes memory res) = address(agg).call{value: 1000}(data);
//
//        require(success);
//    }
//    function test_scspsp() public {
//        // block 16919015
//        bytes memory data = hex"000006002f62f2b4c5fcd7570a709dec05d68ea19c82a9ec0110049b9ca9a694340000000010c02aaa39b223fe8d0a0e5c4f27ead9083c756cc210049b16ee78810f940000800006da0fd433c1a5d7a4faa01111c044910a184553010822a7ce1c0000600098c2b0681d8bf07767826ea8bd3b11b0ca421631002f62f2b4c5fcd7570a709dec05d68ea19c82a9ec162c39705644d768ccb6f05a";
//        (bool success, bytes memory res) = address(agg).call{value: 1000}(data);
//
//        require(success);
//    }
//    function test_scsp() public {
//        // block 16919012
//        bytes memory data = hex"0000060088e6a0c2ddd26feeb64f039a2c41296fcb3f5640011002d89577d7d4020000000010c02aaa39b223fe8d0a0e5c4f27ead9083c756cc21002d86271bdb455fe00002000cd452c162da7761f08f656b8e5ede3a3859813780008157e2a18";
//        (bool success, bytes memory res) = address(agg).call{value: 1000}(data);
//
//        require(success);
//    }

//    function test_scscsp() public {
//        // block 16919012
//
//        bytes memory data = hex"000006004585fe77225b41b697c938b018e2ac67ac5a20c0011005b12aefafa80400000007009a772018fbd77fcd2d25657e5c547baff3fd7d16000627865700000010c02aaa39b223fe8d0a0e5c4f27ead9083c756cc21005afd2a6f402c24100002000cd452c162da7761f08f656b8e5ede3a38598137800082aeb98f2";
//        (bool success, bytes memory res) = address(agg).call{value: 1000}(data);
//
//        require(success);
//    }

//    function test_scsc() public {
//        // block 16919287
//        bytes memory data = hex"00003000397ff1542f962076d0bfe58ea045ffa2d347aca001100959eb1c0e4ae200000007007bea39867e4169dbe237d55c8242a8f2fcdcc38700084641b77e00000080c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2100959e13ba42c4d32";
//        (bool success, bytes memory res) = address(agg).call{value: 1000}(data);
//        require(success);
//    }
    function test_SushiSwapExactOutPayToSelf_SushiSwapExactOutPayToSender_SushiSwapExactOutPayToSender_UniswapV3ExactOutPayToSender_UniswapV3ExactOutPayToSender_PaybackPayToSender() public {

        bytes memory data = hex"00003000397ff1542f962076d0bfe58ea045ffa2d347aca00110049b9ca9a694340000000700893f503fac2ee1e5b78665db23f9c94017aae97d010821c62b2d00000010c02aaa39b223fe8d0a0e5c4f27ead9083c756cc210049acf1ec48e37e500000800c2e9f25be6257c210d7adf0d4cd6e3e881ba25f800121f2a85cc2c3a583b1900006000055475920a8c93cffb64d039a8205f7acc7722d300893f503fac2ee1e5b78665db23f9c94017aae97d0a0ce5c564d7";
        (bool success, bytes memory res) = address(agg).call{value: 1000}(data);

        require(success);
    }




}
