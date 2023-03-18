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
//    function test_withdraw() public {
//        weth.deposit{value: 1000000000000000000000000}();
//        weth.transfer(address(agg), uint(1000000000000000000000000));
//        bytes memory data = hex"00000001000000000000000000000000C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc20000000000000000000000000000000000000000000000001000000000000000";
//        uint before = weth.balanceOf(address(agg));
//        vm.startPrank(address(0x5615dEB798BB3E4dFa0139dFa1b3D433Cc23b72f));
//
//        address(agg).call(data);
//        require(before > weth.balanceOf(address(agg)));
//
//    }
//
//    function test_withdraw_and_fail() public {
//        weth.deposit{value: 1000000000000000000000000}();
//        weth.transfer(address(agg), uint(1000000000000000000000000));
//        bytes memory data = hex"00000001000000000000000000000000C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc20000000000000000000000000000000000000000000000001000000000000000";
//        vm.startPrank(address(agg));
//        vm.expectRevert();
//        address(agg).call(data);
//
//    }

//    function test_approve() public {
//        weth.deposit{value: 1000000000000000000000000}();
//        weth.transfer(address(agg), uint(1000000000000000000000000));
//        bytes memory data = hex"00000002000000000000000000000000C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2000000000000000000000000C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc20000000000000000000000000000000000000000000000001000000000000000";
//        uint before = weth.balanceOf(address(agg));
//        vm.startPrank(address(0x5615dEB798BB3E4dFa0139dFa1b3D433Cc23b72f));
//
//        address(agg).call(data);
//        require(before > weth.balanceOf(address(agg)));
//
//    }
//    function test_UniswapV3ExactOutPayToSelf_UniswapV3ExactOutPayToSender_UniswapV3ExactOutPayToSender_PaybackPayToSender() public {
//
//        bytes memory data = hex"0000060001dbe1bc2b5dbd1022793d5ac3d52d1c8624b33e5b100d02ab486cedc000000020000020e5c16d0b16490142d8026f4b0394f2cc9f2705100ce5af43d4d4a47100002000003306c01f98f848092ad5ae57e5c7dc432f761d81100cf16618915b745700000080c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2100d015988ad0cde24";
//        (bool success, bytes memory res) = address(agg).call(data);
//        require(success);
//    }
//
//    function test_UniswapV3ExactOutPayToSelf_UniswapV2ExactOutPayToSender_UniswapV3ExactOutPayToSender_PaybackPayToSender() public {
//
//        bytes memory data = hex"00000600005ebd677545bc10fa0acbd9b8b462391c87f576e4101a055690d9db80000000000e019ec96dcb54331626b79d8450a3daa9bcfa02e0b0101642a21ac308588600002000017fbeb8f8296093b191b50dd3ea6fed12146c9edd12494b15794e02be94e700000080c02aaa39b223fe8d0a0e5c4f27ead9083c756cc210192c8f890b7877f9";
//        (bool success, bytes memory res) = address(agg).call(data);
//        require(success);
//    }
//    function test_UniswapV3ExactOutPayToSelf_UniswapV3ExactOutPayToSender_UniswapV3ExactOutPayToSender_UniswapV3ExactOutPayToSender_PaybackPayToSender() public {
//
//        bytes memory data = hex"00000600013887e82dbdbe8ec6db44e6298a2d48af572a3b781001a055690d9db8000000200001c0d19f4fae83eb51b2adb59eb649c7bc2b19b2f6120ac2e9f914543a9a4c0000200000f5db999a11146a25611f5d61b5d8379dde6e7455080bd518bc0000200001b1a19fafa68df18e63c73a1d14283887db051c711602d6f821680112ac6f712f00000080c02aaa39b223fe8d0a0e5c4f27ead9083c756cc210019f19e598e2e271";
//        (bool success, bytes memory res) = address(agg).call(data);
//        require(success);
//    }
//
//    function test_SushiSwapExactOutPayToSelf_UniswapV3ExactOutPayToSender_UniswapV3ExactOutPayToSender_UniswapV3ExactOutPayToSender_PaybackPayToSender() public {
//
//        bytes memory data = hex"0e00000000a914a9b9e03b6af84f9c6bd2e0e8d27d405695db1001a055690d9db8000000200001e081eeab0adde30588ba8d5b3f6ae5284790f54a107551850bb55731ba00002000019bf2f49ef2b555777af9ae9d7dea31932c60f2b4080c14db8000002000002d6fcfde9709343c4c7a78d91077473d6b60465314018257a1ffb4aafb126300000080c02aaa39b223fe8d0a0e5c4f27ead9083c756cc210019ec9e9d4ebb440";
//        (bool success, bytes memory res) = address(agg).call(data);
//        require(success);
//    }
//    function test_UniswapV3ExactOutPayToSelf_UniswapV3ExactOutPayToSender_UniswapV3ExactOutPayToSender_UniswapV3ExactOutPayToSender_SushiSwapExactOutPayToSender_PaybackPayToSender() public {
//
//        bytes memory data = hex"0000060001824a30f2984f9013f2c8d0a29c0a3cc5fd5c06731001a055690d9db8000000200000af852a6eed8287c6589f6b63ac4091264290f0531210319cbfd45a15322b00002000017fccbd86d90f8809b41d863b4a1bb68757e7c26d080bcb6ce4000020000019ff5aea95f3f6c82b323989c64abd9ae8b9cdfd124711c6b6050b3aaf7f0000000e00397ff1542f962076d0bfe58ea045ffa2d347aca0080b86036e00000080c02aaa39b223fe8d0a0e5c4f27ead9083c756cc210019f87ac0a58eebc";
//        (bool success, bytes memory res) = address(agg).call(data);
//        require(success);
//    }
//    function test_SushiSwapExactOutPayToSelf_SushiSwapExactOutPayToSender_SushiSwapExactOutPayToSender_UniswapV3ExactOutPayToSender_UniswapV3ExactOutPayToSender_PaybackPayToSender() public {
//
//        bytes memory data = hex"0e00000000cb2286d9471cc185281c4f763d34a962ed2129621001a055690d9db8000000000e01ebd49b4c8f7f0ded2ca8b951cf92a583e7b4c8e70a258b3294990000000e00ba87dc891945dbb3caeeaf822de208d7ea89b298120750f9cb63cd2e6aed00002000000ed8721b9f1af5f0bea82d4407b56ef011dc7b33080b7c5d9c0000200000840deeef2f115cf50da625f7368c24af6fe7441010018d4840c05703a000000080c02aaa39b223fe8d0a0e5c4f27ead9083c756cc210018fc1190ad7db17";
//        (bool success, bytes memory res) = address(agg).call(data);
//        require(success);
//    }


    function test_UniswapV2ExactOutPayToSelf_UniswapV2ExactOutPayToSender_BalancerWeightedExactOut() public {

        bytes memory data = hex"0e00000001dbaa04796cb5c05d02b8a41b702d9b67c13c9fa91008ac4f7e7fdd5d800000000e001c9922314ed1415c95b9fd453c3818fd41867d0bb473c2606f7ac371518b13d2d097c34c38ca33d5143d22d1284e8081d9903d0000000e00a0b86991c6218b36c1d19d4a2e9eb0ce3606eb4863a65a174cc725824188940255ad41c371f28f28084e33f5030000000e01dac17f958d2ee523a2206206994597c13d831ec70d4a11d5eeaac28ec3f61d100daf4d40471f1852083f223a0900000080c02aaa39b223fe8d0a0e5c4f27ead9083c756cc210081962d5ba387a21";
        (bool success, bytes memory res) = address(agg).call(data);
        require(success);

    }


}
