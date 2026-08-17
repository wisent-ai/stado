// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "StadoDesktop",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Stado", targets: ["Stado"]),
    ],
    dependencies: [
        .package(url: "https://github.com/wisent-ai/wisent-components.git", revision: "1700f22dd179dd96a0212dd012e8a0e86aaccd60"),
        .package(url: "https://github.com/wisent-ai/wisent-desktop-auth.git", revision: "3bd2401cbb360a1326893308e5b4d336b8370644"),
        // By url, not by sibling path: CI checks out this repository alone, and
        // a missing sibling makes the whole graph unresolvable there.
        .package(url: "https://github.com/wisent-ai/echo.git", from: "0.1.2"),
    ],
    targets: [
        .executableTarget(
            name: "Stado",
            dependencies: [
                .product(name: "WisentDesignSystem", package: "wisent-components"),
                .product(name: "WisentAuth", package: "wisent-desktop-auth"),
                .product(name: "WisentOnboarding", package: "echo"),
            ],
            path: "Sources/Stado",
            resources: [.process("Resources")]
        ),

    ]
)
