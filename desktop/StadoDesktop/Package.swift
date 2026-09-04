// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "StadoDesktop",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Stado", targets: ["Stado"]),
    ],
    dependencies: [
        // The design system by version, not by commit: 0.8.1 declares no
        // dependencies of its own, so an exact version requirement is legal,
        // and it ships `WisentSkeleton` and the skeleton rows
        // `WisentLoadingPanel` stands content in place with. SwiftPM admits
        // exactly one requirement per package per resolution, and
        // `wisent-desktop-auth` 0.3.1 names this same `exact: "0.8.1"`,
        // so the pair agrees. Auth is on a version too now that
        // `wisent-errors` is tagged 1.0.0 and 0.3.1 names it by version.
        .package(url: "https://github.com/wisent-ai/wisent-components.git", exact: "0.8.1"),
        .package(url: "https://github.com/wisent-ai/wisent-desktop-auth.git", exact: "0.3.1"),
        // By url, not by sibling path: CI checks out this repository alone, and
        // a missing sibling makes the whole graph unresolvable there.
        .package(url: "https://github.com/wisent-ai/echo.git", exact: "0.3.0"),
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
            path: "tests/fleet",
            exclude: ["HostsDynamicCapacity.probierz.mjs"]
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
