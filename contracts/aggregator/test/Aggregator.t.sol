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
//    function test_scspsc() public {
//        // block 16920115
//        bytes memory data = hex"0000300020e95253e54490d8d30ea41574b24f741ee70201011002d89577d7d40200000008009e0905249ceefffb9605e034b534544684a58be6000a6706ba966100006000f6dcdce0ac3001b2f67f750bc64ea5beb37b58240120e95253e54490d8d30ea41574b24f741ee7020108152ea90000000080c02aaa39b223fe8d0a0e5c4f27ead9083c756cc21002d8107dccb30c7b";
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
//    function test_scscspsp() public {
//        // block 16920111
//        bytes memory data = hex"00000600a3f558aebaecaf0e11ca4b2199cc5ed341edfd74011005b12aefafa804000000070078235d08b2ae7a3e00184329212a4d7acd2f9985001211ef5ea5d78d3706ef00000010c02aaa39b223fe8d0a0e5c4f27ead9083c756cc21005ac4206e64fa673000080008d9b9e25b208cac58415d915898c2ffa3a530aa1000a5c8250059a000020005b8fb733f1a427e68533db48b7210d1548ee1dcd00082a72b6a8";
//        (bool success, bytes memory res) = address(agg).call{value: 1000}(data);
//
//        require(success);
//    }


//
//    function test_scscscsc() public {
//        // block 16920115
//        bytes memory data = hex"0000060011b815efb8f581194ae79006d24e0d814b7697f600102e36d06942ec8200000007007858e59e0c01ea06df3af3d20ac7b0003275d4bf010a0154c54e40000007006279653c28f138c8b31b8a0f6f8cd2c58e8c1705010a01551d193000000700fca9090d2c91e11cc546b0d7e4918c79e0088194000a01c57eb11a00000080c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2102e3386621e179c52";
//        (bool success, bytes memory res) = address(agg).call{value: 1000}(data);
//
//        require(success);
//    }
//    function test_scspspsp() public {
//        // block 16920115
//        bytes memory data = hex"0000060011b815efb8f581194ae79006d24e0d814b7697f6001005b12aefafa8040000000010c02aaa39b223fe8d0a0e5c4f27ead9083c756cc21005ac3caf5326f2bb00008000b4e16d0168e52d35cacd2c6185b44281ec28c9dc000829d0791800008000eb4b2b5e0eae7a0eadd0673ef8c3c830f8762f2800140193ee38b877317eed58000060002b9f8fe8ffc437a9008bb3097066f02b0a1c52ec0111b815efb8f581194ae79006d24e0d814b7697f60829f87b38";
//        (bool success, bytes memory res) = address(agg).call{value: 1000}(data);
//
//        require(success);
//    }
    function test_SushiSwapExactOutPayToSelf_SushiSwapExactOutPayToSender_SushiSwapExactOutPayToSender_UniswapV3ExactOutPayToSender_UniswapV3ExactOutPayToSender_PaybackPayToSender() public {
        // block 16920111
        bytes memory data = hex"0000060011b815efb8f581194ae79006d24e0d814b7697f6001002d89577d7d402000000070048da0965ab2d2cbf1c17c09cfb5cbe67ad5b1406010814d4c3f5000200000b09dea16768f0799065c475be02919503cb2a3500020000000000000000001ac02aaa39b223fe8d0a0e5c4f27ead9083c756cc26b175474e89094c44da98b954eedeac495271d0f1212f54ecbe3987a9b2d1002d883c1ad469336";
        (bool success, bytes memory res) = address(agg).call{value: 1000}(data);

        require(success);
    }




}
