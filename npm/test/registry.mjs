// Just enough of an npm registry to publish to and install from.
import { createServer } from "node:http";

export async function startRegistry() {
  const packages = new Map();
  const published = [];
  let failWith = 0;

  const server = createServer(async (req, res) => {
    const send = (status, body) => {
      res.writeHead(status, { "content-type": "application/json" });
      res.end(JSON.stringify(body));
    };
    if (failWith) return send(failWith, { error: "unavailable" });

    const [name, , file] = decodeURIComponent(req.url.split("?")[0].slice(1)).split(/(\/-\/)/);
    if (req.method === "PUT") {
      const chunks = [];
      for await (const chunk of req) chunks.push(chunk);
      const doc = JSON.parse(Buffer.concat(chunks));
      const [version] = Object.keys(doc.versions);
      const attachment = Object.entries(doc._attachments).find(([n]) => n.endsWith(".tgz"))[1];
      const known = packages.get(doc.name) ?? { name: doc.name, versions: {}, "dist-tags": {}, files: {} };
      const path = `${doc.name.split("/").pop()}-${version}.tgz`;
      const manifest = doc.versions[version];
      manifest.dist.tarball = `${url}/${doc.name}/-/${path}`;
      known.versions[version] = manifest;
      known["dist-tags"].latest = version;
      known.files[path] = Buffer.from(attachment.data, "base64");
      packages.set(doc.name, known);
      published.push({ name: doc.name, version, access: doc.access });
      return send(201, { ok: true });
    }

    const known = packages.get(name);
    if (!known) return send(404, { error: "Not found" });
    if (file) {
      res.writeHead(200, { "content-type": "application/octet-stream" });
      return res.end(known.files[file]);
    }
    const { files, ...packument } = known;
    return send(200, packument);
  });

  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const url = `http://127.0.0.1:${server.address().port}`;
  return {
    url,
    packages,
    published,
    failNextWith: (status) => (failWith = status),
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}
