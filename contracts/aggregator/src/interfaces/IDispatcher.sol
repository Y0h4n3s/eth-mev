abstract contract Dispatcher {
    function dispatch(bytes1 commandType, bytes memory inputs) virtual internal returns (bool success, bytes memory output);
}