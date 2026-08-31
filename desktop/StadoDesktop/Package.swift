// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "StadoDesktop",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Stado", targets: ["Stado"]),
    ],
    dependencies: [
        // The revision that ships `WisentSkeleton` and the skeleton rows
        // `WisentLoadingPanel` now stands content in place with, and the auth
        // package that names the same one: SwiftPM admits exactly one revision
        // of a package per resolution, so a mixed pair does not resolve.
        .package(url: "https://github.com/wisent-ai/wisent-components.git", revision: "e52cdda9036b8d44c7ebf51626fcde606e6859b6"),
        .package(url: "https://github.com/wisent-ai/wisent-desktop-auth.git", revision: "b47177c97fe93b44ec72c74dedde23f3e1abb090"),
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
        .testTarget(
            name: "FleetTests",
            dependencies: ["Stado"],
            path: "tests/fleet"
        ),
        .testTarget(
            name: "LinkTests",
            dependencies: [
                "Stado",
                .product(name: "WisentDesignSystem", package: "wisent-components"),
            ],
            path: "tests/link"
        ),
        .testTarget(
            name: "ServiceTests",
            dependencies: [
                "Stado",
                .product(name: "WisentDesignSystem", package: "wisent-components"),
            ],
            path: "tests/service"
        ),
        .testTarget(
            name: "ProductTests",
            dependencies: ["Stado"],
            path: "tests/product"
        ),
    ]
)
