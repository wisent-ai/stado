// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import "@openzeppelin/contracts/token/ERC20/extensions/ERC20Burnable.sol";
import "@openzeppelin/contracts/access/Ownable.sol";
import "@openzeppelin/contracts/token/ERC20/extensions/ERC20Permit.sol";

/// @title Wisent Token (WST)
/// @notice Platform utility token for compute.wisent.com GPU marketplace
/// @dev ERC-20 with owner mint capability for platform operations
contract WisentToken is ERC20, ERC20Burnable, Ownable, ERC20Permit {
    /// @notice Emitted when tokens are minted by platform
    event PlatformMint(address indexed to, uint256 amount, string reason);

    /// @notice Emitted when deposit is credited to a user account
    event DepositCredited(address indexed from, uint256 amount, bytes32 indexed depositId);

    /// @notice Maximum supply cap (1 billion tokens)
    uint256 public constant MAX_SUPPLY = 1_000_000_000 * 10 ** 18;

    constructor(
        address initialOwner,
        uint256 initialSupply
    ) ERC20("Wisent", "WST") Ownable(initialOwner) ERC20Permit("Wisent") {
        require(initialSupply <= MAX_SUPPLY, "Exceeds max supply");
        _mint(initialOwner, initialSupply);
    }

    /// @notice Mint new tokens (only owner/platform)
    /// @param to Recipient address
    /// @param amount Amount to mint in wei
    /// @param reason Human-readable reason for the mint
    function platformMint(
        address to,
        uint256 amount,
        string calldata reason
    ) external onlyOwner {
        require(totalSupply() + amount <= MAX_SUPPLY, "Exceeds max supply");
        _mint(to, amount);
        emit PlatformMint(to, amount, reason);
    }

    /// @notice Mark a deposit as credited (for event tracking)
    /// @param from User who deposited
    /// @param amount Amount deposited
    /// @param depositId Platform deposit record ID
    function markDepositCredited(
        address from,
        uint256 amount,
        bytes32 depositId
    ) external onlyOwner {
        emit DepositCredited(from, amount, depositId);
    }
}
