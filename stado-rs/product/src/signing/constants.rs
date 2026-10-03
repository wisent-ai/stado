pub const MACH_O_MAGICS: [[u8; 4]; 8] = [
    [0xfe, 0xed, 0xfa, 0xce],
    [0xce, 0xfa, 0xed, 0xfe],
    [0xfe, 0xed, 0xfa, 0xcf],
    [0xcf, 0xfa, 0xed, 0xfe],
    [0xca, 0xfe, 0xba, 0xbe],
    [0xbe, 0xba, 0xfe, 0xca],
    [0xca, 0xfe, 0xba, 0xbf],
    [0xbf, 0xba, 0xfe, 0xca],
];
pub const DEVELOPER_ID: &str = "Developer ID Application:";
pub const DEVELOPMENT: &str = "Apple Development:";
pub const BEGIN_CERTIFICATE: &str = "-----BEGIN CERTIFICATE-----";
pub const END_CERTIFICATE: &str = "-----END CERTIFICATE-----";
/// Apple's Worldwide Developer Relations G3 intermediate, the issuer of every
/// Apple Development and Developer ID certificate this fleet signs with. A
/// Mac whose keychains lack it builds no chain, and `security find-identity`
/// then reports the certificate as no valid identity at all. It is public,
/// so it travels inside Stado rather than as an object every signer must read.
pub const APPLE_ISSUERS_PEM: &str = include_str!("apple-issuers.pem");
