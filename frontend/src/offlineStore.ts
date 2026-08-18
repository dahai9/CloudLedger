import { invoke } from "@tauri-apps/api/core";
import type { AuditPeriod, Ledger, LedgerDashboard, OfflineTransaction, UserAccount } from "./types";

export interface OfflineSnapshot {
  version: 3;
  user: UserAccount;
  ledgers: Ledger[];
  dashboards: Record<string, LedgerDashboard>;
  auditPeriods: Record<string, AuditPeriod>;
  activeLedgerId?: string;
  activityMonth: string;
  outbox: OfflineTransaction[];
}

const isTauriRuntime =
  "__TAURI_INTERNALS__" in window || window.location.hostname === "tauri.localhost";
const databaseName = "cloudledger-offline";
const storeName = "state";
const activeUserKey = "active-user";

export const offlineStore = {
  async loadLast(): Promise<OfflineSnapshot | undefined> {
    const mirrored = await browserLoadLast().catch(() => undefined);
    if (isOfflineSnapshot(mirrored)) return mirrored;

    const document = isTauriRuntime ? await invoke<unknown>("offline_cache_load") : undefined;
    if (isOfflineSnapshot(document)) {
      await browserSave(document).catch(() => undefined);
      return document;
    }
    return undefined;
  },

  async loadAuthoritative(): Promise<OfflineSnapshot | undefined> {
    const document = isTauriRuntime
      ? await invoke<unknown>("offline_cache_load")
      : await browserLoadLast();
    return isOfflineSnapshot(document) ? document : undefined;
  },

  async save(snapshot: OfflineSnapshot): Promise<void> {
    if (isTauriRuntime) {
      await invoke("offline_cache_store", {
        userId: snapshot.user.id,
        document: snapshot,
      });
      await browserSave(snapshot).catch(() => undefined);
      return;
    }
    await browserSave(snapshot);
  },

  async clear(userId: string): Promise<void> {
    if (isTauriRuntime) {
      await invoke("offline_cache_clear", { userId });
      await browserClear(userId).catch(() => undefined);
      return;
    }
    await browserClear(userId);
  },
};

function isOfflineSnapshot(value: unknown): value is OfflineSnapshot {
  if (!value || typeof value !== "object") return false;
  const snapshot = value as Partial<OfflineSnapshot>;
  return (
    snapshot.version === 3 &&
    Boolean(snapshot.user?.id) &&
    Array.isArray(snapshot.ledgers) &&
    Boolean(snapshot.dashboards) &&
    Boolean(snapshot.auditPeriods) &&
    Array.isArray(snapshot.outbox) &&
    typeof snapshot.activityMonth === "string"
  );
}

async function browserLoadLast(): Promise<unknown> {
  const database = await openDatabase();
  const activeUserId = await request<string | undefined>(
    database.transaction(storeName, "readonly").objectStore(storeName).get(activeUserKey),
  );
  if (!activeUserId) return undefined;
  return request<unknown>(
    database.transaction(storeName, "readonly").objectStore(storeName).get(activeUserId),
  );
}

async function browserSave(snapshot: OfflineSnapshot): Promise<void> {
  const database = await openDatabase();
  const transaction = database.transaction(storeName, "readwrite");
  const store = transaction.objectStore(storeName);
  store.put(snapshot, snapshot.user.id);
  store.put(snapshot.user.id, activeUserKey);
  await transactionDone(transaction);
}

async function browserClear(userId: string): Promise<void> {
  const database = await openDatabase();
  const transaction = database.transaction(storeName, "readwrite");
  const store = transaction.objectStore(storeName);
  store.delete(userId);
  const activeUserId = await request<string | undefined>(store.get(activeUserKey));
  if (activeUserId === userId) store.delete(activeUserKey);
  await transactionDone(transaction);
}

function openDatabase(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const open = indexedDB.open(databaseName, 3);
    open.onupgradeneeded = (event) => {
      const database = open.result;
      if (!database.objectStoreNames.contains(storeName)) {
        database.createObjectStore(storeName);
        return;
      }
      if ((event as IDBVersionChangeEvent).oldVersion < 3) {
        open.transaction?.objectStore(storeName).clear();
      }
    };
    open.onsuccess = () => resolve(open.result);
    open.onerror = () => reject(open.error ?? new Error("无法打开离线存储"));
  });
}

function request<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error("离线存储读取失败"));
  });
}

function transactionDone(transaction: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    transaction.oncomplete = () => resolve();
    transaction.onerror = () => reject(transaction.error ?? new Error("离线存储写入失败"));
    transaction.onabort = () => reject(transaction.error ?? new Error("离线存储写入已取消"));
  });
}
