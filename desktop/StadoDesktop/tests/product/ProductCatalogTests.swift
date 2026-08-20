import Foundation
import XCTest
@testable import Stado

@MainActor
final class ProductCatalogTests: XCTestCase {
    func testDecodesJedenSterAndTamaWithBothSurfaces() throws {
        let data = Data(#"{"products":[{"id":"jeden","name":"Jeden","description":"agent","surfaces":[{"kind":"cli","repository":"wisent-ai/jeden"},{"kind":"desktop","repository":"wisent-ai/jeden-desktop"}],"installations":[{"surface":"cli","kind":"stado-release","repository":"wisent-ai/jeden"},{"surface":"desktop","kind":"desktop-release","repository":"wisent-ai/jeden-desktop"}]},{"id":"ster","name":"Ster","description":"representations","surfaces":[{"kind":"cli","repository":"wisent-ai/ster"},{"kind":"desktop","repository":"wisent-ai/ster-desktop"}],"installations":[{"surface":"cli","kind":"stado-release","repository":"wisent-ai/ster"},{"surface":"desktop","kind":"desktop-release","repository":"wisent-ai/ster-desktop"}]},{"id":"tama","name":"Tama","description":"policy","surfaces":[{"kind":"cli","repository":"wisent-ai/tama"},{"kind":"desktop","repository":"wisent-ai/tama-desktop"}],"installations":[{"surface":"cli","kind":"npm","repository":"wisent-ai/tama"},{"surface":"desktop","kind":"desktop-release","repository":"wisent-ai/tama-desktop"}]}]}"#.utf8)
        let catalog = try JSONDecoder().decode(ProductCatalogEnvelope.self, from: data)
        XCTAssertEqual(catalog.products.map(\.id), ["jeden", "ster", "tama"])
        XCTAssertEqual(catalog.products[0].installations.map(\.surface), ["cli", "desktop"])
        XCTAssertEqual(catalog.products[1].installations.map(\.surface), ["cli", "desktop"])
        XCTAssertEqual(catalog.products[2].installations.map(\.surface), ["cli", "desktop"])
    }

    func testLifecycleArgumentsNameTheExactSurfaceAndHost() {
        XCTAssertEqual(
            ProductsStore.lifecycleArguments("install", product: "weles", surface: "service", host: "charless-mac-mini"),
            ["product", "install", "weles", "--surface", "service", "--host", "charless-mac-mini", "--json"]
        )
        XCTAssertEqual(
            ProductsStore.lifecycleArguments("install", product: "tama", surface: "desktop", host: nil),
            ["product", "install", "tama", "--surface", "desktop", "--json"]
        )
    }
}
