const { STATUS_CODES } = require("node:http");

const RELEASE_URI = /^stado:\/\/releases\/stado\/([A-Za-z\p{N}._-]+)\/(linux-amd64|darwin-arm64)\/([A-Za-z\p{N}._-]+)$/u;
const RELEASE_COORDINATES = new Set(["0.5.0-cobalt.1", "0.6.0"]);
const RELEASE_OBJECTS = new Set([
  "SHA256SUMS",
  "release-manifest.json",
  "stado",
  "stado-coverage",
  "stado-fix",
  "stado-mcp",
  "stado-watchdog",
  "wc",
]);
const MUTABLE_COORDINATES = new Set(["latest", "main", "master", "stable"]);

function statusNamed(label) {
  return Number(Object.keys(STATUS_CODES).find((key) => STATUS_CODES[key] === label));
}

module.exports = function releaseObject(request, response) {
  if (request.method !== "GET") {
    response.setHeader("Allow", "GET");
    response.status(statusNamed("Method Not Allowed")).json({ error: "method not allowed" });
    return;
  }

  const uri = typeof request.query.uri === "string" ? request.query.uri : "";
  const match = RELEASE_URI.exec(uri);
  const [, version, platform, name] = match || [];
  if (!match || !RELEASE_COORDINATES.has(version) || MUTABLE_COORDINATES.has(version) || !RELEASE_OBJECTS.has(name)) {
    response.status(statusNamed("Not Found")).json({ error: "release object not found" });
    return;
  }

  const asset = name === "release-manifest.json"
    ? `release-manifest-${platform}.json`
    : `${name}-${platform}`;
  const location = `https://github.com/wisent-ai/stado/releases/download/v${encodeURIComponent(version)}/${encodeURIComponent(asset)}`;
  response.redirect(location);
};
