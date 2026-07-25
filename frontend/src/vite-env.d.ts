/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_CLOUDLEDGER_CLOUD_URL?: string;
  readonly VITE_CLOUDLEDGER_USE_MOCK?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

interface Window {
  readonly __CLOUDLEDGER_CONFIG__?: {
    readonly apiBaseUrl?: string;
  };
}
