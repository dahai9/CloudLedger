function configuredBaseUrl(value: string | undefined) {
  return value?.trim().replace(/\/+$/, "") || undefined;
}

function defaultWebBaseUrl() {
  const { protocol, hostname } = window.location;
  const isWebOrigin = protocol === "http:" || protocol === "https:";
  const isTauriOrigin = hostname === "tauri.localhost";

  if (isWebOrigin && hostname && !isTauriOrigin) {
    const host = hostname.includes(":") && !hostname.startsWith("[") ? `[${hostname}]` : hostname;
    return `${protocol}//${host}:8787`;
  }

  return "http://127.0.0.1:8787";
}

export const cloudBaseUrl =
  configuredBaseUrl(window.__CLOUDLEDGER_CONFIG__?.apiBaseUrl) ??
  configuredBaseUrl(import.meta.env.VITE_CLOUDLEDGER_CLOUD_URL) ??
  defaultWebBaseUrl();
