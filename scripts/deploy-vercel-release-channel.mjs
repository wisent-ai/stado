#!/usr/bin/env node
import { readFile } from "node:fs/promises";

const required = (name) => {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required`);
  return value;
};

const authFile = required("VERCEL_AUTH_FILE");
const projectName = required("VERCEL_PROJECT_NAME");
const teamId = required("VERCEL_TEAM_ID");
const functionFile = required("STADO_RELEASE_FUNCTION_FILE");
const auth = JSON.parse(await readFile(authFile, "utf8"));
if (typeof auth.token !== "string" || !auth.token) {
  throw new Error("Vercel authentication token is missing");
}

const source = await readFile(functionFile);
const endpoint = new URL("https://api.vercel.com/v13/deployments");
endpoint.searchParams.set("teamId", teamId);
const deploymentResponse = await fetch(endpoint, {
  method: "POST",
  headers: {
    Authorization: `Bearer ${auth.token}`,
    "Content-Type": "application/json",
  },
  body: JSON.stringify({
    name: projectName,
    target: "production",
    projectSettings: { framework: null },
    files: [{
      file: "api/release/object.js",
      data: source.toString("base64"),
      encoding: "base64",
    }],
  }),
});
const deployment = await deploymentResponse.json();
if (!deploymentResponse.ok) {
  throw new Error(`Vercel deployment failed: ${JSON.stringify(deployment)}`);
}
const hostname = deployment.alias?.find(Boolean) || deployment.url;
if (!hostname) throw new Error("Vercel deployment returned no hostname");
process.stdout.write(JSON.stringify({
  id: deployment.id,
  project: deployment.name,
  ready_state: deployment.readyState,
  url: `https://${hostname}`,
}) + "\n");
