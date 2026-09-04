// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const contentTypes = new Map([
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".wasm", "application/wasm"],
]);

export async function serveReport(fileUrl) {
  const indexPath = fileURLToPath(fileUrl);
  const root = path.dirname(indexPath);
  const server = createServer((request, response) => {
    sendFile(root, request.url, response).catch(() => {
      response.writeHead(500);
      response.end();
    });
  });

  await listen(server);
  const address = server.address();
  if (!address || typeof address === "string") {
    await close(server);
    throw new Error("Unable to determine the report server address");
  }

  return {
    url: `http://127.0.0.1:${address.port}/${path.basename(indexPath)}`,
    close: () => close(server),
  };
}

async function sendFile(root, requestUrl, response) {
  const url = new URL(requestUrl || "/", "http://127.0.0.1");
  const relativePath = decodeURIComponent(url.pathname).replace(/^\/+/, "");
  const target = path.resolve(root, relativePath || "index.html");
  const relativeTarget = path.relative(root, target);
  if (
    relativeTarget === ".." ||
    relativeTarget.startsWith(`..${path.sep}`) ||
    path.isAbsolute(relativeTarget)
  ) {
    response.writeHead(403);
    response.end();
    return;
  }

  try {
    const body = await readFile(target);
    response.writeHead(200, {
      "Cache-Control": "no-store",
      "Content-Type":
        contentTypes.get(path.extname(target)) || "application/octet-stream",
    });
    response.end(body);
  } catch (error) {
    if (error.code !== "ENOENT") {
      throw error;
    }
    response.writeHead(404);
    response.end();
  }
}

function listen(server) {
  return new Promise((resolve, reject) => {
    const onError = (error) => reject(error);
    server.once("error", onError);
    server.listen(0, "127.0.0.1", () => {
      server.off("error", onError);
      resolve();
    });
  });
}

function close(server) {
  return new Promise((resolve, reject) => {
    server.close((error) => (error ? reject(error) : resolve()));
  });
}
