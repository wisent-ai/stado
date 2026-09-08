import Foundation

// MARK: - Container context

extension BackendProvisioner {
    func prepareContainerContext(stadoExecutable: URL) async throws -> URL {
        let rootValue = try await runCapture(stadoExecutable.path, ["package-root"])
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let source = URL(fileURLWithPath: rootValue).appendingPathComponent("stado", isDirectory: true)
        guard fileManager.fileExists(atPath: source.appendingPathComponent("cli.py").path) else {
            throw BackendProvisioningError.commandFailed("The installed Stado package source could not be located.")
        }
        let context = fileManager.temporaryDirectory
            .appendingPathComponent("stado-cloud-\(UUID().uuidString)", isDirectory: true)
        try fileManager.createDirectory(at: context, withIntermediateDirectories: true)
        try fileManager.copyItem(at: source, to: context.appendingPathComponent("stado", isDirectory: true))
        let dockerfile = """
        FROM python:\
        3.12-slim
        ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 PYTHONPATH=/app
        WORKDIR /app
        RUN pip install --no-cache-dir 'stado[aws,azure]'
        COPY stado /app/stado
        CMD ["stado", "cloud-control-plane", "--bind", "0.0.0.0", "--port", "8080", "--interval", "30"]
        """
        try dockerfile.write(
            to: context.appendingPathComponent("Dockerfile"),
            atomically: true,
            encoding: .utf8
        )
        return context
    }
}
