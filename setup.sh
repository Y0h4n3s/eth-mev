apt update
apt install build-essential pkg-config clang libssl-dev screen nethogs -y
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --default-toolchain nightly --profile complete -y
source "$HOME/.cargo/env"
source "./garb/.env"
sudo add-apt-repository -y ppa:openjdk-r/ppa
sudo apt-get update
wget -O - https://debian.neo4j.com/neotechnology.gpg.key --no-check-certificate | sudo apt-key add -
echo 'deb https://debian.neo4j.com stable latest' | sudo tee -a /etc/apt/sources.list.d/neo4j.list
apt -o "Acquire::https::Verify-Peer=false" update
sudo apt-get install neo4j=1:5.4.0 -y