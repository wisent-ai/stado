import { ethers } from 'hardhat';

async function main() {
  const [deployer] = await ethers.getSigners();
  console.log('Deploying WisentToken with account:', deployer.address);
  console.log('Account balance:', (await ethers.provider.getBalance(deployer.address)).toString());

  // Initial supply: 100 million WST (100M * 10^18)
  const initialSupply = ethers.parseEther('100000000');

  const WisentToken = await ethers.getContractFactory('WisentToken');
  const token = await WisentToken.deploy(deployer.address, initialSupply);
  await token.waitForDeployment();

  const address = await token.getAddress();
  console.log('WisentToken deployed to:', address);
  console.log('Initial supply:', ethers.formatEther(initialSupply), 'WST');
  console.log('Max supply: 1,000,000,000 WST');
  console.log('');
  console.log('Set WST_TOKEN_ADDRESS=' + address + ' in your .env');
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
