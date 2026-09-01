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
        // `wisent-desktop-auth` at a8aed6d names this same `exact: "0.8.1"`,
        // so the pair agrees. Auth itself stays on a commit because it still
        // names `wisent-errors` by revision.
        .package(url: "https://github.com/wisent-ai/wisent-components.git", exact: "0.8.1"),
        .package(url: "https://github.com/wisent-ai/wisent-desktop-auth.git", revision: "a8aed6d2f9e3e2ea22799ee0cfe8bae2358a70ad"),
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
