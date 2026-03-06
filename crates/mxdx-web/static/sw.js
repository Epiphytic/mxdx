const CACHE_NAME = "mxdx-v1";

const STATIC_ASSETS = [
  "/manifest.webmanifest",
  "/sw.js",
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches.open(CACHE_NAME).then((cache) => cache.addAll(STATIC_ASSETS))
  );
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches.keys().then((keys) =>
      Promise.all(
        keys
          .filter((key) => key !== CACHE_NAME)
          .map((key) => caches.delete(key))
      )
    )
  );
  self.clients.claim();
});

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);

  // Network-first for HTMX partials (dashboard polling, etc.)
  if (
    event.request.headers.get("HX-Request") ||
    url.pathname.startsWith("/dashboard")
  ) {
    event.respondWith(
      fetch(event.request)
        .then((response) => {
          const clone = response.clone();
          caches.open(CACHE_NAME).then((cache) => cache.put(event.request, clone));
          return response;
        })
        .catch(() => caches.match(event.request))
    );
    return;
  }

  // Cache-first for static assets
  event.respondWith(
    caches.match(event.request).then((cached) => {
      if (cached) {
        return verifyIntegrity(cached);
      }
      return fetch(event.request).then((response) => {
        const clone = response.clone();
        caches.open(CACHE_NAME).then((cache) => cache.put(event.request, clone));
        return response;
      });
    })
  );
});

async function verifyIntegrity(response) {
  // SRI verification: if the response has a known hash, verify it.
  // For cached static assets, confirm the body has not been tampered with.
  const integrityHeader = response.headers.get("X-Content-Hash");
  if (!integrityHeader) {
    return response;
  }

  const body = await response.clone().arrayBuffer();
  const hashBuffer = await crypto.subtle.digest("SHA-256", body);
  const hashArray = Array.from(new Uint8Array(hashBuffer));
  const hashHex = hashArray.map((b) => b.toString(16).padStart(2, "0")).join("");
  const expected = integrityHeader.replace("sha256-", "");

  if (hashHex !== expected) {
    // Integrity mismatch — evict from cache and fetch fresh copy
    const cache = await caches.open(CACHE_NAME);
    await cache.delete(response.url);
    return fetch(response.url);
  }

  return response;
}
